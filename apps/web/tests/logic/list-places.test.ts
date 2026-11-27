/**
 * Places of a list put in order by hand: the server's orders of a wiki's pages
 * (crates/model/src/page.rs, the same cases), and the writes that move one
 * thing of a list, give a copy its place and a new thing the last, with
 * those that keep the others where they show.
 */
import { describe, expect, it } from 'vitest';

import {
  isPlace,
  movedTo,
  placeAfter,
  placeAtEnd,
  placeBetween,
  spreadPlaces,
  type Placed,
  type PlaceWrite,
} from '$lib/logic/list-places.ts';

const between = (before?: string, after?: string): string => {
  const made = placeBetween(before, after);
  if (made === undefined) throw new Error(`no place between ${before} and ${after}`);
  return made;
};

/** A sequence of numbers that is the same in every run (xorshift, as the server's test). */
function sequence(seed: bigint): (limit: number) => number {
  let state = seed;
  const mask = (1n << 64n) - 1n;
  return (limit) => {
    state ^= (state << 13n) & mask;
    state ^= state >> 7n;
    state ^= (state << 17n) & mask;
    return Number(state % BigInt(limit));
  };
}

describe('a place between two', () => {
  it('leaves room on both sides of the first', () => {
    const first = between();
    expect(first).toBe('V');
    expect(between(undefined, first) < first).toBe(true);
    expect(between(first, undefined) > first).toBe(true);
  });

  it('is between the two wherever it is asked for', () => {
    const below = sequence(0x2545f4914f6cdd1dn);
    const places: string[] = [];
    for (let turn = 0; turn < 3000; turn += 1) {
      const at = below(places.length + 1);
      const before = places[at - 1];
      const after = places[at];
      const made = between(before, after);
      expect(before === undefined || before < made).toBe(true);
      expect(after === undefined || made < after).toBe(true);
      expect(isPlace(made)).toBe(true);
      places.splice(at, 0, made);
    }
    expect([...places].sort()).toEqual(places);
  });

  it('is found between two next to each other, as the server finds it', () => {
    for (const [before, after] of [
      ['1', '2'],
      ['V', 'V1'],
      ['Vz', 'W'],
      ['01', '1'],
      ['z', 'z1'],
    ] as const) {
      const made = between(before, after);
      expect(before < made && made < after).toBe(true);
    }
    expect(between(undefined, '1')).toBe('0z');
    expect(between(undefined, '01')).toBe('00z');
    expect(between('V', 'W')).toBe('VV');
    expect(between('z', undefined)).toBe('z1');
  });

  it('stays short for a thousand added at one end', () => {
    let [last, first] = [between(), between()];
    for (let turn = 0; turn < 1000; turn += 1) {
      last = between(last, undefined);
      first = between(undefined, first);
    }
    expect(last.length).toBeLessThanOrEqual(20);
    expect(first.length).toBeLessThanOrEqual(20);
  });

  it('is none for what is no place, or for two not in order', () => {
    for (const malformed of ['', 'V0', 'a-b', 'ö']) {
      expect(isPlace(malformed)).toBe(false);
      expect(placeBetween(malformed, undefined)).toBeUndefined();
      expect(placeBetween(undefined, malformed)).toBeUndefined();
    }
    expect(isPlace(3)).toBe(false);
    expect(placeBetween('V', 'V')).toBeUndefined();
    expect(placeBetween('W', 'V')).toBeUndefined();
  });

  it('spreads a count of places short and evenly, with room between and at the ends', () => {
    for (const count of [1, 2, 5, 61, 62, 200]) {
      const spread = spreadPlaces(count);
      expect(spread).toHaveLength(count);
      expect(spread.every((place) => isPlace(place))).toBe(true);
      expect([...spread].sort()).toEqual(spread);
      expect(new Set(spread).size).toBe(count);
      expect(placeBetween(undefined, spread[0])).toBeDefined();
      expect(placeBetween(spread.at(-1), undefined)).toBeDefined();
    }
    expect(spreadPlaces(1)).toEqual(['V']);
    expect(spreadPlaces(200).every((place) => place.length <= 2)).toBe(true);
  });
});

const list = (...items: [string, string?][]): Placed[] =>
  items.map(([id, place]) => ({ id, place }));

/** The list as it reads once `writes` are made, by place and then identifier. */
function readAfter(shown: readonly Placed[], writes: readonly PlaceWrite[]): string[] {
  const after = new Map(shown.map((item) => [item.id, item.place]));
  for (const { id, place } of writes) after.set(id, place);
  return [...after]
    .sort(([idA, a = ''], [idB, b = '']) => (a < b ? -1 : a > b ? 1 : idA < idB ? -1 : 1))
    .map(([id]) => id);
}

