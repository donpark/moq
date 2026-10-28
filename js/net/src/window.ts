/**
 * A subscriber's own ranges, resolved to the group sequences it reads, and the helpers that
 * translate ranges between the model and the wire. Mirrors the Rust `subscription` module.
 *
 * The aggregate a publisher sees is the union of every subscriber's ranges, so it says what
 * arrives, not what one subscriber asked for. A {@link Window} is the per-subscriber side:
 * resolved at the hop serving the subscriber, it filters that subscriber's reads.
 *
 * @module
 */

import type { Anchor, Range } from "./track.ts";

/** A run of group sequences `[start, end)`; an open run ends at infinity. */
export type Run = { start: number; end: number };

/** One range resolved to groups: from `start` up to the exclusive `end`, undefined while open. */
export type Resolved = { start: number; end?: number };

/** Everything from the latest group onward. */
export const LIVE: Range = Object.freeze({ start: Object.freeze({ relative: 0 }) });

/** Check one anchor's shape, naming `field` in the error. */
function anchorOf(field: string, anchor: Anchor): Anchor {
	const keys = typeof anchor === "object" && anchor !== null ? Object.keys(anchor) : [];
	const value = keys.length === 1 ? (anchor as Record<string, unknown>)[keys[0]] : undefined;
	if ((keys[0] !== "absolute" && keys[0] !== "relative") || !Number.isSafeInteger(value) || (value as number) < 0) {
		throw new TypeError(`${field} must be {absolute} or {relative}, a non-negative safe integer`);
	}
	return keys[0] === "absolute" ? { absolute: value as number } : { relative: value as number };
}

/** Check and copy a caller's ranges, refusing a malformed one rather than serving it widened. */
export function rangesOf(ranges: readonly Range[]): Range[] {
	if (!Array.isArray(ranges)) throw new TypeError("ranges must be an array");
	return ranges.map((range) => {
		if (typeof range !== "object" || range === null) throw new TypeError("a range must be an object");
		const out: Range = {};
		if (range.start !== undefined) out.start = anchorOf("range start", range.start);
		if (range.end !== undefined) out.end = anchorOf("range end", range.end);
		return out;
	});
}

/** The exclusive end of an absolute anchor, or undefined for any other end. */
function absolute(anchor: Anchor | undefined): number | undefined {
	return anchor?.absolute;
}

/** How many groups back a relative anchor counts, or undefined for any other anchor. */
function relative(anchor: Anchor | undefined): number | undefined {
	return anchor?.relative;
}

/** Whether `range` certainly delivers nothing, wherever the latest group lands. */
export function isEmpty(range: Range): boolean {
	const end = absolute(range.end);
	if (end === undefined) return false;
	// Nothing sorts below group 0, whatever the start.
	if (end === 0) return true;
	// Nothing sorts below group 0, so the oldest start floors there.
	const start = range.start === undefined ? 0 : absolute(range.start);
	return start !== undefined && end <= start;
}

/** A canonical sort key: absolute starts first, then relative ones, then the oldest. */
function rank(range: Range): [number, number, number] {
	const end = absolute(range.end) ?? relative(range.end);
	const endKey = range.end === undefined ? Number.POSITIVE_INFINITY : (end ?? 0);
	const start = absolute(range.start);
	if (start !== undefined) return [0, start, endKey];
	const back = relative(range.start);
	return back === undefined ? [2, 0, endKey] : [1, back, endKey];
}

/**
 * The union of `ranges` in a canonical order, without empty or redundant ranges.
 *
 * Absolute spans merge where they overlap or touch. The rest can't be placed without the
 * latest group, so only an open range drops what it certainly covers: the open absolute span
 * drops ranges starting inside it, and the farthest open relative range drops relative ranges
 * starting no farther back. The oldest start covers everything. Mirrors the Rust `union`.
 */
