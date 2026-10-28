/**
 * Resume checked against a model of one logical track, mirroring the Rust `model::resume_model`.
 *
 * Random sequences of route flips, route failures, give-ups, ranges, cache expiry, and reader
 * joins and leaves drive an origin's fronts on the manual clock. Every reader's groups, frames,
 * drops, and end are checked against what one plain cursor on the logical track would see:
 * every group its range and budget allow, each once, in its order. A newer publisher epoch wins
 * the path while older readers stay on theirs until it has no route left, then end with an error.
 *
 * `MOQ_RESUME_SEED` and `MOQ_RESUME_CASES` pick the cases; nightly sweeps a fresh seed.
 */

import { test } from "bun:test";
import { Manual, Milli, Nano } from "@moq/clock";
import fc from "fast-check";
import { Producer as BroadcastProducer } from "./broadcast.ts";
import * as Epoch from "./epoch.ts";
import { NotFound, StreamCode, StreamError, TooFarBehind } from "./error.ts";
import type { Frame, Consumer as GroupConsumer, Producer as GroupProducer } from "./group.ts";
import { HopSchema, Route } from "./hop.ts";
import { hooks } from "./internal.ts";
import { type Consumer as OriginConsumer, Producer as OriginProducer, type Requesting } from "./origin.ts";
import * as Path from "./path.ts";
import { routeFailed } from "./resume.ts";
import { Timescale, Timestamp } from "./time.ts";
import type * as track from "./track.ts";
import { wireOf } from "./wire.ts";

/** Groups in the logical track; group `g` has `1 + g % 3` frames. */
const GROUPS = 12;
/** Routes per epoch, cheaper the higher the slot. */
const SLOTS = 3;
/** The older publisher instance, then a newer one. */
const EPOCHS = 2;
const READERS = 4;
const PATH = Path.from("room/alice");
/** A budget nothing in a case outlives. */
const HOUR = Milli(3_600_000);
/** A budget spanning about three groups. */
const SHORT = Milli(30);

const enc = new TextEncoder();
const dec = new TextDecoder();

function frames(group: number): number {
	return 1 + (group % 3);
}

/** Frame `frame` of `group` as `epoch` publishes it: every route of one epoch carries it. */
function payload(epoch: number, group: number, frame: number): string {
	return `e${epoch}:${group}.${frame}`;
}

// UUIDv7s minted in order, so the second is the newer instance.
const EPOCH_IDS = [
	Epoch.parse("01900000-0000-7000-8000-000000000001"),
	Epoch.parse("01900000-0001-7000-8000-000000000001"),
];

/** How a route fails: its session closes on the far side. */
function routeFailure(): Error {
	return new StreamError(StreamCode.SessionClosed);
}

/** A verdict on content, not a route failing: another route may still deliver it. */
function giveUp(): Error {
	return new StreamError(StreamCode.DeliveryTimeout);
}

type Budget = "hour" | "short";
type Range = readonly [number, number] | undefined;

interface Spec {
	/** Sequence order through {@link track.Ordered} rather than arrival order. */
	ordered: boolean;
	budget: Budget;
	/** The subscription's one group range, or the default. */
	range: Range;
	/** Polls drops from the start, as a publisher forwarding them does. */
	drops: boolean;
}

type Op =
	/** The route announces itself to the origin. */
	| { kind: "up"; epoch: number; slot: number }
	/** The route's session fails, failing its copy. */
	| { kind: "down"; epoch: number; slot: number }
	/**
	 * The route's copy gets up to `frames` more frames of `group`, or of the lowest group it
	 * lacks, finishing the group once complete.
	 */
	| { kind: "write"; epoch: number; slot: number; group?: number; frames: number }
	/** The route gives up `group` with a verdict on its content. */
	| { kind: "giveUp"; epoch: number; slot: number; group: number }
	/** The route's copy declares the track's end, once it holds the last group. */
	| { kind: "end"; epoch: number; slot: number }
	| { kind: "advance"; ms: number }
	/** Long enough for idle cached groups to expire. */
	| { kind: "expire" }
	| { kind: "join"; reader: number; spec: Spec }
	| { kind: "leave"; reader: number }
	/** Replace the subscription's group range. */
	| { kind: "ranges"; reader: number; range: Range }
	/** Replace the reader's local group bounds. */
	| { kind: "bounds"; reader: number; range: Range }
	/** Poll the reader whether or not it was woken. */
	| { kind: "read"; reader: number };

