# [S] Merge and publish the frame-start refusal backport

## Goal

[moq-dev/moq#6967](https://github.com/moq-dev/moq/pull/6967) merges to
`release`, and a patch of `@moq/net` containing it publishes. The published
reader refuses a lite group stream starting at a nonzero frame rather than
silently numbering its frames from zero.

## Plan

The release implementation above ports the refusal from
[moq-dev/moq#6395](https://github.com/moq-dev/moq/pull/6395), with a regression
that verifies refusal and that no misnumbered group reaches the consumer.
Both strict release CI gates pass.

Decided 2026-10-27 (maintainer): PR creation only in this session; merging
and publication remain for a later authorized session. No package version
was bumped. Delete this quest and its links after the release PR merges and
its patch publishes.

The Rust changes from
[moq-dev/moq#6922](https://github.com/moq-dev/moq/pull/6922) depend on main's
newer group model and remain excluded from this JS-only backport.

Public API: none. Wire: none; refuses a stream it misread before.
