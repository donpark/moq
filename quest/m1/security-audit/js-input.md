# [S] Browser input checks

## Goal

`@moq/room` refuses `hang/user.json` and `hang/preview.json` values that do
not match their schema instead of casting them, and the `http://` certificate
fingerprint fetch in `@moq/net` is aborted when the connection is cancelled
and accepts only a 64-hex-character SHA-256 fingerprint.

## Plan

From the 2026-10-27 audit (JS-4, JS-2). `js/room/src/metadata.ts` casts parsed
JSON to `User`/`Preview`; a `null` throws at `value.id` and ends that update
loop, and wrong-typed fields reach the UI. Use zod schemas through the JSON
stream decoder, which already takes one (`@moq/json`), so a malformed value is
refused loudly.

`js/net/src/connection/connect.ts` fetches the fingerprint with no abort
signal (a TODO marks it) and reads an unbounded `.text()`. Wire the
connection's cancel signal into the fetch and refuse any body that is not
exactly 64 hex characters.

Decided 2026-10-27 (maintainer): keep the `http://` fingerprint path for any
host, not just loopback (JS-3), so LAN device testing keeps working; its
on-path risk is the documented dev-only tradeoff.

Public API: none expected. Wire: none.
