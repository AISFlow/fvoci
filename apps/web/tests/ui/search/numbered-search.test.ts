/**
 * The search with a card's number: where a board is shown, `#12` or `12`
 * finds that board's card first, and the words what else they find; a row
 * of a card shows its number.
 */
import { cleanup, render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { findNumbered, type NumberedSearch } from '$lib/features/search/numbered-search.ts';
import SearchHitRow from '$lib/features/search/SearchHitRow.svelte';
import type { SearchAsk, SearchHit } from '$lib/state/content-search.ts';
import type { LinkedBlock } from '$lib/state/references.ts';

const hit = (id: string, title = id): SearchHit => ({
  id,
  kind: 'card',
  containerId: 'b',
  containerKind: 'board',
  title,
  snippet: [],
});

const twelve: LinkedBlock = {
  id: 'c12',
  kind: 'card',
  title: 'Plan',
  icon: '',
  containerId: 'b',
  containerKind: 'board',
  number: 12,
};

function search(options: {
  board?: string;
  card?: LinkedBlock | Error;
  hits?: SearchHit[];
}): NumberedSearch & { asked: [string, number][] } {
  const asked: [string, number][] = [];
  return {
    asked,
    find: () => Promise.resolve(options.hits ?? []),
    numbered: (boardId, number) => {
      asked.push([boardId, number]);
      return options.card instanceof Error
        ? Promise.reject(options.card)
        : Promise.resolve(options.card);
    },
    boardShown: () => options.board,
  };
}

const ask = (query: string, more: Partial<SearchAsk> = {}): SearchAsk => ({
  query,
  limit: 20,
  ...more,
});

describe('a search with a number', () => {
  it('puts the card of the board shown first, and the words do not list it again', async () => {
    const given = search({ board: 'b', card: twelve, hits: [hit('x'), hit('c12', 'Plan')] });
    const found = await findNumbered(given, ask('#12'));
    expect(found.map((one) => one.id)).toEqual(['c12', 'x']);
    expect(found[0]).toEqual({ ...hit('c12', 'Plan'), number: 12 });
    expect(given.asked).toEqual([['b', 12]]);
    expect((await findNumbered(given, ask('12'))).map((one) => one.id)).toEqual(['c12', 'x']);
  });

  it('keeps to the number of hits asked for', async () => {
    const hits = [hit('x'), hit('y'), hit('z')];
    const found = await findNumbered(
      search({ board: 'b', card: twelve, hits }),
      ask('12', { limit: 3 }),
    );
    expect(found.map((one) => one.id)).toEqual(['c12', 'x', 'y']);
  });

  it('asks no number off a board, for pages, or for what is no number', async () => {
    for (const [given, asked] of [
      [search({ card: twelve, hits: [hit('x')] }), ask('#12')],
      [search({ board: 'b', card: twelve, hits: [hit('x')] }), ask('#12', { kind: 'page' })],
      [search({ board: 'b', card: twelve, hits: [hit('x')] }), ask('#12 plan')],
      [search({ board: 'b', card: twelve, hits: [hit('x')] }), ask('012')],
    ] as const) {
      expect((await findNumbered(given, asked)).map((one) => one.id)).toEqual(['x']);
      expect(given.asked).toEqual([]);
    }
  });

  it('answers with the words where the number names no card, or cannot be asked', async () => {
    const none = search({ board: 'b', hits: [hit('x')] });
    expect((await findNumbered(none, ask('#3'))).map((one) => one.id)).toEqual(['x']);
    const failing = search({ board: 'b', card: new Error('offline'), hits: [hit('x')] });
    expect((await findNumbered(failing, ask('#3'))).map((one) => one.id)).toEqual(['x']);
  });

  it('fails as the words fail', async () => {
    const given = {
      ...search({ board: 'b', card: twelve }),
      find: vi.fn(() => Promise.reject(new Error('down'))),
    };
    await expect(findNumbered(given, ask('#12'))).rejects.toThrow('down');
  });

  it('normalizes `#0012` and keeps the current board card ahead of other boards, preserving containerTitle', async () => {
    const otherBoardHit: SearchHit = {
      id: 'other-12',
      kind: 'card',
      containerId: 'other',
      containerKind: 'board',
      containerTitle: 'Other Board',
      title: 'Other Plan',
      number: 12,
      snippet: [],
    };
    const currentBoardHit: SearchHit = {
      id: 'c12',
      kind: 'card',
      containerId: 'b',
      containerKind: 'board',
      containerTitle: 'Current Board',
      title: 'Plan',
      number: 12,
      snippet: [],
    };
    const given = search({ board: 'b', card: twelve, hits: [otherBoardHit, currentBoardHit] });
    const found = await findNumbered(given, ask('#0012'));
    expect(found).toEqual([currentBoardHit, otherBoardHit]);
    expect(given.asked).toEqual([['b', 12]]);

    // Even if `numbered` fails, hits from the current board with matching number still sort first.
    const fallback = search({
      board: 'b',
      card: new Error('offline'),
      hits: [otherBoardHit, currentBoardHit],
    });
    expect((await findNumbered(fallback, ask('#12'))).map((one) => one.id)).toEqual([
      'c12',
      'other-12',
    ]);

    // Bare number query with no matching card on current board must NOT reorder unrelated hits on `b`.
    const unrelatedOnCurrent: SearchHit = {
      id: 'c3',
      kind: 'card',
      containerId: 'b',
      containerKind: 'board',
      containerTitle: 'Current Board',
      title: 'Mentions 2026',
      number: 3,
      snippet: [],
    };
    const higherOtherBoard: SearchHit = {
      id: 'other-top',
      kind: 'card',
      containerId: 'other',
      containerKind: 'board',
      containerTitle: 'Other Board',
      title: '2026 Roadmap',
      number: 1,
      snippet: [],
    };
    const bareFallback = search({
      board: 'b',
      hits: [higherOtherBoard, unrelatedOnCurrent],
    });
    expect((await findNumbered(bareFallback, ask('2026'))).map((one) => one.id)).toEqual([
      'other-top',
      'c3',
    ]);
  });

  it('returns cross-board `#12` hits when off a board without calling `numbered`', async () => {
    const otherBoardHit: SearchHit = {
      id: 'other-12',
      kind: 'card',
      containerId: 'other',
      containerKind: 'board',
      containerTitle: 'Other Board',
      title: 'Other Plan',
      number: 12,
      snippet: [],
    };
    const given = search({ hits: [otherBoardHit] });
    expect(await findNumbered(given, ask('#12'))).toEqual([otherBoardHit]);
    expect(given.asked).toEqual([]);
  });
});

describe('a row of a card that was found', () => {
  afterEach(() => {
    cleanup();
  });

  it('shows its number, and says it in words', () => {
    const { container } = render(SearchHitRow, {
      props: {
        hit: { ...hit('c12', 'Plan'), number: 12 },
        id: 'row',
        href: '/w/x/boards/b/-/c12',
        container: 'Board',
        locale: 'en',
        active: false,
        onGo: () => undefined,
        onPoint: () => undefined,
      },
    });
    const number = container.querySelector('[data-slot="card-number"]');
    expect(number?.getAttribute('data-number')).toBe('12');
    expect(number?.querySelector('[aria-hidden="true"]')?.textContent).toBe('#12');
    expect(number?.querySelector('.sr-only')?.textContent).toBe(' number 12 ');
  });
});