interface Case {
	ops: Op[];
	/** Every live route declares the end once it has caught up. */
	finish: boolean;
}

const range: fc.Arbitrary<Range> = fc.option(
	fc
		.tuple(fc.nat({ max: GROUPS - 1 }), fc.integer({ min: 1, max: GROUPS }))
		.map(([start, len]): Range => [start, Math.min(start + len, GROUPS)]),
	{ nil: undefined, freq: 2 },
);
const epoch = fc.nat({ max: EPOCHS - 1 });
const slot = fc.nat({ max: SLOTS - 1 });
const reader = fc.nat({ max: READERS - 1 });
const spec: fc.Arbitrary<Spec> = fc.record({
	ordered: fc.boolean(),
	budget: fc.oneof(
		{ arbitrary: fc.constant<Budget>("hour"), weight: 3 },
		{ arbitrary: fc.constant<Budget>("short"), weight: 1 },
	),
	range,
	drops: fc.boolean(),
});
const op: fc.Arbitrary<Op> = fc.oneof(
	{
		weight: 30,
		arbitrary: fc.record({
			kind: fc.constant("write" as const),
			epoch,
			slot,
			group: fc.oneof(
				{ weight: 7, arbitrary: fc.constant(undefined) },
				{ weight: 3, arbitrary: fc.nat({ max: GROUPS - 1 }) },
			),
			frames: fc.integer({ min: 1, max: 3 }),
		}),
	},
	{ weight: 8, arbitrary: fc.record({ kind: fc.constant("up" as const), epoch, slot }) },
	{ weight: 6, arbitrary: fc.record({ kind: fc.constant("down" as const), epoch, slot }) },
	{
		weight: 4,
		arbitrary: fc.record({ kind: fc.constant("giveUp" as const), epoch, slot, group: fc.nat({ max: GROUPS - 2 }) }),
	},
	{ weight: 2, arbitrary: fc.record({ kind: fc.constant("end" as const), epoch, slot }) },
	{
		weight: 6,
		arbitrary: fc.record({ kind: fc.constant("advance" as const), ms: fc.integer({ min: 1, max: 200 }) }),
	},
	{ weight: 1, arbitrary: fc.constant({ kind: "expire" as const }) },
	{ weight: 10, arbitrary: fc.record({ kind: fc.constant("join" as const), reader, spec }) },
	{ weight: 3, arbitrary: fc.record({ kind: fc.constant("leave" as const), reader }) },
	{ weight: 3, arbitrary: fc.record({ kind: fc.constant("ranges" as const), reader, range }) },
	{ weight: 2, arbitrary: fc.record({ kind: fc.constant("bounds" as const), reader, range }) },
	{ weight: 8, arbitrary: fc.record({ kind: fc.constant("read" as const), reader }) },
);
const cases: fc.Arbitrary<Case> = fc.record({
	ops: fc.array(op, { minLength: 1, maxLength: 63, size: "max" }),
	finish: fc.boolean(),
});

/** One route's copy of the logical track, as its upstream publishes it. */
interface Copy {
	track: track.Producer;
	/** Answers the front's requests for the route until the route goes. */
	serving: { close(): void };
	broadcast: BroadcastProducer;
	/** Frames written per group, with the writer while the group is open. */
	groups: Map<number, { written: number; writer?: GroupProducer }>;
	givenUp: Set<number>;
}

function complete(copy: Copy, group: number): boolean {
	return copy.groups.get(group)?.written === frames(group);
}

/** One publisher instance: its routes, and what any of them ever carried. */
class Instance {
	routes: (Copy | undefined)[] = Array.from({ length: SLOTS }, () => undefined);
	/** Most frames of each group any route carried. */
	written = new Map<number, number>();
	/** Groups some route finished. */
	complete = new Set<number>();
	/** Groups some route gave up. */
	givenUp = new Set<number>();
	/** Some route declared the track's end. */
	ended = false;
	/** The newest group when idle cached groups last expired: any route may have let go of those. */
	expired?: number;

