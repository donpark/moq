#!/usr/bin/env bun
/**
 * moq-sign-00 primitives and known-answer vectors.
 *
 * `bun drafts/moq-sign-00.ts` verifies drafts/moq-sign-00.json against this
 * file. `bun drafts/moq-sign-00.ts --write` regenerates the JSON.
 *
 * This is the language-neutral contract, not the TypeScript signing core: it
 * does not wrap tracks or groups.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const PROFILE = "moq-sign-00";
export const LABEL = utf8(PROFILE);
export const KEY_LEN = 32;
export const SIGNATURE_LEN = 64;

// The RFC 8032 PKCS #8 wrapper for a raw Ed25519 seed.
const PKCS8_PREFIX = hex("302e020100300506032b657004220420");
const ED25519 = { name: "Ed25519" } as const;

export class ProfileError extends Error {
	readonly code: string;
	constructor(code: string) {
		super(code);
		this.code = code;
	}
}

/** One signed frame's identity. */
export type Identity = {
	path: string;
	epoch: string;
	track: string;
	group: bigint;
	frame: bigint;
};

function utf8(text: string): Uint8Array<ArrayBuffer> {
	return new TextEncoder().encode(text);
}

function hex(text: string): Uint8Array<ArrayBuffer> {
	return Uint8Array.from(text.match(/../g) ?? [], (byte) => Number.parseInt(byte, 16));
}

