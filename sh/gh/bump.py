#!/usr/bin/env python3
"""Patch-bump hand-versioned packages whose sources changed since they last shipped.

release-plz bumps Rust crates on `release`; this does the same for the packages
whose version a human owns. Without a bump, a backport touching only one of
them lands unchanged in version and never publishes.

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
needs a patch bump.

Usage:
  bump.py check [--ref <rev>]   List the packages that need a bump or a tag.
  bump.py apply                 Bump them in the working tree.
  bump.py pr                    Bump them on the `release-bump` branch and open
                                or update its PR into `release`; close it when
                                nothing is left. Needs GH_TOKEN.
  bump.py tag                   Push each release tag HEAD's versions lack.
                                Needs GH_TOKEN.
"""

import argparse
from dataclasses import dataclass, field
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys

BASE = "release"
BRANCH = "release-bump"
TITLE = "chore: patch-bump hand-versioned packages"
REPO = "moq-dev/moq"

# The C and C++ packages' moq-ffi pin.
FFI_VERSION = "cpp/moq/FFI_VERSION"
FFI_SHA256 = "cpp/moq/FFI_SHA256.json"


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


def patch(version: str) -> str:
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version)
    if not match:
        raise ValueError(f"cannot patch-bump non-release version {version!r}")
    major, minor, micro = match.groups()
    return f"{major}.{minor}.{int(micro) + 1}"


def substitute(pattern: str, text: str, version: str, path: str) -> str:
    """Replace group 2 of the first match of `pattern` with `version`."""
    out, count = re.subn(pattern, lambda m: m.group(1) + version + m.group(3), text, count=1, flags=re.M)
    if count != 1:
        raise ValueError(f"version not found in {path}")
    return out


