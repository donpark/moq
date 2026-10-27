# [S] Probing beside Cubic meets the deadline budget

## Goal

`tests::probe::competing_cubic` in `rs/moq-quic/src/tests/probe.rs`, run by
Nightly through `just rs quic-sim`, passes its deadline budget on `main`,
fixed at its cause.

## Plan

Over seeds 1 through 8, probing beside a Cubic upload averages 413.6 late
frames versus 385.1 for the baseline, beyond the `LATE` budget. Find why
probing adds delay beside Cubic and fix it in the transport. Keep the
overhead, fairness, and rise budgets and the other probe scenarios passing.
Do not relax the assertions or disable probing to make the scenario pass.

Run the probe suite with release debug assertions and overflow checks, as
`just rs quic-sim` does. `just rs test --release -p moq-quic --run-ignored all
--no-fail-fast -E 'test(/^tests::probe::/)'` also includes the short gate cases.

### Reproduction and remaining work

Confirmed on `ee5ac743710677763c6e8a4bfb2d18093e576ba2` with the release
checks above: the eight-seed mean is 413.625 late frames versus 385.125,
and p99 is 1180.544 ms versus 997.987 ms. Both deadline and p99 budgets fail.
The largest regressions are seed 3 (691 versus 369 late) and seed 6 (510
versus 244); seed 7 has the other long p99 tail. Trace the probe, model
aging, and ProbeRTT transitions on those seeds before changing the sender.
A transport root cause is not established yet.

Queue-delay gates on redundant probes were explored and discarded, with
all assertions unchanged:

- Gating the latest RTT at 1.25 times min RTT passes Cubic (383.750 late)
  but violates the overhead budget in four other scenarios, up to 18.58%.
- Returning a zero inflight allowance instead of suspending the probe keeps
  feedback congestion-limited, but worsens Cubic to 453.250 late frames.
- Allowing the peer's maximum ACK delay passes the other scenarios, but
  Cubic still has 435.000 late frames.
- Gating on a complete round's minimum RTT instead avoids reacting to an
  individual delayed ACK and passes the other scenarios, but Cubic still
  has 394.750 late frames, beyond 389.625 allowed.

No production change from those experiments is retained. Do not tune an RTT
threshold to these seeds; the next step is to establish which controller
transition causes the extra delay and fix that mechanism.

Public API: none. Wire: none.