	live(): Copy[] {
		return this.routes.filter((copy) => copy !== undefined);
	}

	newest(): number | undefined {
		return this.written.size === 0 ? undefined : Math.max(...this.written.keys());
	}
}

/** A group a reader was handed, and how far it read it. */
interface Reading {
	group: GroupConsumer;
	frames: number;
	/** `null` once it ended cleanly, the error once it failed. */
	end?: Error | null;
}

interface Reader {
	instance: number;
	spec: Spec;
	request: Requesting;
	sub: track.Subscriber;
	ordered?: track.Ordered;
	/** Set by a notification since the last poll. */
	flag: boolean;
	unwatch?: () => void;
	/** Left the case: whatever its groups do from here is not checked. */
	gone: boolean;
	/** The subscription's group range. */
	ranges: Range;
	/**
	 * The one group cursor reads honor. Unlike the Rust reader's two filters, a JS subscriber's
	 * subscription range only seeds it: an update resets it to that range, and local bounds
	 * replace it in either direction.
	 */
	cursor: Range;
	/** Every group handed out, in order. */
	handed: number[];
	reading: Map<number, Reading>;
	/** Groups reported dropped. */
	dropped: Set<number>;
	/** Only groups past this one are owed: older ones were cached history at join, or may have expired. */
	floor?: number;
	/** Its instance lost its last route while it read. */
	doomed: boolean;
	/**
	 * The newest group handed out when its routes last changed. A route change resumes from
	 * there, so a group below it that lands only afterwards may be lost.
	 */
	resumed?: number;
	/** `null` once the track ended cleanly, the error once it failed. */
	end?: Error | null;
	/** What every live route carried in full when the track ended cleanly. */
	carried: Set<number>;
}

function within(range: Range, group: number): boolean {
	return range === undefined || (range[0] <= group && group < range[1]);
}

function covers(reader: Reader, group: number): boolean {
	return within(reader.cursor, group);
}

/** Everything observed, to tell whether a poll found anything new. */
function observed(reader: Reader): string {
	let read = 0;
	for (const reading of reader.reading.values()) read += reading.frames + (reading.end !== undefined ? 1 : 0);
	return JSON.stringify([reader.handed.length, read, reader.end !== undefined, reader.dropped.size]);
}

function groups(range: Range): track.Groups {
	return range ? { start: { included: range[0] }, end: { excluded: range[1] } } : {};
}

function subscription(budget: Budget, range: Range): track.Subscription {
	return { maxDelay: budget === "hour" ? HOUR : SHORT, groups: range ? groups(range) : undefined };
}

function max(values: Iterable<number>): number | undefined {
	let most: number | undefined;
	for (const value of values) most = Math.max(most ?? value, value);
	return most;
}

function fail(reader: number, what: string): Error {
	return new Error(`reader ${reader}: ${what}`);
}

class Model {
	clock: Manual;
	origin = new OriginProducer();
	consumer: OriginConsumer;
	instances = Array.from({ length: EPOCHS }, () => new Instance());
	readers: (Reader | undefined)[] = Array.from({ length: READERS }, () => undefined);
	/** The first check a frame read failed, which ends the case once it settles. */
	failure?: Error;
	/** Bumped by every frame read, so settling sees progress no reader flag shows. */
	progress = 0;

	constructor(clock: Manual) {
		this.clock = clock;
		this.consumer = this.origin.consume();
	}

