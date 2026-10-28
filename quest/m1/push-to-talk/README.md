# Push-to-talk loses no speech

## Goal

A listener subscribed while a publisher is muted hears the next utterance from
its first frame. Unmuting costs no catalog round trip, no resubscribe and no
microphone reopen. A muted rendition still costs no media bytes.

## Plan

From [#7101](https://github.com/moq-dev/moq/issues/7101), traced 2026-10-28. With
`Audio.Encoder` `enabled: false`, watch deselects the rendition and
unsubscribes. On unmute the listener waits for the catalog flip, then its
SUBSCRIBE reaches the publisher. The demand gate discards everything encoded
before that (`js/publish/src/audio/encoder.ts:686`). At 100 ms RTT that is
three one-way trips plus the probe and a frame (~515 ms to the first fresh
frame). The emptied buffer then refills the 300 ms delay, so the result is
~820-950 ms to audible, with 250-400 ms of speech never encoded.

Decided 2026-10-28 (maintainer):

- **Hold, not DTX.** DTX is for silence the app can't predict. Push-to-talk
  knows when it is silent, and a held subscription costs no media bytes and no
  encoder CPU. DTX is off on main anyway ([#4919](https://github.com/moq-dev/moq/pull/4919)).
- **Consumers hold a disabled rendition**, audio and video, by default, in
  every first-party consumer. The hang spec weakens from "must not select" to
  "should not select" a disabled rendition.
- **The publisher keeps its encoder across a disable**, and push-to-talk gets a
  mute that leaves capture open.
- **Encoding no longer waits for demand**
  ([always-on encoding](/quest/m1/eager-encode/README.md)). A listener who
  subscribes after the unmute still gets the start within its `maxDelay`. This
  replaces a publisher head buffer, which was considered and dropped as the
  same idea special-cased.

The epic's own work: once the children land, rerun the reporter's
measurement (22 presses, 100 ms RTT, 2% loss, 300 ms delay). Unmute → audible
is then the one-way trip plus the delay, with every frame of the utterance
delivered. Record the result in the PR.

## Required

- [JS consumers hold a disabled rendition](/quest/m1/push-to-talk/hold-js.md) - watch keeps a disabled audio or video rendition subscribed, and the hang spec says "should not select"
- [Rust consumers hold a disabled rendition](/quest/m1/push-to-talk/hold-rs.md) - moq-cli play, moq-ffi and moq-mux select follow the same rule
- [Re-enable without a probe](/quest/m1/push-to-talk/reenable.md) - the JS audio encoder flips its catalog and encodes in the same tick as an unmute
- [A mute that keeps the mic](/quest/m1/push-to-talk/mute.md) - push-to-talk mutes without closing capture, so unmute skips getUserMedia and AudioContext setup

## Closes

- [#7101](https://github.com/moq-dev/moq/issues/7101) - a disabled audio rendition is deselected, so the start of the next utterance is lost

## Related

- [Always-on encoding](/quest/m1/eager-encode/README.md) - encoders run while enabled, so a late subscriber can fetch the start
