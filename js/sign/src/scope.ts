// The signed message: what a frame's signature binds besides its payload.
import type { Epoch, Path } from "@moq/net";
import { Failure } from "./failure.ts";
import type { PublicKey, SecretKey } from "./key.ts";

/** Length of the Ed25519 signature appended to every signed frame. */
export const SIGNATURE_LEN = 64;

const encoder = new TextEncoder();
// Domain separation for the profile, with no length prefix of its own.
const LABEL = encoder.encode("moq-sign-00");

/** One track of one publisher instance: the identity every frame on it is signed under. */
export class Scope {
	// The canonical prefix: label, path, epoch, and track, each length-prefixed.
	#prefix: Uint8Array<ArrayBuffer>;

	constructor(path: Path.Valid, epoch: Epoch.Valid, track: string) {
		const fields = [path, epoch, track].map((field) => encoder.encode(field));
		this.#prefix = new Uint8Array(LABEL.byteLength + fields.reduce((sum, field) => sum + 8 + field.byteLength, 0));
		const view = new DataView(this.#prefix.buffer);
		this.#prefix.set(LABEL);
		let offset = LABEL.byteLength;
		for (const field of fields) {
			view.setBigUint64(offset, BigInt(field.byteLength));
			this.#prefix.set(field, offset + 8);
			offset += 8 + field.byteLength;
		}
	}

	/** The message signed for `payload` at frame `frame` of group `group`. */
	message(group: number, frame: number, payload: Uint8Array): Uint8Array<ArrayBuffer> {
		const out = new Uint8Array(this.#prefix.byteLength + 16 + payload.byteLength);
		const view = new DataView(out.buffer);
		out.set(this.#prefix);
		view.setBigUint64(this.#prefix.byteLength, BigInt(group));
		view.setBigUint64(this.#prefix.byteLength + 8, BigInt(frame));
		out.set(payload, this.#prefix.byteLength + 16);
		return out;
	}

	/** The frame body: `payload` followed by its signature. */
	async sign(key: SecretKey, group: number, frame: number, payload: Uint8Array): Promise<Uint8Array<ArrayBuffer>> {
		const signature = await key.sign(this.message(group, frame, payload));
		const body = new Uint8Array(payload.byteLength + SIGNATURE_LEN);
		body.set(payload);
		body.set(signature, payload.byteLength);
		return body;
	}

	/** The payload of a frame body whose signature verifies, as a view into `body`; throws {@link Failure} `signature`. */
	async verify(key: PublicKey, group: number, frame: number, body: Uint8Array): Promise<Uint8Array> {
		const split = body.byteLength - SIGNATURE_LEN;
		if (split < 0) throw new Failure("signature");
		const payload = body.subarray(0, split);
		if (!(await key.verify(this.message(group, frame, payload), body.slice(split)))) throw new Failure("signature");
		return payload;
	}
}