	async apply(op: Op): Promise<void> {
		if (op.kind === "up" || op.kind === "down") {
			for (const reader of this.readers) {
				if (reader?.instance !== op.epoch) continue;
				const newest = max(reader.handed);
				if (newest !== undefined) reader.resumed = Math.max(reader.resumed ?? newest, newest);
			}
		}
		switch (op.kind) {
			case "up":
				this.up(op.epoch, op.slot);
				break;
			case "down": {
				const instance = this.instances[op.epoch];
				const copy = instance.routes[op.slot];
				if (!copy) return;
				instance.routes[op.slot] = undefined;
				copy.serving.close();
				for (const { writer } of copy.groups.values()) writer?.abort(routeFailure());
				copy.track.abort(routeFailure());
				copy.broadcast.close();
				if (instance.live().length === 0) {
					for (const reader of this.readers) {
						if (reader?.instance === op.epoch) reader.doomed = true;
					}
				}
				break;
			}
			case "write": {
				const copy = this.instances[op.epoch].routes[op.slot];
				if (!copy) return;
				let group = op.group;
				if (group === undefined) {
					for (let candidate = 0; candidate < GROUPS; candidate++) {
						if (!complete(copy, candidate) && !copy.givenUp.has(candidate)) {
							group = candidate;
							break;
						}
					}
				}
				if (group !== undefined) this.write(op.epoch, op.slot, group, op.frames);
				break;
			}
			case "giveUp": {
				const instance = this.instances[op.epoch];
				const copy = instance.routes[op.slot];
				if (!copy || complete(copy, op.group) || copy.givenUp.has(op.group)) return;
				copy.givenUp.add(op.group);
				const entry = copy.groups.get(op.group);
				if (entry?.writer) {
					entry.writer.abort(giveUp());
					entry.writer = undefined;
				} else if (!entry) {
					hooks.dropTrackGroups(copy.track, op.group, op.group + 1, giveUp());
				}
				instance.givenUp.add(op.group);
				break;
			}
			case "end":
				this.end(op.epoch, op.slot);
				break;
			case "advance":
				await this.clock.advance(Nano.fromMilli(Milli(op.ms)));
				break;
			case "expire":
				await this.clock.advance(Nano.fromMilli(Milli(60_000)));
				for (const instance of this.instances) instance.expired = instance.newest();
				for (const reader of this.readers) {
					if (reader) reader.floor = this.instances[reader.instance].newest();
				}
				break;
			case "join":
				await this.join(op.reader, op.spec);
				break;
			case "leave":
				this.leave(op.reader);
				break;
			case "ranges": {
				const reader = this.readers[op.reader];
				if (!reader) return;
				reader.ranges = op.range;
				reader.cursor = op.range;
				reader.sub.update(subscription(reader.spec.budget, op.range));
				this.poll(op.reader);
				break;
			}
			case "bounds": {
				const reader = this.readers[op.reader];
				if (!reader) return;
				reader.cursor = op.range;
				(reader.ordered ?? reader.sub).withRange(groups(op.range));
				this.poll(op.reader);
				break;
			}
			case "read":
				this.poll(op.reader);
				break;
		}
	}

	up(epoch: number, slot: number): void {
		const instance = this.instances[epoch];
		if (instance.routes[slot]) return;
		const route = Route.normalize({
			epoch: EPOCH_IDS[epoch],
			hops: [HopSchema.parse(BigInt(10 * (epoch + 1) + slot))],
			cost: BigInt(SLOTS - slot),
		});
		const serving = wireOf(this.origin).receive(PATH, route);
		const broadcast = new BroadcastProducer();
		const producer = broadcast.createTrack("video", { timescale: Timescale.MILLI });
		void (async () => {
			for await (const request of serving.requested()) hooks.acceptRequest(request, broadcast.consume());
		})();
		instance.routes[slot] = { track: producer, serving, broadcast, groups: new Map(), givenUp: new Set() };
	}

	write(epoch: number, slot: number, group: number, more: number): void {
		const instance = this.instances[epoch];
		const copy = instance.routes[slot];
		if (!copy || copy.givenUp.has(group) || complete(copy, group)) return;
		let entry = copy.groups.get(group);
		if (!entry) {
			entry = { written: 0 };
			copy.groups.set(group, entry);
		}
		if (entry.written === 0 && !entry.writer) {
			try {
				entry.writer = copy.track.createGroup(group);
			} catch {
				return;
			}
		}
		const writer = entry.writer;
		if (!writer) return;
		// The cache reclaimed it while idle: this route never carries the rest.
		if (writer.isClosed) {
			entry.writer = undefined;
			return;
		}
		const until = Math.min(entry.written + more, frames(group));
		while (entry.written < until) {
			const timestamp = Timestamp.fromMillis(group * 10 + entry.written);
			writer.writeFrame({ payload: enc.encode(payload(epoch, group, entry.written)), timestamp });
			entry.written++;
		}
		if (entry.written === frames(group)) {
			writer.finish();
			entry.writer = undefined;
			instance.complete.add(group);
		}
		instance.written.set(group, Math.max(instance.written.get(group) ?? 0, entry.written));
	}

