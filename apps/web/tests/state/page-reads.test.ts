import { describe, expect, it } from 'vitest';

import { allPages, firstOf, MAX_PER_PAGE, pagesTogether } from '../../src/lib/state/page-reads.ts';
import type { Paging } from '../../src/lib/state/trash-reads.ts';

/** A list of `length` numbers read a page at a time, and the pages asked for. */
function list(length: number): { read: (paging: Paging) => Promise<number[]>; asked: Paging[] } {
  const items = Array.from({ length }, (_, index) => index);
  const asked: Paging[] = [];
  const read = (paging: Paging): Promise<number[]> => {
    asked.push(paging);
    const from = paging.page * paging.perPage;
    return Promise.resolve(items.slice(from, from + paging.perPage));
  };
  return { read, asked };
}

describe('reading a list in pages', () => {
  it('reads every item of a list of more than a page, a page of 1000 at a time', async () => {
    const { read, asked } = list(2 * MAX_PER_PAGE + 1);
    const items = await allPages(read);
    expect(items).toHaveLength(2001);
    expect(items.at(-1)).toBe(2000);
    expect(asked).toEqual([
      { page: 0, perPage: 1000 },
      { page: 1, perPage: 1000 },
      { page: 2, perPage: 1000 },
    ]);
  });

  it('asks once more when the last page is full, and stops at the empty one', async () => {
    const { read, asked } = list(MAX_PER_PAGE);
    expect(await allPages(read)).toHaveLength(1000);
    expect(asked).toEqual([
      { page: 0, perPage: 1000 },
      { page: 1, perPage: 1000 },
    ]);
  });

  it('reads the first items in one page when they fit, and in pages of 1000 when not', async () => {
    const small = list(80);
    expect(await firstOf(small.read, 50)).toHaveLength(50);
    expect(small.asked).toEqual([{ page: 0, perPage: 50 }]);
    const large = list(1500);
    const items = await firstOf(large.read, 1200);
    expect(items).toEqual(Array.from({ length: 1200 }, (_, index) => index));
    expect(large.asked).toEqual([
      { page: 0, perPage: 1000 },
      { page: 1, perPage: 1000 },
    ]);
    const none = list(5);
    expect(await firstOf(none.read, 0)).toEqual([]);
    expect(none.asked).toEqual([]);
  });
});

/**
 * A list of `length` items (`{ id }`) read a page at a time, answered a turn
 * later; the pages asked for, in rounds: each round the pages asked before
 * any of them was answered.
 */
function slowList(items: () => { id: string }[]) {
  const rounds: number[][] = [];
  let answering = false;
  const read = async (paging: Paging): Promise<{ id: string }[]> => {
    if (rounds.length === 0 || answering) {
      rounds.push([]);
      answering = false;
    }
    rounds.at(-1)?.push(paging.page);
    expect(paging.perPage).toBe(MAX_PER_PAGE);
    const from = paging.page * paging.perPage;
    await new Promise((resolve) => setTimeout(resolve, 0));
    answering = true;
    return items().slice(from, from + paging.perPage);
  };
  return { read, rounds };
}

const ids = (length: number, prefix = 'c') =>
  Array.from({ length }, (_, index) => ({ id: `${prefix}${String(index)}` }));

describe('reading the pages of a long list together', () => {
  it('reads a list of one page once', async () => {
    const { read, rounds } = slowList(() => ids(80));
    expect(await pagesTogether(read)).toHaveLength(80);
    expect(rounds).toEqual([[0]]);
  });

  it('asks for the next two pages together after a full first one, then twice as many', async () => {
    const two = slowList(() => ids(2000));
    const items = await pagesTogether(two.read);
    expect(items.map((item) => item.id)).toEqual(ids(2000).map((item) => item.id));
    // Two round trips for 2000 cards, where page after page took three.
    expect(two.rounds).toEqual([[0], [1, 2]]);

    const exactly = slowList(() => ids(MAX_PER_PAGE));
    expect(await pagesTogether(exactly.read)).toHaveLength(1000);
    expect(exactly.rounds).toEqual([[0], [1, 2]]);

    const many = slowList(() => ids(5500));
    const all = await pagesTogether(many.read);
    expect(all.map((item) => item.id)).toEqual(ids(5500).map((item) => item.id));
    expect(many.rounds).toEqual([[0], [1, 2], [3, 4, 5, 6]]);
  });

  it('keeps an item once that moved to a later page while the pages were read', async () => {
    // A card older than the first page's last one is restored after the first page was read:
    // the rest moves one place on, and the first page's last card is on the second too. (The
    // restored card itself comes with the board's change messages, as page after page.)
    let list = ids(1500);
    const { read } = slowList(() => list);
    const reading = pagesTogether(read);
    await new Promise((resolve) => setTimeout(resolve, 0));
    list = [{ id: 'restored' }, ...list];
    const items = await reading;
    expect(items.map((item) => item.id)).toEqual(ids(1500).map((item) => item.id));
  });
});
