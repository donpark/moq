# [M] Head refill

## Goal

The fetched head of a live group that started past frame 0 refills that
group in place, rather than living as a second copy in `Slot.head`.

## Plan

See the [epic](/quest/m1/unbounded-groups/README.md) for the decisions.

Today a live group the feed started past the frames a fetch asks for gets a
separate fetched copy in `Slot.head` (`model/track.rs`), served in the live
group's place while it lasts. Front eviction already refills a group's holes
in place: `insert_group_request` hands the fill a copy that writes into the
cached group through `Handle::adopt_refill`.

- Let a fetch below the live group's `offset` refill there the same way,
  lowering the offset as the run lands, so readers find one copy.
- Delete `Slot.head`, `Slot::serving`, and the `kept` and back-cut special
  cases in `insert_group_request`.
- Coordinate with [moq-transport fill refill](/quest/m2/ietf-fill-refill.md),
  which makes the moq-transport headless group a refill of its leading hole.

Public API: none. Wire: none.
