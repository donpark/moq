#!/usr/bin/env python3
"""Check that backports bump hand-versioned packages, and push their release tags.

release-plz bumps Rust crates on `release`; every other package's version is
bumped by hand, since only a human can judge a change's semver level. A
backport that changes one of them without a bump lands unchanged in version,
and its release workflow skips it as already published. `check` fails that PR;
it never picks or writes a version.

Two kinds of package publish differently:

- JS, Python, Swift, and Kotlin publish on a push to `release` once the registry
  lacks their version. Each first-parent commit is a push their release workflow
  saw, so one last shipped at the oldest first-parent commit carrying its
  current version.
- Dart, C, and C++ publish from a `<prefix><version>` tag, and OBS rides each
  C++ tag. One last shipped at its current version's tag; a current version
  with no tag is unreleased, so `tag` pushes it, while C and C++ wait until the
  moq-ffi release `cpp/moq/FFI_VERSION` pins is published.

Any change to a package's sources since it last shipped is unreleased, so it
needs a bump.

Usage:
  versions.py check [--base <rev>] [--head <rev>] [--report]
      Fail when merging head into base changes a package's shipped sources at a
      released version, lowers a version, or leaves a lockfile entry behind.
      Warns of the packages base already carries unreleased changes for, and
      those only a shared build script changed for.
      --report lists the failures without failing.
  versions.py tag [--dry-run]
      Push each release tag HEAD's versions lack. Needs GH_TOKEN.
"""

import argparse
from dataclasses import dataclass, field
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

REPO = "moq-dev/moq"

# The C and C++ packages' moq-ffi pin.
FFI_VERSION = "cpp/moq/FFI_VERSION"
FFI_SHA256 = "cpp/moq/FFI_SHA256.json"
FFI_CMAKE = "cpp/moq/cmake/moq-ffi.cmake"


