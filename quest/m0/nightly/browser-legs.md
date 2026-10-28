# [M] Nightly browser legs pass

## Goal

Nightly's browser publisher fairness benchmark, browser backend baseline, and
Firefox player test pass.

## Plan

Three unowned failures in Nightly [40506938673](https://github.com/moq-dev/moq/actions/runs/40506938673) (2026-10-27, at 22808bbb2f), possibly
one cause:

- The Audio quality job's "Browser publisher fairness benchmark" step
  (`js/net/bench/serve-browser-entry.ts`) fails with "only 0/1024 streams".
- The browser backend baseline (`wasm-baseline`) fails with "missing WASM
  frame 0".
- The Media job's Firefox player fails on the console error "A VideoFrame
  was garbage collected without being closed".
  [Firefox publishing](/quest/m2/publish-firefox.md) covers only the
  publisher side.

Triage together first: if the first two share a cause, fix it once. If the
causes differ and one fix is large, split it into its own child of this epic.
The unclosed `VideoFrame` is a leak in the player, not a console-noise
filter to add.

Public API: none. Wire: none.
