# [S] The JS audio encoder re-enables without a probe

## Goal

Re-enabling a disabled `Audio.Encoder` republishes the catalog as enabled and
encodes the first captured frame at once. There is no new probe encoder, and
no `reset()` delay.

## Plan

Traced 2026-10-28 from [#7101](https://github.com/moq-dev/moq/issues/7101):
`enabled = true` builds a new probe `AudioEncoder` (encode and flush,
`js/publish/src/audio/encoder.ts:457-535`). The catalog stays disabled until
the probe finishes (`:545-569`), and skipped samples force `encoder.reset()`
(`:699-702`). Together that costs ~10-30 ms plus up to a frame.

Decided 2026-10-28 (maintainer): keep the probe result, and the encoder,
across a disable while the capture format and settings are unchanged. A
changed format or settings probes again, as today.

Tests: a disable/enable cycle with unchanged capture flips the catalog in the
same tick and encodes the first frame without a reset. A changed sample rate
probes again.

Public API: none. Wire: none.