export function union(ranges: readonly Range[]): Range[] {
	const kept = ranges.filter((range) => !isEmpty(range));
	if (kept.some((range) => range.start === undefined && range.end === undefined)) return [{}];

	const spans: Run[] = [];
	for (const range of kept) {
		const start = absolute(range.start);
		if (start === undefined) continue;
		if (range.end === undefined) spans.push({ start, end: Number.POSITIVE_INFINITY });
		else {
			const end = absolute(range.end);
			if (end !== undefined) spans.push({ start, end });
		}
	}
	spans.sort((a, b) => a.start - b.start || a.end - b.end);
	const runs: Run[] = [];
	for (const span of spans) {
		const last = runs.at(-1);
		if (last && span.start <= last.end) last.end = Math.max(last.end, span.end);
		else runs.push({ ...span });
	}
	const tail = runs.at(-1)?.end === Number.POSITIVE_INFINITY ? runs.at(-1)?.start : undefined;
	let farthest: number | undefined;
	for (const range of kept) {
		const back = relative(range.start);
		if (back !== undefined && range.end === undefined) farthest = Math.max(farthest ?? back, back);
	}

	const out: Range[] = runs.map((run) =>
		run.end === Number.POSITIVE_INFINITY
			? { start: { absolute: run.start } }
			: { start: { absolute: run.start }, end: { absolute: run.end } },
	);
	for (const range of kept) {
		const start = absolute(range.start);
		const back = relative(range.start);
		if (start !== undefined) {
			// Merged above unless it ends relative to the latest group.
			if (range.end === undefined || absolute(range.end) !== undefined) continue;
			if (tail === undefined || start < tail) out.push(range);
		} else if (back !== undefined) {
			if (range.end === undefined) continue;
			if (farthest === undefined || back > farthest) out.push(range);
		} else out.push(range);
	}
	if (farthest !== undefined) out.push({ start: { relative: farthest } });

	out.sort((a, b) => {
		const [x, y] = [rank(a), rank(b)];
		return x[0] - y[0] || x[1] - y[1] || x[2] - y[2];
	});
	return out.filter((range, index) => index === 0 || JSON.stringify(range) !== JSON.stringify(out[index - 1]));
}

/**
 * The lowest absolute group any range asks for, or undefined when one counts back from the
 * latest group or reaches back to the oldest one: the floor a wire without ranges carries.
 */
export function rangesStart(ranges: readonly Range[]): number | undefined {
	let floor: number | undefined;
	for (const range of ranges) {
		const start = absolute(range.start);
		if (start === undefined) return undefined;
		floor = Math.min(floor ?? start, start);
	}
	return floor;
}

/**
 * The exclusive end past every range, or undefined when one is open or ends relative to the
 * latest group: the cap a wire without ranges carries. No range at all ends at group 0.
 */
export function rangesEnd(ranges: readonly Range[]): number | undefined {
	let end = 0;
	for (const range of ranges) {
		const next = absolute(range.end);
		if (next === undefined) return undefined;
		end = Math.max(end, next);
	}
	return end;
}

/**
 * Whether a wire without ranges carries `range` upstream: absolute anchors, and the live and
 * oldest starts it spells with no floor. Mirrors the Rust `Range::is_wire`.
 */
export function isWire(range: Range): boolean {
	const back = relative(range.start);
	return (back === undefined || back === 0) && relative(range.end) === undefined;
}

/**
 * The ranges a SUBSCRIBE carries for `ranges`, without the empty ones. Mirrors the Rust
 * `wire_ranges`.
 *
 * The copy resolves a relative end itself, against the groups it receives past it, so it asks
 * through the live edge: SUBSCRIBE_START names a resolved start, but nothing names a resolved
 * end, so one resolved upstream would leave the copy short.
 */
export function wireRanges(ranges: readonly Range[]): Range[] {
	return ranges
		.filter((range) => !isEmpty(range))
		.map((range) => (relative(range.end) === undefined ? range : range.start ? { start: range.start } : {}));
}

/** Whether every range has an end, so a lite-07 publisher ends the subscription once it covers them. */
export function bounded(ranges: readonly Range[]): boolean {
	return ranges.every((range) => range.end !== undefined);
}

/**
 * Whether a subscriber joins at the live edge: a range counting back from the latest group (or
 * reaching back to the oldest) and following it. A fixed start resumes a known group instead,
 * such as a route switch picking up where the reader stopped.
 */
