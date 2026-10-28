# [S] QMux datagrams obey or refuse their policy

## Goal

`@moq/qmux` `Session.datagrams` is typed as the browser's
`WebTransportDatagramDuplexStream` and nothing more. Its policy fields either
take effect or refuse loudly. `incomingHighWaterMark` bounds the inbound queue
whenever it is set. Setting `incomingMaxAge` or `outgoingMaxAge` to anything
other than `null`, or changing `outgoingHighWaterMark`, throws. Applications can
no longer construct a `Datagrams`, inject inbound packets, overwrite the
negotiated size limit, or close the receive side.

Non-goals: implementing datagram expiry or an outbound queue, and the Rust
qmux crate.

## Plan

Decided 2026-10-27 from the release-to-main API audit (maintainer picked each
option):

- Today ([js/qmux/src/session.ts](/js/qmux/src/session.ts) `Datagrams`):
  `incomingHighWaterMark` is copied into the readable's `highWaterMark` at
  construction, so a later assignment does nothing. The audit reproduced
  `incomingHighWaterMark = 1` still queueing two packets. The other three
  fields are never read. No consumer in this repository or moq.pro sets any
  of them, and nothing outside qmux imports `Datagrams`, `push`, or
  `setMaxDatagramSize`.
- **Live high-water mark, refuse the rest** (chosen over implementing all
  four, which adds an age-stamped inbound queue and an outbound queue with no
  consumer, and over refusing all four, which drops a cheap live-media
  control). `push` drops once the queue length reaches the current
  `incomingHighWaterMark`. Count the queue yourself instead of relying on the
  stream's fixed `desiredSize`. Each of the other three becomes an accessor
  that throws `NotSupportedError` for a non-default value and accepts the
  default, so code written for the native interface that never changes them
  keeps working.
- **The DOM type is the public view** (chosen over a new qmux-owned
  interface). Stop exporting `Datagrams` from
  [js/qmux/src/index.ts](/js/qmux/src/index.ts). Declare `Session.datagrams`
  as `WebTransportDatagramDuplexStream` and keep the class in a private field
  that the session's frame handling and parameter negotiation use.
- Tests: a lowered high-water mark sheds the overflow, a raised one admits
  more, reads drain and cancel the readable cleanly after a change, and each
  refused setter throws while its default is accepted.

Public API: breaking in `@moq/qmux` (the `Datagrams` export and its mutators
go, and three setters now throw). The Impact bullet says to use
`session.datagrams` through the WebTransport interface. Wire: none.
