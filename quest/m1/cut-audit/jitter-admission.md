# [M] The export jitter buffer admits only frames due within its window

## Goal

The TS, MKV, and FLV exports' jitter buffer stays bounded by construction:
a track is read only while its newest queued frame is due within the delay
plus its lead, so a track whose timestamps outrun real time is held back in its
source as backpressure instead of queued without bound beside an honest
track.

## Plan

Found by the 2026-10-24 pre-cut audit at 151a529670. Reproduced, new on main
(`jitter.rs` is not on release).

`full()` (`rs/moq-mux/src/jitter.rs:1058`) reports a track full only once
its front frame is due, so future frames are always read and queued;
`bound()` (`:692`) gates only generation decisions; and the drift check
(`:517`) measures the steadiest track. With a 500 ms delay, audio at 1x and
video at 3x, the buffer grew linearly from 500 frames at 20 s to 3,006 at
120 s with no error. A fast track alone does fail, so one bad track stays
unbounded while an honest one sits beside it. Affects `ts/export.rs:442`,
`mkv/export.rs:485`, and the FLV export.

Make admission the bound: a track is full once its newest queued frame (the
back of its queue, or the frame about to be admitted) is due later than now
plus delay plus its lead. Judging the front frame would not work: in the
scenario above the oldest video frame stays near its deadline while newer
frames pile up behind it, and `Stream::fill` keeps reading while
`jitter.full(name)` is false. No frame-count cap. Alternatively charge
a track whose lead keeps growing as a source fault.

Then evaluate the larger simplification: about a dozen fixes to the export
clock (f85547ffd0, 5d5974f083, 3fdf46160b, e84ffb3de3, a1465d0d00,
11bfa30461, 3cf978cdbf, 493d8eee99, 7a3890cd30, 3b77d23015, 7c4c48d08b,
41f3087e3c) each added a rule about which frames count. With admission as
the single invariant, check whether acquisition, the leapt/lagging arms,
and some of the nine tuning constants can go. Land what the measurement
supports; record what it does not.

Investigation in [moq-dev/moq#7012](https://github.com/moq-dev/moq/pull/7012):

- A mocked-clock 120-second feed, with audio at 1x and video at 3x, reaches
  4,938 queued frames under the original admission rule. Established-clock
  admission holds at most 54 frames for its two 20 ms tracks and 500 ms delay,
  including one lookahead frame per track.
- Admission needs each track's lead before the first two-second steering
  window. Seed it from acquisition, and wake the reader when a future tail
  enters its window, before the queued frame itself is due.
- Payload backpressure must not hide received group handles from the source
  cursor's latency budget, nor hide a completely received finite tail from
  TS scheduling. Track receipt separately from delivery, requiring both track
  FIN and each group's FIN before recognizing the existing finite-tail rule.
  Reap never-decoded aborted group handles with amortized doubling compaction;
  their cache charge is cleared on eviction, so retaining their metadata forever
  would merely move the unbounded queue. Keep the current gap and already
  decoded markers and faults. All five original TS integration regressions pass.
- **Acquisition remains unbounded and blocks completion.** Before a clock
  exists, `full` ignores the acquisition's held frames. The new deterministic
  probe offers 256 future frames and retains all 256, past its 500 ms window's
  derived 27-frame bound. The current acquisition chooses the freshest frame
  of the entire available join backlog. Stopping at a bounded prefix changes
  that anchor and can change the measured lead of a track sampled afterward;
  retaining the whole backlog preserves that behavior at an unbounded cost.
  Decide a bounded sampling/ownership design before landing. Recommendation:
  keep this draft while that design is settled alongside
  [Container backpressure](/quest/m1/container-backpressure.md), which already
  owns per-consumer decoded-frame and group-overhead byte budgets in Rust and
  JS. This PR has not expanded into its public configuration design, and does
  not claim a complete hostile-peer memory bound.
- The admission benchmark sweeps 1/4/16/64 tracks against 64/256/1024 source
  frames. At 64 tracks the original fill grows from 0.507 ms to 7.696 ms;
  the iterated candidate remains about 0.25 ms. This measures established-clock
  read work, not the unresolved startup path.

Public API: none. Wire: none.

## Related

- [TS fidelity and recovery](/quest/m1/ts-reliability/README.md) - the import side's state ownership
