# [S] kio's log survives Loom's eviction model

## Goal

`just rs loom` passes `a_log_reader_racing_eviction_never_touches_freed_memory`
in `rs/kio/src/loom.rs`, with the bug it found fixed at its cause, whether
that is the log or the model's instrumentation.

## Plan

Nightly [40506938673](https://github.com/moq-dev/moq/actions/runs/40506938673) (2026-10-27, at 22808bbb2f) panics with "Causality violation:
Concurrent read and write accesses to `UnsafeCell`", then aborts on a panic
in a destructor. The model came with the lock-free log in
[moq-dev/moq#6782](https://github.com/moq-dev/moq/pull/6782). Treat it as a
possible use-after-free in `rs/kio/src/log.rs` until shown otherwise: a
reader's hint into a segment the collector frees is the case the model was
written to catch. Reproduce from the failing interleaving, then decide from
the trace whether the log orders the read before the free. Never loosen the
model or its preemption bound to pass.

Public API: none. Wire: none.

## Related

- [In-memory unsafe runs under Miri](/quest/m2/miri-memory.md) - Miri over the same log
