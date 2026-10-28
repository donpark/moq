/** Exchange signed groups with Rust: verify the ones Rust signed, and sign through the JS wrappers. */
import assert from "node:assert/strict";
import { Broadcast, Epoch, Path } from "../../js/net/src/index.ts";
import * as Signed from "../../js/sign/src/broadcast.ts";
import { Failure } from "../../js/sign/src/failure.ts";
import { SecretKey } from "../../js/sign/src/key.ts";
import { Scope } from "../../js/sign/src/scope.ts";
import * as Track from "../../js/sign/src/track.ts";

/** One group's frame bodies, signature included, as they cross the wire. */
type Group = { track: string; group: number; bodies: string[] };
type Input = { seed: string; path: string; epoch: string; rust: Group[]; payloads: string[][] };

function unhex(text: string): Uint8Array<ArrayBuffer> {
	return Uint8Array.from(text.match(/../g) ?? [], (byte) => Number.parseInt(byte, 16));
}

function hex(bytes: Uint8Array): string {
	return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

const input = JSON.parse(process.argv[2] ?? "") as Input;
const key = await SecretKey.fromSeed(unhex(input.seed));
const path = Path.from(input.path);
const epoch = Epoch.parse(input.epoch);

// Each Rust group through a verifying JS track consumer: its payloads, or the failure code.
const verified: (string[] | string)[] = [];
for (const { track, group, bodies } of input.rust) {
	const net = new Broadcast.Producer().createTrack(track);
	const consumer = Track.Consumer.make(net.subscribe(), key.public, new Scope(path, epoch, track));
	const writer = net.createGroup(group);
	for (const body of bodies) writer.writeFrame({ payload: unhex(body) });
	writer.finish();
	const received = await consumer.recvGroup();
	assert(received);
	try {
		const payloads: string[] = [];
		for (let frame = await received.readFrame(); frame; frame = await received.readFrame()) {
			payloads.push(hex(frame.payload));
		}
		verified.push(payloads);
	} catch (err) {
		if (!(err instanceof Failure)) throw err;
		verified.push(err.code);
	}
}

// Groups signed through the JS wrappers, tapped raw off the net broadcast.
const inner = new Broadcast.Producer();
const signed = new Signed.Producer(inner, key, path);
const producer = signed.createTrack("js");
const tap = inner.consume().track("js").subscribe();
const js: Group[] = [];
for (const payloads of input.payloads) {
	const group = producer.appendGroup();
	for (const payload of payloads) await group.writeFrame(undefined, unhex(payload));
	group.finish();
	const raw = await tap.recvGroup();
	assert(raw);
	const bodies: string[] = [];
	for (let frame = await raw.readFrame(); frame; frame = await raw.readFrame()) bodies.push(hex(frame.payload));
	js.push({ track: "js", group: raw.sequence, bodies });
}

process.stdout.write(JSON.stringify({ public: key.public.toString(), verified, epoch: signed.epoch, js }));
