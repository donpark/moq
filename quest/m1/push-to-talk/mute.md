# [S] A push-to-talk mute keeps the microphone open

## Goal

A push-to-talk publisher can mute and unmute without closing the microphone.
Unmuting pays no getUserMedia, no new AudioContext and no worklet load.

## Plan

Traced 2026-10-28 from [#7101](https://github.com/moq-dev/moq/issues/7101):
`<moq-publish muted>` sets `audioEnabled = enabled && !muted`
(`js/publish/src/element.ts:204`). That turns off Capture, which tears down
the AudioContext and worklet (`capture.ts:177`) and stops the track
(`microphone.ts:61`). Unmuting then pays 50-300+ ms on real hardware before
any frame exists. Toggling `Audio.Encoder.enabled` leaves capture running.

Decided 2026-10-28 (maintainer): provide and document a mute that disables
the encoder but keeps capture. The browser's microphone indicator stays lit
while muted, and the docs say so.

Open for the maintainer when this starts: the shape. Recommended: document
the library path (`Audio.Encoder.enabled` with capture left on) in
`doc/lib/js/publish.md`, and give the element one attribute that selects it,
not a second mute flag.

Tests: muting this way keeps the capture's track live, and unmuting encodes
without a new getUserMedia.

Public API: additive (one element attribute, if chosen). Wire: none.
