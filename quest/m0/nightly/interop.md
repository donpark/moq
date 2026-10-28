# [M] Interop passes on main

## Goal

The scheduled Interop workflow (`.github/workflows/interop.yml`) passes on
`main` again, so a red run means a new break. It has failed every night
since 2026-10-22; the last green run was 2026-10-22 11:47.

## Plan

Triage of run
[40517499786](https://github.com/moq-dev/moq/actions/runs/40517499786)
(2026-10-27) and the two before it. Four steps fail:

- **Released wire compatibility**: `stale planned break: moq-lite-07-wip,
  @moq/net changed from 0.4.5 to 0.4.6`. `@moq/net` 0.4.6 published on
  2026-10-23 and nobody re-measured. Run `just test wire-compat --remeasure`,
  review the rewritten `test/interop/compat/planned-breaks.json`, and refresh
  its reason. A patch release between cuts is not covered by
  [the cut](/quest/m2/release/cut.md)'s re-measure step, so also find where
  CONTRIBUTING.md's release procedure should say to re-measure after any
  publish of a pinned package, and fix that inline.
- **Harness**: `test/interop/clients/js/publish-ui.browser.ts:390` still
  sets `moq-transport-14` on a `<moq-watch>`, which
  [moq-transport 18 and 22 only](/quest/m1/ietf-drop/README.md) removed, so
  `checkVersions` refuses it. Update the expected versions.
- **`interop --all`**: the failing cell changes each night (`cpp ->
  js-native-node` tone "timestamp gap", `go -> c`, `rust -> js` past its
  30 s budget, `goaway rust <-> js-native-*`). Route each to its owner:
  FFI-publisher tone gaps belong to
  [FFI publisher audio gaps](/quest/m0/test-flakes-2/ffi-audio-gap.md).
  Fix the rest here at the cause, or plan a quest per cause through
  `/quest-plan` if one is large.
- **Tail**: `tail js-native-* -> rust` belongs to
  [JavaScript FETCH](/quest/m1/js-fetch.md); confirm it still fails only
  for that reason.

Done when a scheduled Interop run passes, or every remaining red cell has
an owning quest in m0.

Public API: none. Wire: none.