	end(epoch: number, slot: number): void {
		const instance = this.instances[epoch];
		const copy = instance.routes[slot];
		if (!copy?.groups.has(GROUPS - 1)) return;
		copy.track.finish();
		instance.ended = true;
	}

	async join(index: number, spec: Spec): Promise<void> {
		if (this.readers[index]) return;
		// The newest instance with a route wins the path.
		let expected: number | undefined;
		for (let epoch = EPOCHS - 1; epoch >= 0; epoch--) {
			if (this.instances[epoch].live().length > 0) {
				expected = epoch;
				break;
			}
		}
		if (expected === undefined) return;
		const request = this.consumer.request(PATH);
		// Answered by the routes' serving loops, without time passing.
		for (let turn = 0; !request.active.peek(); turn++) {
			const closed = request.closed.peek();
			if (closed !== undefined || turn === 64) {
				request.close();
				throw fail(index, `request failed: ${closed ?? "never resolved"}`);
			}
			await this.clock.advance(Nano.zero);
		}
		const broadcast = request.active.peek();
		const instance = broadcast?.epoch === undefined ? -1 : EPOCH_IDS.indexOf(broadcast.epoch);
		if (!broadcast || instance === -1) {
			request.close();
			throw fail(index, "resolved no known instance");
		}
		if (instance !== expected) {
			request.close();
			throw fail(index, `joined instance ${instance}, not the newest ${expected}`);
		}
		const sub = broadcast.track("video").subscribe(subscription(spec.budget, spec.range));
		this.readers[index] = {
			instance,
			spec,
			request,
			sub,
			ordered: spec.ordered ? sub.ordered() : undefined,
			flag: false,
			gone: false,
			ranges: spec.range,
			cursor: spec.range,
			handed: [],
			reading: new Map(),
			dropped: new Set(),
			floor: this.instances[instance].newest(),
			doomed: false,
			end: undefined,
			carried: new Set(),
		};
		this.poll(index);
	}

	leave(index: number): void {
		const reader = this.readers[index];
		if (!reader) return;
		this.readers[index] = undefined;
		reader.gone = true;
		reader.unwatch?.();
		for (const reading of reader.reading.values()) reading.group.close();
		reader.sub.close();
		reader.request.close();
	}

