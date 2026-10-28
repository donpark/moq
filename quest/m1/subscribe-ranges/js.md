# [L] JS ranges

## Goal

`@moq/net` matches Rust: `Subscription` ranges take `Time(pts)` anchors too,
`fetchGroup` becomes a one-range subscription or is removed, and a JavaScript
publisher serves requested ranges that are no longer in its live cache.

## Plan

Landed in [moq-dev/moq#6949](https://github.com/moq-dev/moq/pull/6949):
`Subscription.ranges` of `absolute` and `relative` anchors in place of
`groups`, a per-reader window (`js/net/src/window.ts`), the union aggregate,
lite-07 SUBSCRIBE carrying the list (older wires the covering span, refusing
other relative anchors per reader), `maxDelay: null`, and the lite-07 publisher
FINning a covered bounded subscription without SUBSCRIBE_END.

What remains:

- **Time anchors.** Mirror Rust's `Anchor::Time(pts)`: resolved from the
  cache's group starts (the time-end settle walk in Rust's `window.rs`),
  refused on an untimed track with `TimestampMismatch`, sent upstream on
  lite-07, and a datagram judged by its own timestamp. Today a JS publisher
  decoding a time anchor serves from the oldest group or through the live edge.
- **Reopen uncovered demand.** When a lite-07 FIN leaves the demand uncovered
  because an update moved the ranges, the JS subscriber fails the track
  (`#settleTail`). Reopen that demand on the same track instead, as the Rust
  copy does.
- **On-demand request surface.** Decided in the 2026-10-05 audit: the producer
  half of [JavaScript FETCH](/quest/m1/js-fetch.md) folds in here, so
  `@moq/net`'s on-demand request surface takes range requests from the start
  instead of the per-group `requested_group` shape Rust replaced with
  `requested_fill`. IETF FETCH dispatch onto it is
  [on-demand JavaScript FETCH](/quest/m1/js-fetch-on-demand.md).
  - Expose an owned request handle rather than a storage callback; dropping a
    request refuses it rather than leaving the subscriber waiting. Preserve
    group sequence, frame boundaries, payload bytes, and track properties.
    Storage, retention, and media catalog interpretation stay outside `js/net`.
  - One logical dynamic track per broadcast and name: a cache-miss request
    never creates a duplicate live producer.
  - Verify with an in-memory responder, without OPFS or an archive writer: a
    browser publisher serves a native subscriber after a group is evicted or
    was never cached, covering exact replay, empty and missing groups,
    concurrent requests, and cancellation while awaiting a reply.
- **`fetchGroup`.** Becomes a one-range subscription on the request surface
  above, or is removed.

Ranges stay whole groups ([moq-dev/moq#6465](https://github.com/moq-dev/moq/pull/6465)),
so the group model keeps refusing a nonzero lite `frameStart` as a protocol
violation, as `lite/subscriber.ts` `runGroup` does. Preserve the per-reader
group-bound rule for reliable reads, live-edge selection, subscription
updates, and publisher datagrams.

## Related

- [Lite-07 ranges](/quest/m1/subscribe-ranges/lite.md) - the Rust lite-07 range fill this mirrors
- [Remove FETCH from lite-07](/quest/m1/subscribe-ranges/lite07-fetch.md) - follows this and Lite-07 ranges
