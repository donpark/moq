# [S] Refill runs

## Goal

A reader behind a hole far longer than the pool refills it without fetching
much more than the hole.

## Plan

See the [epic](/quest/m1/unbounded-groups/README.md) for the decisions.

A refill asks for the whole hole, unpaced (decided 2026-10-24, maintainer).
Pressure sheds the refill's unread tail last-first so its reader keeps the
part it reaches first, and the reader refetches the rest on the next run.
`group_refill/1/1024` in `rs/moq-net/benches/group.rs` costs about 2.7x per
frame what `group_refill/1/64` does.

- Measure the refetch over a real relay hop with a hole several times the
  pool.
- If it matters, bound each ask to a run the pool can hold, derived from the
  pool rather than a fixed constant, and keep the bench showing the slope.

Public API: none. Wire: none.
