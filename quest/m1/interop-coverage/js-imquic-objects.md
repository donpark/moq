# [XS] The runner's JS client writes data stream headers in one write

## Goal

Condition: [englishm/moq-interop-runner#179](https://github.com/englishm/moq-interop-runner/pull/179)
merges and the runner republishes the moq-dev-js client image. That PR
bumps the moq-dev-js client's `@moq/net` from 0.4.6 to 0.4.7, which
carries [moq-dev/moq#6769](https://github.com/moq-dev/moq/pull/6769) and
writes each data stream header in one write, so `data-subgroup-basic`
passes against imquic (draft-22, WebTransport). Check it with
`gh pr view 179 -R englishm/moq-interop-runner --json state`, then
confirm that the `ghcr.io/englishm/moq-interop-runner-moq-dev-js-client:latest`
image is newer than 2026-10-22 and that the next runner nightly shows the
cell green. Delete this quest once #179 merges.

If imquic fixes [meetecho/imquic#43](https://github.com/meetecho/imquic/issues/43)
first and the runner rebuilds its imquic image, the cell turns green even
without the bump. Delete the quest then too.

## Plan

imquic drops a SUBGROUP_HEADER or FETCH_HEADER that arrives across more
than one stream read. `@moq/net` 0.4.6 and earlier write the header one
field per write, which the polyfill sends as separate frames. On
2026-10-28, against the runner's imquic relay image, a client built from
#179 passed `data-subgroup-basic` in 3 of 3 runs. The same client on 0.4.6
failed all 3.

Don't open a competing runner PR. If #179 stalls, nudge it there. The
image rebuild was requested on
[englishm/moq-interop-runner#177](https://github.com/englishm/moq-interop-runner/pull/177).
`announce-subscribe` against imquic times out on both versions and belongs
to the [Interop audit](/quest/m2/audit-interop.md).

The runner's client has to stay on the 0.4 line. `main`'s announced
request watches the tuple's exact path, and moq-transport relays answer
that only with namespaces beneath it, so the client never discovers the
shorter routing prefix the publisher claimed.
[Runner client bump](/quest/m2/release/runner-client-bump.md) records this
and moves the client to `main`'s cut.
