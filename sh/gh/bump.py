#!/usr/bin/env python3
"""Patch-bump hand-versioned packages whose sources changed since they last shipped.

release-plz bumps Rust crates on `release`; this does the same for the packages
whose version a human owns and whose release workflow publishes on a push to
`release` once the registry lacks that version (JS, Python, Swift, Kotlin).
Without a bump, a backport touching only one of them lands unchanged in version
and its release skips it as already published.

A package last shipped at the oldest first-parent commit on `release` that
carries its current version: each first-parent commit is a push its release
workflow saw. Any change to its sources between that commit and HEAD is
unreleased, so the package needs a patch bump.

Usage:
  bump.py check [--ref <rev>]   List the packages that need a bump.
  bump.py apply                 Bump them in the working tree.
  bump.py pr                    Bump them on the `release-bump` branch and open
                                or update its PR into `release`; close it when
                                nothing is left. Needs GH_TOKEN.
"""

import argparse
from dataclasses import dataclass, field
import json
from pathlib import Path
import re
import subprocess
import sys

BASE = "release"
BRANCH = "release-bump"
TITLE = "chore: patch-bump hand-versioned packages"
REPO = "moq-dev/moq"


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
    """Every hand-versioned package that publishes on a push to `release`.

    Go derives its patch in CI, and Dart, C, C++, and OBS release from a tag a
    human pushes, so a version bump alone would not publish any of them.
    """
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
    return found


def shipped(pkg: Package, head: str) -> str | None:
    """The oldest first-parent commit since which `pkg` has had its current version."""
    current = pkg.version(head)
    if current is None:
        raise ValueError(f"no version in {pkg.manifest.path} at {head}")
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


def plan(head: str) -> list[Bump]:
    bumps = []
    for pkg in packages(head):
        since = shipped(pkg, head)
        if since is None:
            continue
        changed = git("diff", "--name-only", since, head, "--", *pkg.paths, *UNSHIPPED).stdout.split()
        if changed:
            old = pkg.version(head)
            bumps.append(Bump(pkg, old, patch(old), since, changed))
    return bumps


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
        lines.append(f"| {bump.package.name} | {bump.old} → {bump.new} | {compare} |")
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check")
    check.add_argument("--ref", default="HEAD")
    sub.add_parser("apply")
    sub.add_parser("pr")
    args = parser.parse_args()

    root = Path(git("rev-parse", "--show-toplevel").stdout.strip())
    if args.command == "check":
        bumps = plan(args.ref)
        for bump in bumps:
            print(f"{bump.package.name}: {bump.old} -> {bump.new} ({len(bump.changed)} files since {bump.since[:10]})")
            for path in bump.changed:
                print(f"  {path}")
        if not bumps:
            print("nothing to bump")
    elif args.command == "apply":
        apply(plan("HEAD"), root)
    else:
        pr(root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