	/** Read everything `index` has ready, checking each step against the model. */
	poll(index: number): void {
		const reader = this.readers[index];
		if (!reader) return;
		const instance = this.instances[reader.instance];
		reader.flag = false;
		reader.unwatch?.();

		while (reader.end === undefined) {
			const recv = reader.spec.ordered ? hooks.tryNextGroup(reader.sub) : hooks.tryRecvGroup(reader.sub, false);
			if (recv.kind === "idle" || recv.kind === "boundary") break;
			if (recv.kind === "group") {
				const group = recv.group;
				const sequence = group.sequence;
				if (reader.reading.has(sequence)) {
					throw fail(index, `group ${sequence} handed out twice: ${reader.handed}`);
				}
				const last = reader.handed.at(-1);
				if (reader.spec.ordered && last !== undefined && last >= sequence) {
					throw fail(index, `group ${sequence} out of order after ${reader.handed}`);
				}
				if (!covers(reader, sequence)) {
					throw fail(index, `group ${sequence} outside cursor ${reader.cursor}, ranges ${reader.ranges}`);
				}
				if (!instance.written.has(sequence)) {
					throw fail(index, `group ${sequence} handed out before any route had it`);
				}
				reader.handed.push(sequence);
				const reading: Reading = { group, frames: 0 };
				reader.reading.set(sequence, reading);
				void this.#read(index, reader, reading);
			} else if (recv.kind === "done") {
				if (!instance.ended) throw fail(index, "the track ended cleanly though no route declared an end");
				// A group landing below the end afterwards is not owed: a reader stops at it.
				reader.carried = new Set();
				for (let group = 0; group < GROUPS; group++) {
					if (instance.live().every((copy) => complete(copy, group))) reader.carried.add(group);
				}
				reader.end = null;
			} else if (recv.kind === "error") {
				if (!reader.doomed) throw fail(index, `the track failed while its instance had a route: ${recv.error}`);
				reader.end = recv.error;
			} else {
				throw fail(index, `an arrival read without dispositions reported drop ${recv.start}..${recv.end}`);
			}
		}

		if (reader.spec.drops && !reader.spec.ordered) {
			for (let drop = hooks.tryDrop(reader.sub); drop; drop = hooks.tryDrop(reader.sub)) {
				for (let group = drop.start; group < drop.end; group++) {
					if (reader.reading.has(group)) {
						throw fail(
							index,
							`dropped ${drop.start}..${drop.end} (${drop.error}) after handing out ${group}`,
						);
					}
				}
				for (let group = drop.start; group < Math.min(drop.end, GROUPS); group++) reader.dropped.add(group);
			}
		}

		reader.unwatch = hooks.groupChanged(reader.sub, () => {
			reader.flag = true;
		});
	}