describe('moving one thing of a list', () => {
  it('writes the thing moved alone where every one has a place', () => {
    const shown = list(['a', 'V'], ['b', 'W'], ['c', 'X']);
    expect(movedTo(shown, 'c', 1)).toEqual({ place: 'VV', keep: [], was: 'X' });
    expect(movedTo(shown, 'a', 1)).toEqual({ place: 'WV', keep: [], was: 'V' });
    expect(movedTo(shown, 'b', 0)).toEqual({ place: 'U', keep: [], was: 'W' });
    expect(movedTo(shown, 'b', 2)).toEqual({ place: 'Y', keep: [], was: 'W' });
  });

  it('gives those without a place the places they show at, the first time, and says its own', () => {
    // Nobody moved anything yet: a, b, c, d by title.
    const shown = list(['a'], ['b'], ['c'], ['d']);
    expect(movedTo(shown, 'c', 1)).toEqual({
      place: 'VV',
      keep: [
        { id: 'a', place: 'V' },
        { id: 'b', place: 'W' },
        { id: 'd', place: 'Y' },
      ],
      // Where it showed: what an undo puts back.
      was: 'X',
    });
    // Some placed, some not: those after the placed ones go on from the last of them.
    expect(movedTo(list(['p', 'V'], ['a'], ['b']), 'p', 1)).toEqual({
      place: 'WV',
      keep: [
        { id: 'a', place: 'W' },
        { id: 'b', place: 'X' },
      ],
      was: 'V',
    });
  });

  it('gives the same places for the same list, whoever moves what', () => {
    const shown = list(['a'], ['b'], ['c'], ['d']);
    const mine = movedTo(shown, 'c', 1);
    const theirs = movedTo(shown, 'a', 3);
    const placeOf = (writes: PlaceWrite[], id: string) =>
      writes.find((write) => write.id === id)?.place;
    // b is moved by neither: both write it the same place.
    expect(placeOf(mine.keep, 'b')).toBe(placeOf(theirs.keep, 'b'));
    expect(placeOf(mine.keep, 'd')).toBe('Y');
    expect(placeOf(theirs.keep, 'd')).toBe('Y');
  });

  it('raises the tie it goes into, those of it after the gap alone', () => {
    // b and c have one place (two moved there at once): the identifier settles them.
    const shown = list(['a', 'V'], ['b', 'W'], ['c', 'W'], ['d', 'X']);
    const moved = movedTo(shown, 'd', 2);
    expect(moved.keep).toEqual([{ id: 'c', place: 'Y' }]);
    expect(readAfter(shown, [...moved.keep, { id: 'd', place: moved.place }])).toEqual([
      'a',
      'b',
      'd',
      'c',
    ]);
    // Three of one place, into the middle: the two after the gap go up, the first stays.
    const three = list(['a', 'W'], ['b', 'W'], ['c', 'W'], ['e', 'X']);
    const into = movedTo(three, 'e', 1);
    expect(into.keep.map((write) => write.id)).toEqual(['b', 'c']);
    expect(readAfter(three, [...into.keep, { id: 'e', place: into.place }])).toEqual([
      'a',
      'e',
      'b',
      'c',
    ]);
  });

  it('says where an undo puts it back, among the places the others are given', () => {
    // b and c tied, d after them: the tie goes up past where d is, and d into the gap.
    const shown = list(['a', 'W'], ['b', 'WV'], ['c', 'WV'], ['d', 'X'], ['e', 'Y']);
    const moved = movedTo(shown, 'd', 2);
    expect(moved.keep.map((write) => write.id)).toEqual(['c']);
    expect(readAfter(shown, [...moved.keep, { id: 'd', place: moved.place }])).toEqual([
      'a',
      'b',
      'd',
      'c',
      'e',
    ]);
    // Its write is the move, and so is a step of undo; undone, the list is as it was.
    expect(moved.was).not.toBe(moved.place);
    expect(readAfter(shown, [...moved.keep, { id: 'd', place: moved.was }])).toEqual([
      'a',
      'b',
      'c',
      'd',
      'e',
    ]);
  });

  it('numbers the list anew where the place between would be too long', () => {
    const narrow = `V${'0'.repeat(30)}1`;
    const shown = list(['a', 'V'], ['b', narrow], ['c', 'W']);
    const moved = movedTo(shown, 'c', 1);
    expect(moved.place.length).toBeLessThanOrEqual(2);
    expect(readAfter(shown, [...moved.keep, { id: 'c', place: moved.place }])).toEqual([
      'a',
      'c',
      'b',
    ]);
    // Undone, it is after b again, in the list numbered anew.
    expect(readAfter(shown, [...moved.keep, { id: 'c', place: moved.was }])).toEqual([
      'a',
      'b',
      'c',
    ]);
  });
});

describe('a copy and a new thing', () => {
  it('puts a copy right after its thing, the others kept where they show', () => {
    const shown = list(['a', 'V'], ['b', 'W'], ['c']);
    expect(placeAfter(shown, 'a')).toEqual({ place: 'VV', keep: [{ id: 'c', place: 'X' }] });
    // Before the next, though it has no place yet.
    expect(placeAfter(shown, 'b').place).toBe('WV');
    // After one without a place, which gets the one it shows at.
    expect(placeAfter(shown, 'c')).toEqual({ place: 'Y', keep: [{ id: 'c', place: 'X' }] });
    // Into a tie after it: the one of the tie after the copy goes up.
    expect(placeAfter(list(['a', 'V'], ['b', 'V']), 'a')).toEqual({
      place: 'W',
      keep: [{ id: 'b', place: 'X' }],
    });
  });

  it('puts a new thing last, those without a place given the places they show at', () => {
    expect(placeAtEnd(list(['a', 'V'], ['b', 'W']))).toEqual({ place: 'X', keep: [] });
    expect(placeAtEnd(list(['a'], ['b']))).toEqual({
      place: 'X',
      keep: [
        { id: 'a', place: 'V' },
        { id: 'b', place: 'W' },
      ],
    });
    expect(placeAtEnd([])).toEqual({ place: 'V', keep: [] });
  });
});