def load_ffi():
    """sh/cpp/ffi.py, which names the pinned assets. Loaded only once C or C++ is
    present, since a `release` from before their cut lacks it."""
    spec = importlib.util.spec_from_file_location("ffi", Path(__file__).parents[1] / "cpp/ffi.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], capture_output=True, text=True, check=check)


def show(rev: str, path: str) -> str | None:
    result = git("show", f"{rev}:{path}", check=False)
    return result.stdout if result.returncode == 0 else None


def precedence(version: str) -> tuple | None:
    """A sort key for semver precedence, or None if `version` is not semver."""
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?", version)
    if not match:
        return None
    major, minor, micro, pre = match.groups()
    # A prerelease sorts before its release. Its identifiers compare in order,
    # numeric ones by value and below alphanumeric ones; build metadata is ignored.
    ids = [(0, int(part), "") if part.isdigit() else (1, 0, part) for part in pre.split(".")] if pre else []
    return (int(major), int(minor), int(micro), pre is None, ids)


def newer(version: str, than: str) -> bool | None:
    """Whether `version` sorts after `than` by semver, or None if either is not one."""
    a, b = precedence(version), precedence(than)
    return None if a is None or b is None else a > b


@dataclass
class Edit:
    """A version string in a file: group 2 of `pattern` is the version."""

    path: str
    pattern: str
    # The command that rewrites it, for a lockfile entry.
    fix: str | None = None

    def read(self, text: str) -> str | None:
        match = re.search(self.pattern, text, flags=re.M)
        return match.group(2) if match else None


@dataclass
class Package:
    name: str
    # The version's source of truth; its value at a commit says what shipped.
    manifest: Edit
    # Shipped sources, as git pathspecs.
    paths: list[str]
    # Copies of the version that must move with it, such as a lockfile entry.
    locks: list[Edit] = field(default_factory=list)
    # The prefix of the tags naming its releases, for a package a tag publishes.
    tag: str | None = None
    # The package whose tag publishes this one, when it is not its own.
    rides: str | None = None
    # Whether its tag waits for the pinned moq-ffi release.
    ffi: bool = False
    # Build scripts shared with other packages. A change to one may or may not
    # change what this ships, which only a human can judge.
    shared: list[str] = field(default_factory=list)

    def version(self, rev: str) -> str | None:
        text = show(rev, self.manifest.path)
        return None if text is None else self.manifest.read(text)


# Files none of these packages ship, so changing one alone is not a release.
UNSHIPPED = [
    ":(exclude,glob)**/*.test.*",
    ":(exclude,glob)**/tests/**",
    ":(exclude,glob)**/Tests/**",
    ":(exclude,glob)**/MIGRATE.md",
]


def packages(rev: str) -> list[Package]:
    """Every hand-versioned package present at `rev`. Go derives its patch in CI."""
    found = []
    listing = git("ls-tree", "--name-only", rev, "js/").stdout.split()
    for directory in sorted(listing):
        text = show(rev, f"{directory}/package.json")
        if text is None:
            continue
        manifest = json.loads(text)
        # CONTRIBUTING.md's rule: a release script, and not private.
        if manifest.get("private") or not manifest.get("scripts", {}).get("release"):
            continue
        name = manifest["name"]
        # The js/common scripts its build runs, such as the packager that
        # rewrites its published manifest.
        build = manifest["scripts"].get("build", "") + "\n" + (show(rev, f"{directory}/vite.config.ts") or "")
        shared = sorted({f"js/common/{script}" for script in re.findall(r"\.\./common/([\w.-]+\.ts)\b", build)})
        found.append(Package(
            name=name,
            manifest=Edit(f"{directory}/package.json", r'^(\t"version": ")([^"]+)(")'),
            # API reports, benches, and examples stay out of the built package.
            paths=[directory, *(f":(exclude){directory}/{d}" for d in ("api", "bench", "examples"))],
            locks=[Edit(
                "bun.lock",
                rf'^(    "{re.escape(directory)}": \{{\n      "name": "{re.escape(name)}",\n      "version": ")([^"]+)(")',
                fix="bun install",
            )],
            shared=shared,
        ))
    found.append(Package(
        name="moq-rs (Python)",
        manifest=Edit("py/moq-rs/pyproject.toml", r'^(\[project\]\n(?:(?!\[).*\n)*?version = ")([^"]+)(")'),
        paths=["py/moq-rs", ":(exclude)py/moq-rs/benches", ":(exclude)py/moq-rs/examples"],
        locks=[Edit("uv.lock", r'^(\[\[package\]\]\nname = "moq-rs"\nversion = ")([^"]+)(")', fix="uv lock")],
    ))
    found.append(Package(
        name="Moq (Swift)",
        manifest=Edit("swift/VERSION", r"\A()(\S+)(\s*)\Z"),
        paths=["swift/Sources/Moq", "swift/Package.swift.template"],
    ))
    found.append(Package(
        name="dev.moq:moq (Kotlin)",
        manifest=Edit("kt/gradle.properties", r"^(moq\.version=)(\S+)()$"),
        paths=["kt/moq", ":(exclude,glob)kt/moq/src/*Test/**", "kt/build.gradle.kts", "kt/settings.gradle.kts", "kt/gradle.properties"],
    ))
    version_file = r"\A()(\S+)(\s*)\Z"
    tagged = [
        Package(
            name="moq (Dart)",
            manifest=Edit("dart/moq/pubspec.yaml", r"^(version: )(\S+)()$"),
            paths=["dart/moq", ":(exclude)dart/moq/test", ":(exclude)dart/moq/bench"],
            tag="moq-dart-v",
        ),
        # The C package links the same pinned moq-ffi input as C++, through C++'s
        # CMake module.
        Package(
            name="moq (C)",
            manifest=Edit("c/moq/VERSION", version_file),
            paths=["c/moq", ":(exclude)c/moq/test", FFI_VERSION, FFI_SHA256, FFI_CMAKE],
            tag="c-v",
            ffi=True,
        ),
        Package(
            name="moq (C++)",
            manifest=Edit("cpp/moq/VERSION", version_file),
            paths=["cpp/moq", ":(exclude)cpp/moq/test", ":(exclude)cpp/moq/bench"],
            tag="cpp-v",
            ffi=True,
        ),
        # Release Cpp builds the plugin on every C++ tag and cuts its obs-moq tag.
        Package(
            name="obs-moq (OBS)",
            manifest=Edit("cpp/obs/VERSION", version_file),
            paths=["cpp/obs", ":(exclude)cpp/obs/test"],
            tag="obs-moq-v",
            rides="moq (C++)",
        ),
    ]
    # `release` gains a package only when a cut brings it.
    found += [pkg for pkg in tagged if show(rev, pkg.manifest.path) is not None]
    return found


def released(pkg: Package, head: str) -> str | None:
    """The commit that shipped `pkg`'s current version, or None while it is unreleased."""
    current = pkg.version(head)
    if current is None:
        raise ValueError(f"no version in {pkg.manifest.path} at {head}")
    if pkg.tag:
        result = git("rev-parse", "--quiet", "--verify", f"refs/tags/{pkg.tag}{current}^{{commit}}", check=False)
        return result.stdout.strip() or None
    # The oldest first-parent commit since which it has had its current version.
    log = git("log", "--first-parent", "--format=%H", head, "--", pkg.manifest.path).stdout.split()
    for rev in log:
        if pkg.version(f"{rev}^") != current:
            return rev
    return None


def baselines(found: list[Package], head: str) -> dict[str, str | None]:
    """`released` for each package in `found`."""
    since = {pkg.name: released(pkg, head) for pkg in found}
    # A rider's own tag comes later, from the release its host's tag starts, so
    # while that run is in flight its version shipped at the host's tag.
    for rider in found:
        host = since.get(rider.rides) if rider.rides else None
        if since[rider.name] is None and host and rider.version(host) == rider.version(head):
            since[rider.name] = host
    return since


def group(pkg: Package, found: list[Package]) -> set[str]:
    """`pkg` and every package one tag publishes along with it."""
    names = {pkg.name, *([pkg.rides] if pkg.rides else [])}
    return names | {other.name for other in found if other.rides in names}


@dataclass
class Unreleased:
    """A package still at a released version that must move to ship."""

    package: Package
    version: str
    since: str
    changed: list[str]
    # The package it releases alongside, when its own sources did not change.
    alongside: str | None = None


def unreleased(head: str) -> list[Unreleased]:
    """Every package at `head` whose released version no longer matches what would ship."""
    found = packages(head)
    since = baselines(found, head)
    stale: dict[str, Unreleased] = {}
    for pkg in found:
        if since[pkg.name] is None:
            continue
        changed = git("diff", "--name-only", since[pkg.name], head, "--", *pkg.paths, *pkg.shared, *UNSHIPPED).stdout.split()
        if changed:
            stale[pkg.name] = Unreleased(pkg, pkg.version(head), since[pkg.name], changed)

    # One tag publishes a package and every package riding it, so once any of
    # them releases, each other one still at a released version must move too.
    for rider in found:
        members = [pkg for pkg in found if pkg.name == rider.rides]
        if not members:
            continue
        members.append(rider)
        releasing = [pkg.name for pkg in members if pkg.name in stale or since[pkg.name] is None]
        if not releasing:
            continue
        for pkg in members:
            if pkg.name not in stale and since[pkg.name] is not None:
                stale[pkg.name] = Unreleased(pkg, pkg.version(head), since[pkg.name], [], alongside=releasing[0])
    return [stale[pkg.name] for pkg in found if pkg.name in stale]


def merge(base: str, head: str) -> str:
    """A commit landing `head` on `base` as a PR's merge does: first parent `base`."""
    result = git("merge-tree", "--write-tree", base, head, check=False)
    if result.returncode:
        raise RuntimeError(f"{head} does not merge cleanly into {base}")
    tree = result.stdout.split()[0]
    identity = {f"GIT_{who}_{key}": value for who in ("AUTHOR", "COMMITTER")
                for key, value in (("NAME", "versions.py"), ("EMAIL", "versions.py@localhost"))}
    return subprocess.run(["git", "commit-tree", tree, "-p", base, "-p", head, "-m", f"merge {head} into {base}"],
                          capture_output=True, text=True, check=True, env={**os.environ, **identity}).stdout.strip()


def how(pkg: Package, version: str) -> str:
    """How to move `pkg` past its released `version`."""
    text = f"raise its version in {pkg.manifest.path} past {version} (CONTRIBUTING.md's Versions: patch for a fix)"
    fixes = [lock.fix for lock in pkg.locks if lock.fix]
    if fixes:
        text += ", then run " + " and ".join(f"`{fix}`" for fix in fixes)
    return text


def check(base: str, head: str) -> tuple[list[str], list[str]]:
    """The problems merging `head` into `base` brings, and warnings: the packages
    `base` already holds unreleased changes for, and those only a shared build
    script changed for."""
    merged = merge(base, head)
    found = packages(merged)
    touched = {
        pkg.name for pkg in found
        if git("diff", "--name-only", base, merged, "--", pkg.manifest.path, *pkg.paths, *pkg.shared, *UNSHIPPED).stdout.strip()
    }

    problems, warnings = [], []
    for item in unreleased(merged):
        pkg = item.package
        shared = [path for path in item.changed if path in pkg.shared]
        only_shared = bool(item.changed) and len(shared) == len(item.changed)
        fix = how(pkg, item.version)
        if item.alongside:
            reason = f"{item.alongside} releases, and the same tag would publish its released {item.version} again"
        elif only_shared:
            reason = f"only the shared build {'script' if len(shared) == 1 else 'scripts'} {', '.join(shared)} changed since {item.version} shipped"
            fix = f"if that changed what it ships, {fix}"
        else:
            reason = f"{len(item.changed)} shipped {'file' if len(item.changed) == 1 else 'files'} changed since {item.version} shipped"
        if not group(pkg, found) & touched:
            reason += f", already on {base}"
        line = f"{pkg.name}: {reason}; {fix}."
        (problems if group(pkg, found) & touched and not only_shared else warnings).append(line)

    for pkg in found:
        if pkg.name not in touched:
            continue
        old, new = pkg.version(base), pkg.version(merged)
        if old and new != old:
            order = newer(new, old)
            if order is None:
                problems.append(f"{pkg.name}: cannot order {new} after {old}, the version on {base}, as semver.")
            elif not order:
                problems.append(f"{pkg.name}: {new} does not come after {old}, the version on {base}.")
        for lock in pkg.locks:
            text = show(merged, lock.path)
            have = lock.read(text) if text is not None else None
            if have != new:
                problems.append(f"{pkg.name}: {lock.path} carries {have}, not {new}; run `{lock.fix}`.")
    return problems, warnings


def release_assets(tag: str) -> set[str]:
    """The asset names on the GitHub release for `tag`, empty when it has none."""
    result = subprocess.run(["gh", "release", "view", tag, "--repo", REPO, "--json", "assets", "--jq", ".assets[].name"],
                            capture_output=True, text=True)
    if result.returncode:
        if "release not found" in result.stderr.lower():
            return set()
        raise RuntimeError(result.stderr)
    return set(result.stdout.split())


def ffi_blocker(head: str) -> str | None:
    """Why a C or C++ tag on `head` must wait for moq-ffi, or None once its pin is published."""
    text = show(head, FFI_VERSION) or ""
    pins = [line.strip() for line in text.splitlines() if line.strip() and not line.strip().startswith("#")]
    if len(pins) != 1:
        return f"{FFI_VERSION} pins no single moq-ffi release"
    release = pins[0]
    ffi = load_ffi()
    hashes = json.loads(show(head, FFI_SHA256) or "{}")
    published = release_assets(f"moq-ffi-v{release}")
    missing = [asset for asset in (ffi.name(release, target) + ".tar.gz" for target in ffi.TARGETS)
               if asset not in hashes or asset not in published]
    if missing:
        return f"moq-ffi v{release} lacks the published, pinned {', '.join(missing)}"
    return None


@dataclass
class Tag:
    name: str
    package: Package
    # Why it cannot be pushed yet, or None.
    blocker: str | None


def pending(head: str) -> list[Tag]:
    """Each release tag HEAD's versions lack, for the packages that push their own."""
    found = packages(head)
    since = baselines(found, head)
    tags = []
    ffi: str | None = None
    checked = False
    for pkg in found:
        if not pkg.tag or pkg.rides or since[pkg.name] is not None:
            continue
        blocker = None
        # A rider still at a released version must be bumped first: the tag
        # would publish that version again, which its release refuses.
        stale = [rider for rider in found if rider.rides == pkg.name and since[rider.name] is not None]
        if stale:
            blocker = f"{stale[0].name} {stale[0].version(head)} is already released; bump it on `release` first"
        elif pkg.ffi:
            if not checked:
                ffi, checked = ffi_blocker(head), True
            blocker = ffi
        tags.append(Tag(f"{pkg.tag}{pkg.version(head)}", pkg, blocker))
    return tags


def push_tags(dry_run: bool = False) -> None:
    head = git("rev-parse", "HEAD").stdout.strip()
    tags = pending(head)
    for tag in tags:
        if tag.blocker:
            # The next push to `release` retries it.
            print(f"waiting to push {tag.name}: {tag.blocker}")
            continue
        if dry_run:
            print(f"would push {tag.name} at {head[:10]}")
            continue
        result = git("push", "origin", f"{head}:refs/tags/{tag.name}", check=False)
        if result.returncode:
            raise RuntimeError(f"pushing {tag.name} failed:\n{result.stderr}")
        print(f"pushed {tag.name} at {head[:10]}")
    if not tags:
        print("nothing to tag")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    check_parser = sub.add_parser("check")
    check_parser.add_argument("--base", default="origin/release")
    check_parser.add_argument("--head", default="HEAD")
    check_parser.add_argument("--report", action="store_true")
    tag_parser = sub.add_parser("tag")
    tag_parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if args.command == "tag":
        push_tags(args.dry_run)
        return 0

    try:
        problems, warnings = check(args.base, args.head)
    except RuntimeError as error:
        print(error, file=sys.stderr)
        return 2
    actions = os.environ.get("GITHUB_ACTIONS")
    for line in warnings:
        print(f"::warning::{line}" if actions else f"warning: {line}")
    for line in problems:
        print(f"::error::{line}" if actions and not args.report else line)
    if not problems:
        print(f"no version problems merging {args.head} into {args.base}")
        return 0
    if args.report:
        return 0
    print(f"{len(problems)} version {'problem' if len(problems) == 1 else 'problems'}; "
          "a backport bumps each hand-versioned package whose shipped sources it changes, "
          "or its release workflow skips the fix as already published.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
