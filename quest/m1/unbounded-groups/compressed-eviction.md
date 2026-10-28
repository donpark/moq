# [M] Compressed front eviction

## Goal

A compressed group sheds its front under pool pressure like a plain group,
instead of holding its whole log under the 32 MiB `MAX_CACHE_BYTES` budget,
and a late plain reader still inflates it. Compressed moq-json and moq-flate
streams then accept appends for their track's life.

## Plan

See the [epic](/quest/m1/unbounded-groups/README.md) for the decisions.

Plain groups already shed: `GroupState::shed` in `model/group.rs` drops whole
frames from a live or read group's front, and a reader at a hole refills it
through `track::Refill` and `Handle::adopt_refill`. A compressed group skips
this: `shed` returns early on `flate`, and `would_overflow` still aborts one
at the budget.

- Keep the prefix-before-inflate contract
  ([compressed tracks](/quest/m1/compress/README.md)). Today
  `GroupState::inflate` refuses a nonzero cached offset. Refill the needed
  compressed prefix before inflation, or retain equivalent decoder state,
  with compressed storage, the shared inflated copy, and codec state all
  bounded by the pool. Never pin a whole growing prefix to make a late plain
  reader work.
- Test a long compressed group through a small pool with a late plain reader
  and legacy-wire egress.
- Then drop the compressed budget from `would_overflow`, moq-json's stream
  encoder, and moq-flate's stream producer, unless the
  [compression migration](/quest/m1/compress/migrate.md) has deleted
  moq-flate first.

Public API: a compressed moq-json or moq-flate stream stops returning
`GroupTooLarge`; a late reader refills or sees `Evicted`. Wire: none.