	/** Read a group handed out to its end, checking each frame against the model. */
	async #read(index: number, reader: Reader, reading: Reading): Promise<void> {
		const sequence = reading.group.sequence;
		for (;;) {
			let frame: Frame | undefined;
			try {
				frame = await reading.group.readFrame();
			} catch (err) {
				if (reader.gone) return;
				reading.end = err instanceof Error ? err : new Error(String(err));
				this.progress++;
				return;
			}
			if (reader.gone) return;
			this.progress++;
			if (!frame) {
				if (reading.frames !== frames(sequence) || !this.instances[reader.instance].complete.has(sequence)) {
					this.failure ??= fail(index, `group ${sequence} ended cleanly after ${reading.frames} frames`);
				}
				reading.end = null;
				return;
			}
			const expected = payload(reader.instance, sequence, reading.frames);
			const read = dec.decode(frame.payload);
			if (read !== expected || reading.frames >= frames(sequence)) {
				this.failure ??= fail(
					index,
					`group ${sequence} frame ${reading.frames} read ${read}, expected ${expected}`,
				);
				return;
			}
			reading.frames++;
		}
	}

	/** Let the routes run without time passing, polling each reader woken. */
	async settle(): Promise<void> {
		let quiet = 0;
		for (let round = 0; round < 10_000; round++) {
			const progress = this.progress;
			await this.clock.advance(Nano.zero);
			if (this.failure) throw this.failure;
			let woken = this.progress !== progress;
			for (let index = 0; index < READERS; index++) {
				if (this.readers[index]?.flag) {
					woken = true;
					this.poll(index);
				}
			}
			quiet = woken ? 0 : quiet + 1;
			if (quiet === 2) return;
		}
		throw new Error("readers never settled: something keeps waking them");
	}

	/**
	 * Every live route catches up on every group and, for `finish`, declares the end; then each
	 * reader must have read what it is owed.
	 */
	async conclude(finish: boolean): Promise<void> {
		const before = this.readers.map((reader) => reader && max(reader.handed));
		for (let epoch = 0; epoch < EPOCHS; epoch++) {
			for (let slot = 0; slot < SLOTS; slot++) {
				for (let group = 0; group < GROUPS; group++) this.write(epoch, slot, group, Number.POSITIVE_INFINITY);
				if (finish) this.end(epoch, slot);
			}
		}
		await this.settle();

		const seen = this.readers.map((reader) => reader && observed(reader));
		for (let index = 0; index < READERS; index++) {
			const reader = this.readers[index];
			if (!reader) continue;
			this.poll(index);
			// A frame read still waiting with a frame or its end ready was never woken.
			for (const [sequence, reading] of reader.reading) {
				if (reading.end !== undefined) continue;
				const ready = reading.group.done ? "its end" : reading.group.tryReadFrame() ? "a frame" : undefined;
				if (ready) throw fail(index, `group ${sequence} has ${ready} ready that never woke its read`);
			}
			if (observed(reader) !== seen[index]) {
				throw fail(
					index,
					`a poll found progress that never woke it: ${seen[index]} -> ${observed(reader)}, handed ${reader.handed}, dropped ${[...reader.dropped]}, end ${reader.end}`,
				);
			}
			this.owed(index, reader, before[index], finish);
		}
	}

	owed(index: number, reader: Reader, before: number | undefined, finish: boolean): void {
		const instance = this.instances[reader.instance];
		// Unlike Rust, a JS reader capped below the end keeps the groups past its cap claimable
		// after the track finishes, so it stays open for the cap to rise.
		const capped = (reader.cursor?.[1] ?? GROUPS) < GROUPS;
		if (reader.doomed) {
			if (reader.end instanceof Error || ((reader.end === null || capped) && instance.ended)) return;
			throw fail(index, `outlived its instance's routes: ${reader.end}`);
		}
		// What every live route carries in full.
		const carried = (group: number) => instance.live().every((copy) => complete(copy, group));
		let owed: number[] = [];
		for (let group = 0; group < GROUPS; group++) {
			if (reader.floor !== undefined && group <= reader.floor) continue;
			if (reader.resumed !== undefined && group <= reader.resumed) continue;
			if (covers(reader, group) && carried(group)) owed.push(group);
		}
		if (reader.spec.ordered && before !== undefined) owed = owed.filter((group) => group > before);
		if (reader.end === null) owed = owed.filter((group) => reader.carried.has(group));
		if (reader.spec.budget === "short") owed = owed.filter((group) => group === GROUPS - 1);
		const missing = owed.filter((group) => !reader.reading.has(group));
		let problems = "";
		if (missing.length > 0) problems += `missing groups ${missing}; `;
		if (reader.spec.budget === "hour") {
			for (const [sequence, reading] of reader.reading) {
				if (reading.end === null || !carried(sequence) || instance.givenUp.has(sequence)) continue;
				// Like a group under the floor, one older than an expiry may be gone from its own copy
				// or from the route it would continue from.
				const gone = reading.end instanceof TooFarBehind || reading.end instanceof NotFound;
				if (gone && instance.expired !== undefined && sequence <= instance.expired) continue;
				// Unlike Rust, an ended JS reader follows no route, so a route failure ends a group still read.
				if (reader.end === null && reading.end instanceof Error && routeFailed(reading.end)) continue;
				problems += `group ${sequence} stopped at frame ${reading.frames} (${reading.end}); `;
			}
		}
		const live = finish && instance.live().length > 0;
		const end = reader.end;
		const ends =
			(end === undefined && (!live || capped)) ||
			(end === null && live) ||
			(end === null && !live && instance.ended);
		if (!ends) problems += `track end ${end}; `;
		if (problems === "") return;
		throw fail(
			index,
			`${problems}handed out ${reader.handed}, dropped ${[...reader.dropped]}, floor ${reader.floor}, spec ${JSON.stringify(reader.spec)}`,
		);
	}

	/** Let go of everything, so nothing outlives the case. */
	async close(): Promise<void> {
		for (let index = 0; index < READERS; index++) this.leave(index);
		for (const instance of this.instances) {
			for (const copy of instance.live()) {
				copy.serving.close();
				copy.track.abort(routeFailure());
				copy.broadcast.close();
			}
		}
		this.origin.close();
		await this.clock.advance(Nano.zero);
	}
}

async function run(c: Case): Promise<void> {
	const clock = new Manual();
	const installed = clock.install();
	const model = new Model(clock);
	try {
		model.up(0, 0);
		for (const op of c.ops) {
			await model.apply(op);
			await model.settle();
		}
		await model.conclude(c.finish);
	} finally {
		await model.close();
		installed.close();
	}
}