@dataclass
class Edit:
    """A version string in a file: group 2 of `pattern` is the version."""

    path: str
    pattern: str

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
        found.append(Package(
            name=name,
            manifest=Edit(f"{directory}/package.json", r'^(\t"version": ")([^"]+)(")'),
            # API reports, benches, and examples stay out of the built package.
            paths=[directory, *(f":(exclude){directory}/{d}" for d in ("api", "bench", "examples"))],
            locks=[Edit("bun.lock", rf'^(    "{re.escape(directory)}": \{{\n      "name": "{re.escape(name)}",\n      "version": ")([^"]+)(")')],
        ))
    found.append(Package(
        name="moq-rs (Python)",
        manifest=Edit("py/moq-rs/pyproject.toml", r'^(\[project\]\n(?:(?!\[).*\n)*?version = ")([^"]+)(")'),
        paths=["py/moq-rs", ":(exclude)py/moq-rs/benches", ":(exclude)py/moq-rs/examples"],
        locks=[Edit("uv.lock", r'^(\[\[package\]\]\nname = "moq-rs"\nversion = ")([^"]+)(")')],
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
        # The C package links the same pinned moq-ffi input as C++.
        Package(
            name="moq (C)",
            manifest=Edit("c/moq/VERSION", version_file),
            paths=["c/moq", ":(exclude)c/moq/test", FFI_VERSION, FFI_SHA256],
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


@dataclass
class Bump:
    package: Package
    old: str
    new: str
    since: str
    changed: list[str]
    # The package it releases alongside, when its own sources did not change.
    alongside: str | None = None


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


def plan(head: str) -> list[Bump]:
    found = packages(head)
    since = baselines(found, head)
    bumps: dict[str, Bump] = {}
    for pkg in found:
        if since[pkg.name] is None:
            continue
        changed = git("diff", "--name-only", since[pkg.name], head, "--", *pkg.paths, *UNSHIPPED).stdout.split()
        if changed:
            old = pkg.version(head)
            bumps[pkg.name] = Bump(pkg, old, patch(old), since[pkg.name], changed)

    # One tag publishes a package and every package riding it, so once any of
    # them releases, each other one still at a released version needs a bump.
    for rider in found:
        group = [pkg for pkg in found if pkg.name == rider.rides]
        if not group:
            continue
        group.append(rider)
        releasing = [pkg.name for pkg in group if pkg.name in bumps or since[pkg.name] is None]
        if not releasing:
            continue
        for pkg in group:
            if pkg.name not in bumps and since[pkg.name] is not None:
                old = pkg.version(head)
                bumps[pkg.name] = Bump(pkg, old, patch(old), since[pkg.name], [], alongside=releasing[0])
    return [bumps[pkg.name] for pkg in found if pkg.name in bumps]


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
        # A rider still at a released version needs its bump merged first: the
        # tag would publish that version again, which its release refuses.
        stale = [rider for rider in found if rider.rides == pkg.name and since[rider.name] is not None]
        if stale:
            blocker = f"{stale[0].name} {stale[0].version(head)} is already released; merge its bump first"
        elif pkg.ffi:
            if not checked:
                ffi, checked = ffi_blocker(head), True
            blocker = ffi
        tags.append(Tag(f"{pkg.tag}{pkg.version(head)}", pkg, blocker))
    return tags


def apply(bumps: list[Bump], root: Path) -> None:
    # Every edit is checked before any file is written, so a stale lockfile
    # leaves the tree untouched. Packages share lockfiles, so edits chain.
    texts: dict[str, str] = {}
    for bump in bumps:
        for edit in [bump.package.manifest, *bump.package.locks]:
            text = texts.get(edit.path) or (root / edit.path).read_text()
            if edit.read(text) != bump.old:
                raise ValueError(f"{edit.path} does not carry {bump.package.name} {bump.old}")
            texts[edit.path] = substitute(edit.pattern, text, bump.new, edit.path)
    for path, text in texts.items():
        (root / path).write_text(text)


def body(bumps: list[Bump], head: str) -> str:
    lines = [
        "Patch-bumps every hand-versioned package whose sources changed on `release` since its current version shipped, "
        "so merging this publishes them through their release workflows.",
        "",
        "| Package | Version | Unreleased changes |",
        "| --- | --- | --- |",
    ]
    for bump in bumps:
        files = "file" if len(bump.changed) == 1 else "files"
        compare = f"[{len(bump.changed)} {files}](https://github.com/{REPO}/compare/{bump.since[:10]}...{head[:10]})"
        if bump.alongside:
            compare = f"none; releases with {bump.alongside}"
        lines.append(f"| {bump.package.name} | {bump.old} → {bump.new} | {compare} |")
    tags = [f"`{b.package.tag}{b.new}`" for b in bumps if b.package.tag and not b.package.rides]
    if tags:
        lines += [
            "",
            f"Once this merges, the Release bump workflow pushes {', '.join(tags)} to publish them; "
            "a C or C++ tag waits until the moq-ffi release `cpp/moq/FFI_VERSION` pins is published.",
        ]
    lines += [
        "",
        "Each bump is a patch, as a backport keeping the published API needs. "
        "The next push to `release` regenerates this branch from scratch, dropping any edit to it.",
        "",
        f"Generated by `just gh bump pr` at {head[:10]}.",
    ]
    return "\n".join(lines) + "\n"


def gh(*args: str, stdin: str | None = None) -> str:
    return subprocess.run(["gh", *args], input=stdin, capture_output=True, text=True, check=True).stdout.strip()


def open_pr() -> str | None:
    number = gh("pr", "list", "--repo", REPO, "--base", BASE, "--head", BRANCH, "--state", "open",
                "--json", "number", "--jq", ".[0].number // empty")
    return number or None


def pr(root: Path) -> None:
    head = git("rev-parse", "HEAD").stdout.strip()
    bumps = plan(head)
    existing = open_pr()
    if not bumps:
        if existing:
            gh("pr", "close", existing, "--repo", REPO, "--delete-branch",
               "--comment", f"Every hand-versioned package on `release` is bumped as of {head[:10]}.")
            print(f"closed #{existing}: nothing left to bump")
        else:
            print("nothing to bump")
        return

    apply(bumps, root)
    names = ", ".join(f"{b.package.name} {b.new}" for b in bumps)
    git("checkout", "-B", BRANCH)
    git("commit", "--all", "--message", f"{TITLE}\n\n{names}")
    # The branch is regenerated from `release` on every run, like release-plz's.
    git("push", "--force", "origin", f"HEAD:refs/heads/{BRANCH}")
    text = body(bumps, head)
    if existing:
        gh("pr", "edit", existing, "--repo", REPO, "--title", TITLE, "--body-file", "-", stdin=text)
        print(f"updated #{existing}: {names}")
    else:
        url = gh("pr", "create", "--repo", REPO, "--base", BASE, "--head", BRANCH, "--title", TITLE,
                 "--body-file", "-", stdin=text)
        print(f"opened {url}: {names}")


def push_tags() -> None:
    head = git("rev-parse", "HEAD").stdout.strip()
    tags = pending(head)
    for tag in tags:
        if tag.blocker:
            # The next push to `release` retries it.
            print(f"waiting to push {tag.name}: {tag.blocker}")
            continue
        git("push", "origin", f"{head}:refs/tags/{tag.name}")
        print(f"pushed {tag.name} at {head[:10]}")
    if not tags:
        print("nothing to tag")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check")
    check.add_argument("--ref", default="HEAD")
    sub.add_parser("apply")
    sub.add_parser("pr")
    sub.add_parser("tag")
    args = parser.parse_args()

    root = Path(git("rev-parse", "--show-toplevel").stdout.strip())
    if args.command == "check":
        bumps = plan(args.ref)
        for bump in bumps:
            reason = f"releases with {bump.alongside}" if bump.alongside else f"{len(bump.changed)} files"
            print(f"{bump.package.name}: {bump.old} -> {bump.new} ({reason} since {bump.since[:10]})")
            for path in bump.changed:
                print(f"  {path}")
        for tag in pending(args.ref):
            print(f"{tag.package.name}: tag {tag.name}" + (f" (waiting: {tag.blocker})" if tag.blocker else ""))
        if not bumps:
            print("nothing to bump")
    elif args.command == "apply":
        apply(plan("HEAD"), root)
    elif args.command == "tag":
        push_tags()
    else:
        pr(root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
