# [S] Resolve the remaining external Smoke interoperability failures

## Goal

The Smoke workflow passes on main, with every red cell fixed at its cause.
Smoke clients continue to use the interfaces their installed artifacts expose.

## Plan

The Go, C, and Cloudflare clients call their installed interfaces
([moq-dev/moq#6981](https://github.com/moq-dev/moq/pull/6981)), and
`freshness.sh` enforces Go `@latest` and the newest `libmoq`. The Cloudflare
probe holds its track reader, so a probe that races PUBLISH_NAMESPACE
retries instead of failing `done`.

What remains is the Upstream Docker interoperability job. The
[Smoke run](https://github.com/moq-dev/moq/actions/runs/40657717906) at
`e6ed0ee427` fails 24 of 38 pairs. Each is classified, and none needs a
protocol change in this repo:

- Drafts 14 and 16 (8 pairs): the registry still declares 14 to 22 for
  both moq-dev entries; this checkout's relay refuses 14 to 17.
  [englishm/moq-interop-runner#178](https://github.com/englishm/moq-interop-runner/pull/178)
  narrows them to 18 and 22, which drops these pairs.
- The JS client image still runs `@moq/net` 0.4.4, since the runner has not
  republished it since #177 merged: NAMESPACE_DONE against moq-rs-draft-18,
  quiche-moq, quic-zig, and moq-go
  ([JS namespace lifecycle](/quest/m1/interop-coverage/js-namespace-done.md)),
  imquic `data-subgroup-basic`
  ([JS subgroup data through imquic](/quest/m1/interop-coverage/js-imquic-objects.md)),
  and `moqt://` against moqlivemock and xquic-draft-18. JS has no raw QUIC;
  the runner client bump dials WebTransport on that port instead, which
  helps only where the relay serves it.
- The Rust client image's quic-zig `subscribe-namespace-lifecycle` passes
  on `main` and waits for the
  [runner client bump](/quest/m2/release/runner-client-bump.md).
- Ours, without a quest: moq-go's webtransport-go requires
  SETTINGS_WT_ENABLED (0x2c7cf000) and sends `:protocol webtransport-h3`
  (WebTransport draft 15). `web-transport-proto` 0.6.3 sends neither and
  refuses the token, so every moq-go case against our relay fails.
- Theirs: Cloudflare draft-18 `data-subgroup-basic`
  ([cloudflare/moq-rs#281](https://github.com/cloudflare/moq-rs/issues/281),
  [cloudflare/moq-rs#282](https://github.com/cloudflare/moq-rs/issues/282)),
  and Moxygen's and moqx's `announce-subscribe` (filter type 250,
  [Moxygen minimal interop](/quest/m1/smoke-moxygen.md)).
- Unsupported by design: PUBLISH and SUBSCRIBE_TRACKS cases (Cloudflare
  draft-18, aiomoqt), raw-QUIC-only clients (moqtopus, xquic-draft-18),
  which the runner's docker mode always dials at `https://relay:4443`, and
  xquic-draft-18's relay without namespace discovery.
- Not yet classified: the JS client's imquic `announce-subscribe`, owned by
  the [interop audit](/quest/m2/audit-interop.md).

The job fails on any red pair, so the unsupported pairs keep Smoke red even
after every fix ships. Open decision: gate the job only on the pairs whose
client and relay are both moq-dev, as `relays.sh` gates only on moq-dev's
relay, or check in an expected-failure list per case, each entry linking
its owner, that fails on any new red cell.
Do not weaken a lane or retry media exchange to make it pass.

Public API: none. Wire: none.
