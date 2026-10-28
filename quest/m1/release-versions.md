# [S] Every package's release backport ships

## Goal

A fix merged into `release` for any hand-versioned package publishes. Today
release-plz bumps only Rust crates, so a backport touching only a JS, Python,
Dart, Kotlin, Swift, C, or C++ package can land on `release` unchanged in
version, and its release workflow skips it as already published. On `main`,
the Release gate's Versions check (`just gh versions check`) now fails such a
backport, and the Release tag workflow pushes the Dart, C, and C++ tags for
versions a human set on `release`. Done when both run on `release` and the
fixes `release` already carries unreleased are bumped there.

## Plan

Found 2026-10-28: the @moq/flate and @moq/net backports
([moq-dev/moq#6961](https://github.com/moq-dev/moq/pull/6961),
[moq-dev/moq#6959](https://github.com/moq-dev/moq/pull/6959),
[moq-dev/moq#6967](https://github.com/moq-dev/moq/pull/6967)) merged and the
release PR merged, but Release JS logged "already published, skipping" and a
separate hand bump was needed.

Decided 2026-10-28 (maintainer), superseding the earlier bump bot: "we can
only use an automated bump tool like this if it understands semver. If it
doesn't, we need manual bumps." Nothing here can judge semver, since
`just api-diff --deny` doesn't cover the hand-written wrappers.

- ✅ Bumps on `release` stay manual; a human picks the level. No workflow or
  script picks or writes a version.
- ✅ A Release gate check fails a backport that changes a hand-versioned
  package's shipped sources without moving its version past the released one,
  or without moving its `bun.lock`/`uv.lock` entry. It names each package and
  how to bump it, requires an OBS bump with each C++ release and the reverse,
  and never edits.
- ✅ A workflow still pushes the missing `moq-dart-v*`, `c-v*`, and `cpp-v*`
  tag for a version a human set on `release`: C and C++ only once the pinned
  moq-ffi release is published, and C++ only once OBS is off its released
  version.

Remaining, on `release` (neither publish nor tag by hand here):

1. Backport the tooling as a tooling-only PR: `sh/gh/versions.py`,
   `sh/gh/versions.test.py`, the `.github/justfile` recipe and test line,
   `.github/workflows/release-tag.yml`, its `alert.yml` entry, and the
   Versions job in `release-gate.yml`. Its own Release gate runs the check.
   Its merge is a push, so Release tag pushes `moq-dart-v0.1.1`, which
   `release` has carried untagged since the drain backport
   ([335e0b5f29](https://github.com/moq-dev/moq/commit/335e0b5f29)); check
   that Release Dart publishes it ([dart-publish](/quest/m2/release/dart-publish.md)).
2. Bump by hand the packages `release` already carries unreleased changes for
   (`just gh versions check --base origin/release --head origin/release` lists
   them):
   - @moq/hang 0.5.4, from
     [moq-dev/moq#6631](https://github.com/moq-dev/moq/pull/6631).
   - @moq/publish 0.5.3, from
     [moq-dev/moq#6598](https://github.com/moq-dev/moq/pull/6598).
   - @moq/boy 0.4.1, only if swapping its esbuild devDependency for rolldown
     ([moq-dev/moq#5354](https://github.com/moq-dev/moq/pull/5354)) changed
     what it ships.

   It also warns for a dozen JS packages over `js/common/package.ts`; that
   change ([e44d51fecd](https://github.com/moq-dev/moq/commit/e44d51fecd))
   only added CI API-report checks, so they need no release.

Public API: none. Wire: none.
