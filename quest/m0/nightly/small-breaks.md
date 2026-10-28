# [S] Three mechanical scheduled-run breaks

## Goal

Nightly's Features and drill-sensitivity steps and TS's MPEG-2 video step get
past their mechanical failures and run their real checks.

## Plan

All found in the 2026-10-27 scheduled runs (Nightly [40506938673](https://github.com/moq-dev/moq/actions/runs/40506938673) at 22808bbb2f, TS
[40515802751](https://github.com/moq-dev/moq/actions/runs/40515802751)):

- Features: `-D dead-code` rejects `Incoming::refuse`
  (`rs/moq-tokio/src/quic/driver/incoming.rs:30`), which only the server and
  tests call, so the driver-only feature build sees it unused. Gate it on the
  feature that uses it, rather than allowing dead code.
- drill-sensitivity: `test/drill/mutations/fetch-never-sent.patch` no
  longer compiles (E0425 `request`), so `test/drill/sensitivity.sh` reports
  that it "broke the build". Re-anchor the patch to the current code so it
  still breaks the behavior the drill expects to catch.
- TS: `.github/workflows/ts.yml`'s MPEG-2 step sets `MOQ_TEST_RUNS`, which
  the relay refuses as an undeclared `MOQ_*` variable since
  `check_env`. Rename it to the variable the harness reads
  (`HARNESS_RUNS`), and grep the workflows for any other stale `MOQ_*`.

Public API: none. Wire: none.
