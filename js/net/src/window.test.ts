import { expect, test } from "bun:test";
import type { Range } from "./track.ts";
import { clamped, joinsLive, rangesOf, union, Window, wireRanges, wireSpan } from "./window.ts";

const abs = (start: number, end?: number): Range =>
	end === undefined ? { start: { absolute: start } } : { start: { absolute: start }, end: { absolute: end } };

test("union merges absolute spans and drops what an open range covers", () => {
	expect(union([abs(4, 6), abs(0, 2), abs(2, 3), abs(5, 9)])).toEqual([abs(0, 3), abs(4, 9)]);
	// The open absolute span drops a relative-ended range starting inside it.
	expect(union([abs(3), { start: { absolute: 5 }, end: { relative: 0 } }])).toEqual([abs(3)]);
	// The farthest open relative range drops relative ranges starting no farther back.
	expect(
		union([
			{ start: { relative: 2 } },
			{ start: { relative: 5 } },
			{ start: { relative: 1 }, end: { relative: 0 } },
		]),
	).toEqual([{ start: { relative: 5 } }]);
	// A relative range above the live edge never hides an absolute one, nor the reverse.
	expect(union([abs(10), { start: { relative: 0 } }])).toEqual([abs(10), { start: { relative: 0 } }]);
	// The oldest start covers everything, and empty ranges vanish.
	expect(union([abs(2, 4), {}])).toEqual([{}]);
	expect(union([abs(4, 4), { end: { absolute: 0 } }])).toEqual([]);
});

test("a window reads only its ranges, counting relative anchors back from the latest group", () => {
	const window = new Window(
		[abs(2, 4), { start: { relative: 1 } }, { start: { absolute: 0 }, end: { relative: 8 } }],
		10,
	);
	expect([0, 1, 2, 3, 4, 8, 9, 10, 11].map((sequence) => window.contains(sequence))).toEqual([
		true,
		true,
		true,
		true,
		false,
		false,
		true,
		true,
		true,
	]);
	expect(window.floor()).toBe(0);
	expect(window.start()).toBe(0);
	expect(window.end()).toBeUndefined();
	expect(window.next(4)).toBe(9);
	expect(window.runs(1, 12)).toEqual([
		{ start: 1, end: 4 },
		{ start: 9, end: 12 },
	]);
	expect(window.resolved()).toEqual([{ start: 2, end: 4 }, { start: 9 }, { start: 0, end: 3 }]);
});

test("a relative anchor clamps to group 0 and waits for a first group", () => {
	expect(new Window([{ start: { relative: 50 }, end: { relative: 40 } }], 10).runs()).toEqual([{ start: 0, end: 1 }]);

	const window = new Window([{ start: { relative: 0 } }], undefined);
	expect(window.pending).toBe(true);
	expect(window.contains(3)).toBe(true);
	expect(window.floor()).toBeUndefined();
	expect(window.settle(7)).toBe(true);
	expect(window.pending).toBe(false);
	expect(window.contains(6)).toBe(false);
	expect(window.floor()).toBe(7);
	// Resolved once: a later group never moves it.
	expect(window.settle(9)).toBe(false);
});

test("a bounded window ends past its last group, and nothing in range ends at 0", () => {
	expect(new Window([abs(2, 4), abs(6, 9)], undefined).end()).toBe(9);
	expect(new Window([], undefined).end()).toBe(0);
	// The oldest start names no floor, so a wire without ranges asks with none.
	expect(new Window([{ end: { absolute: 5 } }], undefined).start()).toBeUndefined();
});

test("a wire without ranges carries one span covering them all", () => {
	expect(wireSpan([abs(4, 6), abs(2, 3)])).toEqual({ start: 2, end: 6 });
	expect(wireSpan([{ start: { relative: 0 } }])).toEqual({ start: undefined, end: undefined });
	// No single start covers both an absolute and a relative one.
	expect(wireSpan([abs(9), { start: { relative: 0 } }])).toEqual({ start: 0, end: undefined });
	// A relative end goes out open, since the copy resolves it against what it receives.
	expect(wireRanges([{ start: { relative: 3 }, end: { relative: 1 } }, abs(5, 5)])).toEqual([
		{ start: { relative: 3 } },
	]);
});

test("clamping raises starts to the floor and caps ends without widening", () => {
	const ranges: Range[] = [abs(1, 9), {}, { start: { relative: 2 }, end: { relative: 0 } }];
	expect(clamped(ranges, 4, 6, [undefined, undefined, { start: 8, end: 11 }])).toEqual([
		abs(4, 6),
		abs(4, 6),
		abs(8, 6),
	]);
	// A relative start not resolved keeps its anchor for the caller to floor.
	expect(clamped([{ start: { relative: 2 } }], 4, undefined, [])).toEqual([{ start: { relative: 2 } }]);
});

test("ranges are checked rather than served widened", () => {
	expect(() => rangesOf([{ start: { absolute: -1 } }])).toThrow(TypeError);
	expect(() => rangesOf([{ start: { absolute: 1, relative: 2 } as never }])).toThrow(TypeError);
	expect(() => rangesOf([{ end: { time: 5 } as never }])).toThrow(TypeError);
	expect(joinsLive([{}])).toBe(true);
	expect(joinsLive([abs(3)])).toBe(false);
});
