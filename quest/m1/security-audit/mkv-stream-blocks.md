# [M] MKV import streams blocks

## Goal

The MKV importer streams a SimpleBlock or BlockGroup payload into its frame
as bytes arrive instead of buffering the whole tag, so a tag declaring
gigabytes holds memory only as fast as the output accepts it.

## Plan

From the 2026-10-27 audit (MUX-3). `rs/moq-mux/src/container/mkv/import.rs`
never sets ebml-iterable's tag limit, whose default is about 4 GB, and
SimpleBlock and BlockGroup are buffered whole.

The audit's fix (lower `max_allowable_tag_size`) is wrong: the library applies
it to every sized tag, including the Segment and Cluster masters, so it would
refuse ordinary files larger than the limit.

Decided 2026-10-27 (maintainer): stream the block payload, the same principle
as [fMP4 import streams mdat](/quest/m1/security-audit/fmp4-stream-mdat.md),
rather than a size pre-check. A pre-check would compare against the frame cap
that [unbounded groups](/quest/m1/unbounded-groups/README.md) removes, after
which a large block is a legal large frame. This needs a small block reader
beside ebml-iterable for the block header and payload; prefer an upstream
ebml-iterable option if one fits. A test feeds a huge declared block in
chunks and asserts held bytes stay bounded.

Public API: none expected. Wire: none.
