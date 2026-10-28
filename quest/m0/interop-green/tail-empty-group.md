# [M] Draft-18 tail ends cleanly

## Goal

In `just test interop --tail --versions ietf`, a Rust tail subscriber
joining a JS-native publisher (node and bun) over moq-transport-18 receives
every group with its frames and a clean end, as it does over draft 22.

## Plan

Interop run [40517499786](https://github.com/moq-dev/moq/actions/runs/40517499786)
(head [38bc872f](https://github.com/moq-dev/moq/commit/38bc872f1dec5ece74e07f43b508443d8a300378))
fails `tail js-native-node -> rust` and `tail js-native-bun -> rust` with
`Error: empty group` from `test/interop/clients/rust/tail.rs`; the publisher
is `test/interop/clients/js-native/tail.ts` under `run_tail_pair` in
`test/interop/interop.sh`. Draft 22 passes the same cell, and draft 18
passed at [8810d736](https://github.com/moq-dev/moq/commit/8810d736518185eb813dc8758f8f95f31d50cc9e),
so it is a deterministic regression between the two.

Reproduce on current main first:
[3079d472](https://github.com/moq-dev/moq/commit/3079d47269727b3f03d96f3b9cad4737c7696cd8)
changed how a group reported dropped is finalized and may already fix it. If
it still fails, bisect the range and fix at the source, whether that is the
JS IETF publisher (`js/net/src/ietf/publisher.ts`) or the Rust IETF
subscriber (`rs/moq-net/src/ietf/`); never relax the tail check. If it
passes, delete this quest with the passing run as evidence.

Validation checkpoint, 2026-10-28, at
[4018a321](https://github.com/moq-dev/moq/commit/4018a32198860c05f111928e4c4116164bde2c0e):
the current-main reproduction waited for host capacity in `quest gate` and
was stopped before any build or tail cell ran. No passing result or fix is
claimed. Since the failing run, main also gained JS cached and joining FETCH
support in [moq-dev/moq#6945](https://github.com/moq-dev/moq/pull/6945), so
reproduce before changing either implementation. Resume with
`just test interop --tail --publishers rust,js-native-node,js-native-bun
--subscribers rust,js-native-node,js-native-bun --versions ietf` through
`quest gate`. The tail harness selects both native JS runtimes from these
axes and also runs its Rust control lanes; it has no per-direction tail
filter.

Public API: none expected. Wire: none expected.
