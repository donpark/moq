import { expect, spyOn, test } from "bun:test";
import { SessionCode } from "../error.ts";
import { createMockTransportPair } from "../mock.ts";
import * as Path from "../path.ts";
import { Stream } from "../stream.ts";
import { NativeSession } from "./adapter.ts";
import { Connection } from "./connection.ts";
import { PublishNamespace } from "./publish_namespace.ts";
import { Subscribe, SubscribeOk } from "./subscribe.ts";
import { Subscriber } from "./subscriber.ts";
import { ALPN, Version } from "./version.ts";

const versions = [
	[Version.DRAFT_18, ALPN.DRAFT_18],
	[Version.DRAFT_22, ALPN.DRAFT_22],
] as const;

// Both connection drivers run, while the client can inject a malformed peer stream.
for (const [version, alpn] of versions) {
	test.each(["update", "unsolicited"] as const)(
		`${alpn}: %s closes the peer session with its violation`,
		async (kind) => {
			const pair = createMockTransportPair(alpn);
			const clientControl = await Stream.open(pair.client, { version });
			const serverControl = await Stream.accept(pair.server, version);
			if (!serverControl) throw new Error("missing control stream");
			const server = new Connection({
				url: new URL("https://example.com"),
				quic: pair.server,
				control: serverControl,
				maxRequestId: 100n,
				version,
				client: false,
				solicit: true,
			});
			const client = new Connection({
				url: new URL("https://example.com"),
				quic: pair.client,
				control: clientControl,
				maxRequestId: 100n,
				version,
				client: true,
				solicit: true,
			});
			const closing = spyOn(pair.server, "close");
			const logged = spyOn(console, "error").mockImplementation(() => {});
			try {
				const stream = await Stream.open(pair.client, { version });
				if (kind === "update") await stream.writer.u62(0x02n);
				else {
					await stream.writer.u53(PublishNamespace.id);
					await new PublishNamespace({ requestId: 0n, trackNamespace: Path.from("unasked") }).encode(
						stream.writer,
						version,
					);
				}
				// The handler either ends the stream or the session before this read settles.
				await stream.reader.u53().catch(() => undefined);
				expect(closing.mock.calls[0]?.[0]?.closeCode).toBe(SessionCode.ProtocolViolation);
				expect((await pair.client.closed).closeCode).toBe(SessionCode.ProtocolViolation);
			} finally {
				client.close();
				server.close();
				closing.mockRestore();
				logged.mockRestore();
			}
		},
	);

	test(`${alpn}: a shared alias for different tracks closes with a violation`, async () => {
		const pair = createMockTransportPair(alpn);
		const subscriber = new Subscriber({
			session: new NativeSession(pair.server, version, true),
			quic: pair.server,
		});
		const broadcast = subscriber.consume(Path.from("room"));
		const tracks = [];
		try {
			for (const name of ["video", "audio"]) {
				const track = broadcast.track(name).subscribe();
				tracks.push(track);
				const peer = await Stream.accept(pair.client, version);
				if (!peer) throw new Error("missing subscription");
				expect(await peer.reader.u53()).toBe(Subscribe.id);
				const request = await Subscribe.decode(peer.reader, version);
				await peer.writer.u53(SubscribeOk.id);
				await new SubscribeOk({ requestId: request.requestId, trackAlias: 7n }).encode(peer.writer, version);
				await track.info();
			}
			expect((await pair.client.closed).closeCode).toBe(SessionCode.ProtocolViolation);
		} finally {
			for (const track of tracks) track.close();
			broadcast.close();
			subscriber.close();
			pair.client.close();
			pair.server.close();
		}
	});
}