export function joinsLive(ranges: readonly Range[] | undefined): boolean {
	return (ranges ?? [{}]).some((range) => absolute(range.start) === undefined && range.end === undefined);
}

/**
 * The one span a wire without ranges carries for `ranges`, which the subscribers' own reads
 * narrow again: from the lowest absolute start up to {@link rangesEnd}.
 *
 * A subscription counting only back from the latest group asks with no start, which a pre-06
 * publisher reads as the latest group and a lite-06 one as no floor. One mixing the two asks
 * from group 0, since no single start covers both: the latest group may sit below the absolute
 * start. Mirrors the Rust `wire_span`.
 */
export function wireSpan(ranges: readonly Range[]): { start?: number; end?: number } {
	let start = rangesStart(ranges);
	if (start === undefined && ranges.some((range) => absolute(range.start) !== undefined)) start = 0;
	return { start, end: rangesEnd(ranges) };
}

/**
 * `ranges` with every range raised to start no earlier than `floor` and stop no later than the
 * exclusive `end`, without ever widening one. Mirrors the Rust `Subscription::clamped`.
 *
 * `resolved` names where each range resolved, by index, so a relative anchor becomes its own
 * group before it is clamped. A relative start not resolved keeps its anchor, since the floor
 * may lie before it, so the caller drops what it resolves to below `floor`. The oldest start
 * never lies above the floor, so it becomes the floor.
 */
export function clamped(
	ranges: readonly Range[],
	floor: number | undefined,
	end: number | undefined,
	resolved: readonly (Resolved | undefined)[],
): Range[] {
	if (floor === undefined && end === undefined) return [...ranges];
	return ranges.map((range, index) => {
		const at = resolved[index];
		let start = range.start;
		if (at && relative(start) !== undefined) start = { absolute: at.start };
		let requested = range.end;
		if (at?.end !== undefined && relative(requested) !== undefined) requested = { absolute: at.end };
		const named = absolute(start);
		if (floor !== undefined && named !== undefined) start = { absolute: Math.max(named, floor) };
		else if (floor !== undefined && start === undefined) start = { absolute: floor };
		if (end !== undefined) {
			const capped = absolute(requested);
			requested = { absolute: capped === undefined ? end : Math.min(capped, end) };
		}
		const out: Range = {};
		if (start !== undefined) out.start = start;
		if (requested !== undefined) out.end = requested;
		return out;
	});
}

/** One non-empty range, resolved. */
type Span = {
	/** Which of the ranges it came from. */
	index: number;
	start: number;
	/** The exclusive end, infinity while open. */
	end: number;
	/** Whether the start names a group, rather than reaching back to the oldest one. */
	floored: boolean;
};

/**
 * A subscriber's ranges resolved to the group sequences it reads. Mirrors the Rust `Window`,
 * without time anchors.
 *
 * A relative anchor resolves when the window is built, counting back from the latest group,
 * and again only when the ranges are replaced. Before any group exists it waits, reading
 * everything, until {@link settle} names the first one.
 */
export class Window {
	/** What these were resolved from, so only replacing the ranges resolves again. */
	readonly ranges: readonly Range[];
	#spans: Span[] = [];
	/** The groups the spans cover, merged, sorted, and disjoint. */
	#runs: Run[] = [];
	/** A relative anchor waits on a first group, so nothing is filtered yet. */
	#pending = false;

	/** Resolve `ranges`, counting relative anchors back from `latest`. */
	constructor(ranges: readonly Range[], latest: number | undefined) {
		this.ranges = ranges;
		this.#place(latest);
	}

	/** Whether a relative anchor still waits on a first group. */
	get pending(): boolean {
		return this.#pending;
	}

