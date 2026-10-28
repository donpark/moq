# [XS] The runner's JS client withdraws namespaces by reset

## Goal

Condition: the runner republishes its moq-dev-js client image with the
`@moq/net` 0.4.6 bump from
[englishm/moq-interop-runner#177](https://github.com/englishm/moq-interop-runner/pull/177),
merged 2026-10-26 as `2a7e0dd`. The nightly pulls
`ghcr.io/englishm/moq-interop-runner-moq-dev-js-client:latest` instead of
building it, and only a runner maintainer can dispatch the image build:
"Build & Push Docker Images to GHCR" with `moq-dev-js-client`, or "Rebuild
All Docker Images". Check that the registry's tags list
(`https://ghcr.io/v2/englishm/moq-interop-runner-moq-dev-js-client/tags/list`,
with an anonymous pull token) has a tag newer than `20261022-142031` /
`4ed1383`, which predates the bump. Then confirm `subscribe-namespace-lifecycle`
passes for moq-dev-js against Cloudflare draft-18 (shard `moq-rs-draft-18`)
and aiomoqt (shard `aiomoqt-relay`) in the next runner nightly, and delete
this quest once it does.

## Plan

`@moq/net` 0.4.5 and earlier withdraw a PUBLISH_NAMESPACE on drafts 17+
with FIN plus STOP_SENDING, which draft-18 §3.3.2 does not allow. Both
relays end the namespace only on RESET_STREAM, so the subscriber never sees
NAMESPACE_DONE until the publisher's session closes. 0.4.6 shipped from
`release` with the backport
([moq-dev/moq#6518](https://github.com/moq-dev/moq/pull/6518)), so no moq
release is needed.

The 2026-10-27 and 2026-10-28 nightlies ran on `2a7e0dd` with the old
image, and all six moq-dev-js cells (docker, remote QUIC, remote
WebTransport against both relays) still fail the case. Those failures say
nothing about 0.4.6.

Do not open a competing runner PR. Asking the runner's maintainer on #177 to
dispatch the image build is the way to advance this. #177's author tested
only against moq-go, so if the nightly still fails against Cloudflare
draft-18 or aiomoqt after the rebuild, debug that relay pairing with a
client built from `main`.
