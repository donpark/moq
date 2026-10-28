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

The full gated remeasurement completed on 2026-10-28: 89 session cells ran
and 32 were skipped for drafts without FETCH. All 16 lite-07-wip media,
publication, and FETCH cells failed, and its capability cell passed. Token
and catalog/container comparisons passed for all four package sources.

The stable controls do not pass, so the pins remain unchanged. Both
same-source Rust media controls and all 24 stable cross-version Rust media
cells fail the Opus tone's timestamp check by 321 microseconds. Removing
only an Opus source's MP4 edit list reproduces that exact phase error;
preserving it passes. The current fMP4 exporter keeps `dOps.pre_skip` but
strips the edit list that places the first kept sample. Do not count this
as a lite-07-wip break or normalize decoded timestamps to hide it.

The run also exposed four stable JS publication failures: the released
`Group.Producer` completes with `close()`, while the checkout uses
`finish()`. The shared test client selects the supported clean-completion
method once at startup and refuses an unknown shape. This adapts the test
driver to the published API; it changes no library API or wire behavior.
All eight stable lite-05/lite-06 publication combinations pass with that
adapter; all four lite-07-wip publication combinations still fail with
framing errors. Remeasurement now refuses to refresh accepted pins when
stable cells fail.

The fixed export also exposed a decoder-command error: FFmpeg normally
shifts its negative Opus priming start to zero, moving the retained PCM
timestamps by the pre-skip again. The decode command now uses `-copyts` to
preserve demuxed PTS. With the exact output from the independent
[Opus export fix](https://github.com/moq-dev/moq/pull/7118), the unchanged
tone check passes at +3/+1 microseconds; removing only its edit list fails
at 321 microseconds even with `-copyts`. This preserves the assertion.

With that decoder command, the actual fixed/current lite-06 control and
its swapped-tone negative control pass. The exact released/released
0.14.9/0.17.9 control still fails, now with a measured phase error of
781 microseconds. Its complete cause remains unverified; do not assume the
current exporter fix is the only change a released build needs.

After a corrected CLI ships, resolve releases again, rerun its stable
controls and the full gated measurement, and refresh both the pins and the
dated reason only when the stable cells pass. Maintainer decision:
investigate and schedule a corrected patch release, or leave this
measurement blocked until the next release cut.

Public API: none. Wire: none.
