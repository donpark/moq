# [XS] Remeasure the planned wire break

## Goal

`just test wire-compat` runs again: the lite-07-wip planned break in
`test/interop/compat/planned-breaks.json` is measured against the released
packages currently selected by the compatibility resolver.

## Plan

npm published `@moq/net` 0.4.6 on 2026-10-23, so `stale()` in
`test/interop/compat/resolve.ts` refuses every scheduled run since. Run
`just test wire-compat --remeasure`, confirm the lite-07-wip cells still
fail against the released builds and the stable lite-01 through lite-06
cells still pass, and update the entry's releases and reason with the date.

Registry resolution on 2026-10-28 selects `moq-cli` 0.14.9, `moq-relay`
0.17.9, and `@moq/net` 0.4.7. The checked-in exception still names 0.14.8,
0.17.8, and 0.4.5 respectively. Calling `stale()` with those resolved
releases reproduces the refusal at the first difference, `moq-cli` 0.14.8
to 0.14.9.

The local `quest gate just test wire-compat --remeasure` attempt never
passed host admission and was stopped before the harness ran. No stable or
planned-break cell was measured, and the pins remain unchanged. Resolve the
releases again when the measurement can run; these versions are evidence of
the stale pin, not replacements accepted without measurement.

Public API: none. Wire: none.