function toHex(bytes: Uint8Array): string {
	return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function concat(...parts: Uint8Array[]): Uint8Array<ArrayBuffer> {
	const out = new Uint8Array(parts.reduce((sum, part) => sum + part.byteLength, 0));
	let offset = 0;
	for (const part of parts) {
		out.set(part, offset);
		offset += part.byteLength;
	}
	return out;
}

function u64(value: bigint): Uint8Array<ArrayBuffer> {
	const out = new Uint8Array(8);
	new DataView(out.buffer).setBigUint64(0, value);
	return out;
}

function bytes(text: string): Uint8Array<ArrayBuffer> {
	const data = utf8(text);
	return concat(u64(BigInt(data.byteLength)), data);
}

/** The message a frame's signature covers. */
export function message(identity: Identity, payload: Uint8Array): Uint8Array<ArrayBuffer> {
	return concat(
		LABEL,
		bytes(identity.path),
		bytes(identity.epoch),
		bytes(identity.track),
		u64(identity.group),
		u64(identity.frame),
		payload,
	);
}

/** Parse an endpoint ID: exactly 64 lowercase hex characters. */
export function parseKey(text: string): Uint8Array<ArrayBuffer> {
	if (!/^[0-9a-f]{64}$/.test(text)) throw new ProfileError("key");
	return hex(text);
}

async function privateKey(seed: Uint8Array<ArrayBuffer>): Promise<CryptoKey> {
	return crypto.subtle.importKey("pkcs8", concat(PKCS8_PREFIX, seed), ED25519, true, ["sign"]);
}

/** The public key, and endpoint ID, of a 32-byte seed. */
export async function publicKey(seed: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
	const jwk = await crypto.subtle.exportKey("jwk", await privateKey(seed));
	return Uint8Array.from(atob((jwk.x ?? "").replaceAll("-", "+").replaceAll("_", "/")), (c) => c.charCodeAt(0));
}

/** The frame body: `payload || signature`. */
export async function sign(
	seed: Uint8Array<ArrayBuffer>,
	identity: Identity,
	payload: Uint8Array,
): Promise<Uint8Array<ArrayBuffer>> {
	const signature = await crypto.subtle.sign(ED25519, await privateKey(seed), message(identity, payload));
	return concat(payload, new Uint8Array(signature));
}

/** The payload of a body whose signature verifies under `key` at `identity`. */
export async function verify(
	key: Uint8Array<ArrayBuffer>,
	identity: Identity,
	body: Uint8Array<ArrayBuffer>,
): Promise<Uint8Array<ArrayBuffer>> {
	if (body.byteLength < SIGNATURE_LEN) throw new ProfileError("signature");
	const split = body.byteLength - SIGNATURE_LEN;
	const payload = body.slice(0, split);
	const verifier = await crypto.subtle.importKey("raw", key, ED25519, false, ["verify"]);
	if (!(await crypto.subtle.verify(ED25519, verifier, body.slice(split), message(identity, payload)))) {
		throw new ProfileError("signature");
	}
	return payload;
}

// RFC 8032 Section 7.1 TEST 1, so the key derivation is checked against an outside answer.
const RFC8032_SEED = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const RFC8032_PUBLIC = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const SEED = toHex(utf8("moq-sign-00 test seed!!!!!!!!!!!"));
const OTHER_SEED = toHex(utf8("moq-sign-00 other seed!!!!!!!!!!"));
const EPOCH = "019a2b3c-4d5e-7f60-8a1b-2c3d4e5f6a7b";
const OTHER_EPOCH = "019a2b3c-4d5e-7f60-8a1b-2c3d4e5f6a7c";

type IdentityJson = { path: string; epoch: string; track: string; group: number; frame: number };

function identityJson(identity: Identity): IdentityJson {
	return { ...identity, group: Number(identity.group), frame: Number(identity.frame) };
}

function parseIdentity(row: IdentityJson): Identity {
	return { ...row, group: BigInt(row.group), frame: BigInt(row.frame) };
}

async function frameVector(id: string, seed: string, identity: Identity, payload: Uint8Array) {
	return {
		id,
		seed,
		public: toHex(await publicKey(hex(seed))),
		...identityJson(identity),
		payload: toHex(payload),
		message: toHex(message(identity, payload)),
		body: toHex(await sign(hex(seed), identity, payload)),
	};
}

async function generate() {
	const base: Identity = { path: "mesh/node/ssh", epoch: EPOCH, track: "stream", group: 3n, frame: 1n };
	const payload = utf8("hello");
	const body = await sign(hex(SEED), base, payload);
	const key = toHex(await publicKey(hex(SEED)));
	const negative = (id: string, identity: Identity, frameBody: Uint8Array, publicHex = key) => ({
		id,
		public: publicHex,
		...identityJson(identity),
		body: toHex(frameBody),
		error: "signature",
	});
	const flipped = (index: number) => {
		const out = body.slice();
		out[index] ^= 0x01;
		return out;
	};

	return {
		profile: PROFILE,
		constants: { label: toHex(LABEL), key_len: KEY_LEN, signature_len: SIGNATURE_LEN },
		keys: [
			{ id: "rfc8032-test-1", seed: RFC8032_SEED, public: RFC8032_PUBLIC },
			{ id: "test", seed: SEED, public: key },
			{ id: "other", seed: OTHER_SEED, public: toHex(await publicKey(hex(OTHER_SEED))) },
		],
		frames: [
			await frameVector("base", SEED, base, payload),
			await frameVector("empty-payload", SEED, { ...base, frame: 0n }, new Uint8Array()),
			await frameVector("group-above-u32", SEED, { ...base, group: 2n ** 40n + 5n, frame: 0n }, payload),
			await frameVector("frame-above-u32", SEED, { ...base, frame: 2n ** 32n }, payload),
			await frameVector("max-u53", SEED, { ...base, group: 2n ** 53n - 1n }, payload),
			await frameVector("empty-path-utf8-track", SEED, { ...base, path: "", track: "café" }, payload),
			await frameVector("other-key", OTHER_SEED, base, payload),
		],
		negative: [
			negative("relocated-path", { ...base, path: "mesh/node/http" }, body),
			negative("relocated-epoch", { ...base, epoch: OTHER_EPOCH }, body),
			negative("relocated-track", { ...base, track: "stream2" }, body),
			negative("relocated-group", { ...base, group: 4n }, body),
			negative("relocated-frame", { ...base, frame: 0n }, body),
			// Shifting a byte between the path and track keeps their concatenation equal.
			negative("shifted-boundary", { ...base, path: "mesh/node/sshs", track: "tream" }, body),
			negative("wrong-key", base, body, toHex(await publicKey(hex(OTHER_SEED)))),
			negative("tampered-payload", base, flipped(0)),
			negative("tampered-signature", base, flipped(body.byteLength - 1)),
			negative("appended-byte", base, concat(body, new Uint8Array([0]))),
			negative("truncated", base, body.slice(0, SIGNATURE_LEN - 1)),
			negative("empty", base, new Uint8Array()),
		],
		invalid_ids: [
			{ id: "uppercase", text: key.toUpperCase() },
			{ id: "short", text: key.slice(0, 63) },
			{ id: "long", text: `${key}0` },
			{ id: "non-hex", text: `${key.slice(0, 63)}g` },
			{ id: "prefixed", text: `0x${key.slice(2)}` },
			{ id: "empty", text: "" },
		],
	};
}

async function check(doc: Awaited<ReturnType<typeof generate>>): Promise<void> {
	for (const row of doc.keys) {
		assertEqual(`${row.id} public`, toHex(await publicKey(hex(row.seed))), row.public);
		assertEqual(`${row.id} parse`, toHex(parseKey(row.public)), row.public);
	}
	for (const row of doc.frames) {
		const identity = parseIdentity(row);
		assertEqual(`${row.id} message`, toHex(message(identity, hex(row.payload))), row.message);
		assertEqual(`${row.id} body`, toHex(await sign(hex(row.seed), identity, hex(row.payload))), row.body);
		assertEqual(`${row.id} verify`, toHex(await verify(hex(row.public), identity, hex(row.body))), row.payload);
	}
	for (const row of doc.negative) {
		await expectError(row.error, () => verify(hex(row.public), parseIdentity(row), hex(row.body)));
	}
	for (const row of doc.invalid_ids) {
		await expectError("key", () => parseKey(row.text));
	}
}

function assertEqual(label: string, actual: string, expected: string): void {
	if (actual !== expected) throw new Error(`${label}: ${actual} != ${expected}`);
}

async function expectError(code: string, fn: () => unknown | Promise<unknown>): Promise<void> {
	try {
		await fn();
	} catch (error) {
		if (error instanceof ProfileError && error.code === code) return;
		throw error;
	}
	throw new Error(`expected ${code}`);
}

async function main(): Promise<void> {
	const generated = await generate();
	const jsonPath = join(dirname(fileURLToPath(import.meta.url)), "moq-sign-00.json");
	if (process.argv.includes("--write")) writeFileSync(jsonPath, `${JSON.stringify(generated, null, "\t")}\n`);
	const onDisk = JSON.parse(readFileSync(jsonPath, "utf8")) as Awaited<ReturnType<typeof generate>>;
	await check(onDisk);
	if (JSON.stringify(onDisk) !== JSON.stringify(generated)) {
		throw new Error("drafts/moq-sign-00.json is stale; run bun drafts/moq-sign-00.ts --write");
	}
	console.log(
		`moq-sign-00: ${onDisk.keys.length} key, ${onDisk.frames.length} frame, ${onDisk.negative.length} negative, ${onDisk.invalid_ids.length} invalid ID vectors`,
	);
}

if (import.meta.main) {
	await main();
}
