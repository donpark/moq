# [S] Backport the @moq/flate inflate bounds to release

## Goal

`@moq/flate` on `release` stops inflating a hostile slice once it passes its
cap and stops pinning pako's scratch buffer, as
[moq-dev/moq#6900](https://github.com/moq-dev/moq/pull/6900) fixed on main, and
publishes a patch release (npm has 0.1.0 to 0.1.3).

## Plan

Decided 2026-10-27 (maintainer): one PR against `release` per backport, in m0.

The implementation is ready in [moq-dev/moq#6961](https://github.com/moq-dev/moq/pull/6961),
against `release` from `codex/backport-flate-budget`. It preserves the released
API and passed `just check origin/release`; the 12 codec tests cover stopping
inflation, owned output, and independent per-frame limits.

Remaining: merge that PR once authorized, then publish and verify the patch
release. The maintainer requested PRs only on 2026-10-27, so merging, version
bumps, and publication are intentionally pending. Do not delete this quest
until the release condition in Goal holds.

`release`'s `js/flate/src/index.ts` is the older per-frame version (a 64 MiB
`maxFrameSize`, not main's 32 MiB group budget), so port the two fixes onto its
shape rather than main's budget:

- `onData` only returns past the cap, so pako inflates the rest of the slice.
  Stop inflation at the cap and fail the frame.
- `concat()` passes a single chunk through, pinning pako's scratch buffer.
  Copy it out.

Public API: none. Wire: none.
