// Every vector in drafts/moq-sign-00.json, run against the internal primitives.
import { expect, test } from "bun:test";
import { Epoch, type Path } from "@moq/net";
import { Failure } from "./failure.ts";
import { PublicKey, SecretKey } from "./key.ts";
import { Scope } from "./scope.ts";

type Row = Record<string, unknown>;

const vectors = (await Bun.file(new URL("../../../drafts/moq-sign-00.json", import.meta.url)).json()) as {
	keys: Row[];
	frames: Row[];
	negative: Row[];
	invalid_ids: Row[];
};

function unhex(value: unknown): Uint8Array<ArrayBuffer> {
	return Uint8Array.from(String(value).match(/../g) ?? [], (byte) => Number.parseInt(byte, 16));
}

function hex(bytes: Uint8Array): string {
	return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

// The vectors' path field is the raw signed text, including an empty one.
function scope(row: Row): Scope {
	return new Scope(String(row.path) as Path.Valid, Epoch.parse(String(row.epoch)), String(row.track));
}

test("keys", async () => {
	for (const row of vectors.keys) {
		const key = await SecretKey.fromSeed(unhex(row.seed));
		expect(key.public.toString()).toBe(String(row.public));
		expect(PublicKey.parse(String(row.public)).equals(key.public)).toBe(true);
	}
});

test("frames", async () => {
	for (const row of vectors.frames) {
		const s = scope(row);
		const [group, frame] = [Number(row.group), Number(row.frame)];
		expect(hex(s.message(group, frame, unhex(row.payload)))).toBe(String(row.message));
		const body = await s.sign(await SecretKey.fromSeed(unhex(row.seed)), group, frame, unhex(row.payload));
		expect(hex(body)).toBe(String(row.body));
		const payload = await s.verify(PublicKey.parse(String(row.public)), group, frame, unhex(row.body));
		expect(hex(payload)).toBe(String(row.payload));
	}
});

test("negative", async () => {
	for (const row of vectors.negative) {
		const verified = scope(row).verify(
			PublicKey.parse(String(row.public)),
			Number(row.group),
			Number(row.frame),
			unhex(row.body),
		);
		await expect(verified).rejects.toEqual(new Failure("signature"));
	}
});

test("invalid ids", () => {
	for (const row of vectors.invalid_ids) {
		expect(() => PublicKey.parse(String(row.text))).toThrow(new Failure("key"));
	}
	expect(() => PublicKey.fromBytes(new Uint8Array(31))).toThrow(new Failure("key"));
});
