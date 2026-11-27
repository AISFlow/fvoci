/**
 * What a Gantt view keeps from one working-out of its chart to the next
 * (features/views/gantt/kept.ts): the same object for what says the same.
 */
import { describe, expect, it } from 'vitest';

import { Kept, same } from '$lib/features/views/gantt/kept.ts';

interface Line {
  id: string;
  title: string;
  bar: { x: number; width: number };
}

const line = (id: string, title: string, x: number): Line => ({ id, title, bar: { x, width: 2 } });

describe('saying the same', () => {
  it('is equal values, and objects whose members say the same, however deep', () => {
    expect(same(1, 1)).toBe(true);
    expect(same('a', 'b')).toBe(false);
    expect(same(line('a', 'A', 1), line('a', 'A', 1))).toBe(true);
    expect(same(line('a', 'A', 1), line('a', 'A', 2))).toBe(false);
    expect(same(line('a', 'A', 1), line('a', 'B', 1))).toBe(false);
  });

  it('is not an object with a member more or less, or an object and nothing', () => {
    expect(same({ a: 1 }, { a: 1, b: undefined })).toBe(false);
    expect(same({ a: 1, b: 2 }, { a: 1 })).toBe(false);
    expect(same({ a: undefined }, { b: 1 })).toBe(false);
    expect(same({ a: 1 }, null)).toBe(false);
    expect(same(undefined, { a: 1 })).toBe(false);
  });
});

describe('what is kept', () => {
  it('is the object of last time where it says the same, and the new one where it does not', () => {
    const kept = new Kept<Line>();
    const first = kept.keep([line('a', 'A', 1), line('b', 'B', 4)], (each) => each.id);
    const second = kept.keep([line('a', 'A', 1), line('b', 'B', 5)], (each) => each.id);
    expect(second[0]).toBe(first[0]);
    expect(second[1]).not.toBe(first[1]);
    expect(second[1]?.bar.x).toBe(5);
    // The changed one is what is kept from now on.
    const third = kept.keep([line('b', 'B', 5), line('a', 'A', 1)], (each) => each.id);
    expect(third[0]).toBe(second[1]);
    expect(third[1]).toBe(first[0]);
  });

  it('forgets what was not there last time', () => {
    const kept = new Kept<Line>();
    const [first] = kept.keep([line('a', 'A', 1)], (each) => each.id);
    kept.keep([], (each) => each.id);
    const [again] = kept.keep([line('a', 'A', 1)], (each) => each.id);
    expect(again).not.toBe(first);
    expect(again).toEqual(first);
  });
});