	/** Resolve a waiting relative anchor once `latest` exists. Returns whether the groups in range moved. */
	settle(latest: number | undefined): boolean {
		if (!this.#pending || latest === undefined) return false;
		this.#place(latest);
		return true;
	}

	/** Whether group `sequence` is in range. Everything is while a relative anchor waits. */
	contains(sequence: number): boolean {
		if (this.#pending) return true;
		const run = this.#runAt(sequence);
		return run !== undefined && sequence >= run.start;
	}

	/** The lowest group in range at or above `from`, undefined when none is. `from` itself while a relative anchor waits. */
	next(from: number): number | undefined {
		if (this.#pending) return from;
		const run = this.#runAt(from);
		return run === undefined ? undefined : Math.max(run.start, from);
	}

	/** The lowest group in range, undefined while a relative anchor waits. Nothing in range floors at group 0. */
	floor(): number | undefined {
		if (this.#pending) return undefined;
		return this.#runs[0]?.start ?? 0;
	}

	/**
	 * The lowest group the ranges name, undefined while one reaches back to the oldest group or
	 * a relative anchor waits on a first group.
	 */
	start(): number | undefined {
		if (this.#pending || this.#spans.some((span) => !span.floored)) return undefined;
		let start: number | undefined;
		for (const span of this.#spans) start = Math.min(start ?? span.start, span.start);
		return start;
	}

	/** The exclusive end past every group in range, undefined when one stays open. Nothing in range ends at 0. */
	end(): number | undefined {
		if (this.#pending) return undefined;
		const end = this.#runs.at(-1)?.end ?? 0;
		return end === Number.POSITIVE_INFINITY ? undefined : end;
	}

	/** The runs in range from `from` up to the exclusive `end`, lowest first. */
	runs(from = 0, end = Number.POSITIVE_INFINITY): Run[] {
		if (from >= end) return [];
		if (this.#pending) return [{ start: from, end }];
		const out: Run[] = [];
		for (const run of this.#runs) {
			if (run.end <= from) continue;
			if (run.start >= end) break;
			out.push({ start: Math.max(run.start, from), end: Math.min(run.end, end) });
		}
		return out;
	}

	/** Each range resolved, in the order given: undefined for one that is empty or still waits on a first group. */
	resolved(): (Resolved | undefined)[] {
		const resolved: (Resolved | undefined)[] = this.ranges.map(() => undefined);
		for (const span of this.#spans) {
			resolved[span.index] = {
				start: span.start,
				end: span.end === Number.POSITIVE_INFINITY ? undefined : span.end,
			};
		}
		return resolved;
	}

	// The first run ending past `sequence`.
	#runAt(sequence: number): Run | undefined {
		let lo = 0;
		let hi = this.#runs.length;
		while (lo < hi) {
			const mid = (lo + hi) >>> 1;
			if (this.#runs[mid].end <= sequence) lo = mid + 1;
			else hi = mid;
		}
		return this.#runs[lo];
	}

	#place(latest: number | undefined): void {
		this.#spans = [];
		this.#pending = false;
		this.ranges.forEach((range, index) => {
			if (isEmpty(range)) return;
			const start = startOf(range.start, latest);
			const end = endOf(range.end, latest);
			if (start === undefined || end === undefined) {
				this.#pending = true;
				return;
			}
			this.#spans.push({ index, start: start.group, end, floored: start.floored });
		});
		const spans = this.#spans.filter((span) => span.end > span.start).sort((a, b) => a.start - b.start);
		this.#runs = [];
		for (const span of spans) {
			const last = this.#runs.at(-1);
			if (last && span.start <= last.end) last.end = Math.max(last.end, span.end);
			else this.#runs.push({ start: span.start, end: span.end });
		}
	}
}

/**
 * The first group a range starts at, and whether it names that group rather than reaching back
 * to the oldest one. Undefined while it waits on a latest group.
 */
function startOf(
	anchor: Anchor | undefined,
	latest: number | undefined,
): { group: number; floored: boolean } | undefined {
	const group = absolute(anchor);
	if (group !== undefined) return { group, floored: true };
	const back = relative(anchor);
	if (back === undefined) return { group: 0, floored: false };
	if (latest === undefined) return undefined;
	return { group: Math.max(0, latest - back), floored: true };
}

/** Where a range stops, exclusive and infinity while open, or undefined while it waits on a latest group. */
function endOf(anchor: Anchor | undefined, latest: number | undefined): number | undefined {
	const end = absolute(anchor);
	if (end !== undefined) return end;
	const back = relative(anchor);
	if (back === undefined) return Number.POSITIVE_INFINITY;
	if (latest === undefined) return undefined;
	return Math.max(0, latest - back) + 1;
}
