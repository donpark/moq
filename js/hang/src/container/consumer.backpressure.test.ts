import { expect, spyOn, test } from "bun:test";
import { type Micro, Milli } from "@moq/clock";
import { Time, Track } from "@moq/net";
import { Consumer, type Read } from "./consumer.ts";
import { encodeFrame, Format } from "./legacy.ts";
import type { Frame } from "./types.ts";

// Counts decoded samples, so a test can tell how many the consumer holds ahead of its reads.
class Counting extends Format {
	decoded = 0;

	override decode(frame: Uint8Array): Frame[] {
		const frames = super.decode(frame);
		this.decoded += frames.length;
		return frames;
	}
}

const PAYLOAD = new Uint8Array([0xde, 0xad]);

// Let the consumer's group tasks run until they park: on a read, or on a full max buffer.
const flush = () => new Promise<void>((resolve) => setImmediate(resolve));

function write(group: { writeFrame(frame: { payload: Uint8Array; timestamp: Time.Timestamp }): void }, ts: number) {
	group.writeFrame({ payload: encodeFrame(PAYLOAD, ts as Micro), timestamp: Time.Timestamp.now() });
}

// The most frames the consumer may hold: past the max buffer, one more wire frame. Each held frame
// costs at least its payload and its own entry.
function maxHeld(maxBuffer: number): number {
	return Math.ceil(maxBuffer / (64 + PAYLOAD.byteLength)) + 1;
}

// Read every item. With `bounded`, check after each, once decoding has caught up, that the consumer
// holds no more than the max buffer allows, which only holds while no skip drops decoded frames unread.
async function readAll(consumer: Consumer, format: Counting, bounded?: number): Promise<Read[]> {
	const reads: Read[] = [];
	let delivered = 0;
	for (;;) {
		const read = await consumer.next();
		await flush();
		if (read === undefined) return reads;
		reads.push(read);
		if (read.kind === "frame") delivered++;
		if (bounded !== undefined) expect(format.decoded - delivered).toBeLessThanOrEqual(maxHeld(bounded));
	}
}

function timestamps(reads: Read[]): number[] {
	return reads.flatMap((read) => (read.kind === "frame" ? [read.frame.timestamp] : []));
}

test("Consumer holds frames sharing a timestamp within the max buffer", async () => {
	const warn = spyOn(console, "warn").mockImplementation(() => {});
	const maxBuffer = 4096;
	const format = new Counting("data");
	const track = new Track.Producer("test");
	const consumer = new Consumer(track.subscribe({ maxDelay: Milli(30_000) }), {
		format,
		maxDelay: Milli(5_000),
		maxBuffer,
	});
	try {
		// Frames packed into one timestamp have no span, so the age budget never skips the stalled
		// group they queue behind. They fill the max buffer instead, which skips it once its one frame
		// is read, so nothing decoded is dropped.
		const stalled = track.createGroup(0);
		write(stalled, 0);
		const packed = track.createGroup(1);
		for (let i = 0; i < 2_000; i++) write(packed, 10_000);
		packed.finish();
		track.finish();

		const reads = await readAll(consumer, format, maxBuffer);
		const ts = timestamps(reads);
		expect(ts.length).toBe(1 + 2_000);
		expect(ts.at(0)).toBe(0);
		expect(ts.at(-1)).toBe(10_000);
		// The stalled group's tail is lost.
		expect(reads[1]?.kind).toBe("gap");
	} finally {
		consumer.close();
		track.close();
		warn.mockRestore();
	}
});