function setting(name: string, fallback: number, min: number): number {
	const raw = process.env[name];
	if (raw !== undefined && !/^\d+$/.test(raw)) throw new Error(`${name} must be an integer`);
	const value = Number(raw ?? fallback);
	if (!Number.isInteger(value) || value < min || value > 0x7fffffff) {
		throw new Error(`${name} must be an integer between ${min} and 2147483647`);
	}
	return value;
}

test(
	"readers match the logical track",
	async () => {
		const seed = setting("MOQ_RESUME_SEED", 0x6d6f71, 0);
		const numRuns = setting("MOQ_RESUME_CASES", 64, 1);
		await fc.assert(fc.asyncProperty(cases, run), { seed, numRuns });
	},
	{ timeout: 3_600_000 },
);

/** Failures the sweep found, as it shrank them. */
test("regressions", async () => {
	const spec = (ordered: boolean, budget: Budget, range: Range, drops: boolean): Spec => ({
		ordered,
		budget,
		range,
		drops,
	});
	const regressions: Case[] = [
		// Found by the Rust run. An ordered reader whose range a finished track already passed ends.
		{ ops: [{ kind: "join", reader: 0, spec: spec(true, "hour", [0, 1], false) }], finish: true },
		// An ordered reader ends once the rest of its range was given up.
		{
			ops: [
				{ kind: "up", epoch: 1, slot: 2 },
				{ kind: "join", reader: 0, spec: spec(true, "hour", [0, 8], false) },
				{ kind: "giveUp", epoch: 1, slot: 2, group: 7 },
			],
			finish: true,
		},
		// Local bounds below the subscription's range leave no drops to report.
		{
			ops: [
				{ kind: "join", reader: 0, spec: spec(false, "hour", [11, 12], true) },
				{ kind: "write", epoch: 0, slot: 0, group: undefined, frames: 1 },
				{ kind: "bounds", reader: 0, range: [0, 1] },
			],
			finish: false,
		},
		// A drop held for a replaced copy is reported once that copy only backs a read.
		{
			ops: [
				{ kind: "join", reader: 0, spec: spec(false, "short", undefined, true) },
				{ kind: "write", epoch: 0, slot: 0, group: undefined, frames: 1 },
				{ kind: "up", epoch: 0, slot: 2 },
				{ kind: "write", epoch: 0, slot: 2, group: 6, frames: 1 },
			],
			finish: false,
		},
		// A group whose route failed continues on the next route once it lands there, though
		// that route refused to fetch it first.
		{
			ops: [
				{ kind: "up", epoch: 0, slot: 1 },
				{ kind: "join", reader: 1, spec: spec(false, "hour", undefined, false) },
				{ kind: "write", epoch: 0, slot: 1, group: 1, frames: 1 },
				{ kind: "write", epoch: 0, slot: 0, group: 2, frames: 1 },
				{ kind: "down", epoch: 0, slot: 1 },
			],
			finish: false,
		},
		// A group the replaced route failed is still continued once its cache let go of it.
		{
			ops: [
				{ kind: "join", reader: 0, spec: spec(false, "hour", undefined, false) },
				{ kind: "up", epoch: 0, slot: 2 },
				{ kind: "write", epoch: 0, slot: 2, group: 4, frames: 1 },
				{ kind: "down", epoch: 0, slot: 2 },
				{ kind: "expire" },
				{ kind: "write", epoch: 0, slot: 0, group: undefined, frames: 1 },
			],
			finish: false,
		},
		// A group a route gave up and the next route carries is handed out after its drop.
		{
			ops: [
				{ kind: "up", epoch: 1, slot: 0 },
				{ kind: "up", epoch: 1, slot: 2 },
				{ kind: "join", reader: 0, spec: spec(false, "hour", undefined, true) },
				{ kind: "giveUp", epoch: 1, slot: 2, group: 0 },
				{ kind: "down", epoch: 1, slot: 2 },
			],
			finish: false,
		},
	];
	const failures: string[] = [];
	for (const [i, c] of regressions.entries()) {
		try {
			await run(c);
		} catch (err) {
			failures.push(`case ${i}: ${err}`);
		}
	}
	if (failures.length > 0) throw new Error(failures.join("\n"));
});