test("Consumer holds many groups within the max buffer and reaches the live edge", async () => {
	const warn = spyOn(console, "warn").mockImplementation(() => {});
	const maxBuffer = 8192;
	const format = new Counting("data");
	const track = new Track.Producer("test");
	const consumer = new Consumer(track.subscribe({ maxDelay: Milli(30_000) }), {
		format,
		maxDelay: Milli(100),
		maxBuffer,
	});
	try {
		const stalled = track.createGroup(0);
		write(stalled, 0);
		for (let g = 1; g <= 500; g++) {
			const group = track.createGroup(g);
			write(group, g * 1_000);
			group.finish();
		}
		track.finish();

		// The groups' overhead and first frames fill the max buffer, and the rest wait unpulled.
		await flush();
		expect(format.decoded).toBeLessThanOrEqual(maxHeld(maxBuffer));
		expect(format.decoded).toBeGreaterThan(1);

		const reads = await readAll(consumer, format);
		const ts = timestamps(reads);
		expect(ts.at(0)).toBe(0);
		expect(ts.at(-1)).toBe(500_000);
	} finally {
		consumer.close();
		track.close();
		warn.mockRestore();
	}
});

test("Consumer skips a stalled group once group overhead alone fills the max buffer", async () => {
	const warn = spyOn(console, "warn").mockImplementation(() => {});
	const format = new Counting("data");
	const track = new Track.Producer("test");
	const consumer = new Consumer(track.subscribe({ maxDelay: Milli(30_000) }), {
		format,
		maxDelay: Milli(5_000),
		// Two groups' overhead, so nothing past the stalled head would decode without its successor leading.
		maxBuffer: 512,
	});
	try {
		const stalled = track.createGroup(0);
		write(stalled, 0);
		const next = track.createGroup(1);
		write(next, 10_000);
		next.finish();

		expect(await consumer.next()).toMatchObject({ kind: "frame", frame: { timestamp: 0 } });
		expect(await consumer.next()).toMatchObject({ kind: "gap" });
		expect(await consumer.next()).toMatchObject({ kind: "frame", frame: { timestamp: 10_000 } });
	} finally {
		consumer.close();
		track.close();
		warn.mockRestore();
	}
});

test("Consumer reaches a readable group past a stalled successor once group overhead fills the max buffer", async () => {
	const warn = spyOn(console, "warn").mockImplementation(() => {});
	const format = new Counting("data");
	const track = new Track.Producer("test");
	const consumer = new Consumer(track.subscribe({ maxDelay: Milli(30_000) }), {
		format,
		maxDelay: Milli.zero,
		// Three groups' overhead: the readable group cannot decode until the stalled ones are moved past.
		maxBuffer: 768,
	});
	try {
		const stalled = track.createGroup(0);
		write(stalled, 0);
		track.createGroup(1);
		const readable = track.createGroup(2);
		write(readable, 10_000);
		write(readable, 20_000);
		readable.finish();

		expect(await consumer.next()).toMatchObject({ kind: "frame", frame: { timestamp: 0 } });
		expect(await consumer.next()).toMatchObject({ kind: "gap" });
		expect(await consumer.next()).toMatchObject({ kind: "frame", frame: { timestamp: 10_000 } });
		expect(await consumer.next()).toMatchObject({ kind: "frame", frame: { timestamp: 20_000 } });
	} finally {
		consumer.close();
		track.close();
		warn.mockRestore();
	}
});

test("Consumer admits reads completing together one at a time against the max buffer", async () => {
	const format = new Counting("data");
	const track = new Track.Producer("test");
	const consumer = new Consumer(track.subscribe({ maxDelay: Milli(30_000) }), {
		format,
		maxDelay: Milli(5_000),
		maxBuffer: 4096,
	});
	try {
		// Ten open groups park on their first read with room to spare, then large frames land in all of
		// them at once.
		const groups = Array.from({ length: 10 }, (_, sequence) => track.createGroup(sequence));
		await flush();
		const large = new Uint8Array(2_000);
		for (const group of groups) {
			group.writeFrame({ payload: encodeFrame(large, 0 as Micro), timestamp: Time.Timestamp.now() });
		}
		await flush();

		// The first decode fills the max buffer, so only the head may decode past it.
		expect(format.decoded).toBeLessThanOrEqual(2);
	} finally {
		consumer.close();
		track.close();
	}
});
