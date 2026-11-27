/**
 * The window on a long column (features/views/kanban/column-window.ts) and
 * where a drop on it lands in the whole list (drag/sortable-dom.ts).
 */
import { describe, expect, it, vi } from 'vitest';

import {
  CARD_GAP,
  CardHeights,
  cardsBetween,
  contentHeight,
  drawnRuns,
  NEAR,
  piecesOf,
  topsOf,
  type Piece,
} from '$lib/features/views/kanban/column-window.ts';
import { placeAmong } from '$lib/drag/sortable-dom.ts';
import { createCard, type Card } from '$lib/logic/wire.ts';

const cards = (count: number): Card[] =>
  Array.from({ length: count }, (_, at) => ({ ...createCard(), id: `c${String(at)}` }));

/** The pieces in short: a card by its id, `gap`, a blank by its height. */
const short = (pieces: readonly Piece[]): string[] =>
  pieces.map((piece) =>
    piece.kind === 'card'
      ? piece.card.id
      : piece.kind === 'gap'
        ? 'gap'
        : `blank ${String(piece.height)}`,
  );

describe('a window on a long column', () => {
  it('finds the cards between two depths, from where each starts', () => {
    const tops = topsOf([100, 100, 100, 100]);
    expect(tops).toEqual([8, 116, 224, 332, 440]);
    // 150 to 250 down: the second card (116 to 216) and the third (224 to 324).
    expect(cardsBetween(tops, 150, 250)).toEqual({ first: 1, end: 3 });
    expect(cardsBetween(tops, -500, 0)).toEqual({ first: 0, end: 0 });
    expect(cardsBetween(tops, 1000, 2000)).toEqual({ first: 4, end: 4 });
  });

  it('accounts for a drag gap above a short viewport in cardsBetween', () => {
    const tops = topsOf([100, 100, 100, 100]);
    // Gap of 200px before card 1 shifts cards 1..3 down by 208px: card 1 sits at [324, 424].
    expect(cardsBetween(tops, 330, 410, 1, 200)).toEqual({ first: 1, end: 2 });
    // Card 0 above the gap stays at [8, 108].
    expect(cardsBetween(tops, 20, 90, 1, 200)).toEqual({ first: 0, end: 1 });
  });

  it('clears recorded card heights and notifies scroll anchoring synchronously', () => {
    let callback: ResizeObserverCallback | undefined;
    const observed = new Set<Element>();
    const Original = globalThis.ResizeObserver;
    class FakeResizeObserver {
      constructor(cb: ResizeObserverCallback) {
        callback = cb;
      }
      observe(target: Element): void {
        observed.add(target);
      }
      unobserve(target: Element): void {
        observed.delete(target);
      }
      disconnect(): void {
        observed.clear();
      }
    }
    vi.stubGlobal('ResizeObserver', FakeResizeObserver);
    try {
      const onScroll = vi.fn();
      const heights = new CardHeights(() => 100, onScroll);
      const list = document.createElement('div');
      list.scrollTop = 500;
      const stopWatch = heights.watch(list);
      const cardElement = document.createElement('div');
      Object.defineProperty(cardElement, 'offsetTop', { value: 100 });
      const stopMeasure = heights.measure(cardElement, 'c1');

      callback?.(
        [
          {
            target: cardElement,
            borderBoxSize: [{ blockSize: 120, inlineSize: 200 }],
          } as unknown as ResizeObserverEntry,
        ],
        {} as ResizeObserver,
      );
      expect(heights.get('c1')).toBe(120);
      expect(list.scrollTop).toBe(520);
      expect(onScroll).toHaveBeenCalledExactlyOnceWith(520);

      heights.clear();
      expect(heights.get('c1')).toBeUndefined();
      expect(observed.has(cardElement)).toBe(true);

      stopMeasure();
      stopWatch();
    } finally {
      vi.stubGlobal('ResizeObserver', Original);
    }
  });

  it('draws the cards in sight and those near each pin, as few runs as they make', () => {
    expect(drawnRuns(100, { first: 10, end: 15 }, [-1, 50, 13])).toEqual([
      { first: 10, end: 17 },
      { first: 50 - NEAR, end: 50 + NEAR + 1 },
    ]);
    // Within the column: the run near the last card ends with it, and meets the one in sight.
    expect(drawnRuns(5, { first: 0, end: 2 }, [4])).toEqual([{ first: 0, end: 5 }]);
  });

  it('stands a blank as high as the cards it leaves out, and the room between them', () => {
    const column = cards(6);
    const pieces = piecesOf(column, [], -1, [{ first: 2, end: 4 }], [10, 20, 30, 40, 50, 60]);
    expect(short(pieces)).toEqual([
      `blank ${String(10 + 20 + CARD_GAP)}`,
      'c2',
      'c3',
      `blank ${String(50 + 60 + CARD_GAP)}`,
    ]);
  });

  it('keeps the cards on their way, and counts the others and the gap without them', () => {
    const column = cards(6);
    // c1 is on its way; the others are 0 c0, 1 c2, 2 c3, 3 c4, 4 c5; the gap before c3.
    const pieces = piecesOf(column, ['c1'], 2, [{ first: 1, end: 3 }], [10, 30, 40, 50, 60]);
    expect(short(pieces)).toEqual([
      'blank 10',
      'c1',
      'c2',
      'gap',
      'c3',
      `blank ${String(50 + 60 + CARD_GAP)}`,
    ]);
    const ats = pieces.flatMap((piece) =>
      piece.kind === 'card' ? [[piece.card.id, piece.at]] : [],
    );
    expect(ats).toEqual([
      ['c1', -1],
      ['c2', 1],
      ['c3', 2],
    ]);
  });

  it('begins and ends a window with a blank, empty where every card there is drawn', () => {
    const pieces = piecesOf(cards(3), [], -1, [{ first: 0, end: 3 }], [10, 10, 10]);
    expect(short(pieces)).toEqual(['blank 0', 'c0', 'c1', 'c2', 'blank 0']);
    const keys = pieces.flatMap((piece) => (piece.kind === 'blank' ? [piece.key] : []));
    expect(keys).toEqual(['start', 'end']);
  });

  it('counts the gap or carried card in the full height of the list while one is there', () => {
    const tops = topsOf([100, 100, 100]);
    expect(contentHeight(tops, -1, 120)).toBe(tops.at(-1));
    expect(contentHeight(tops, 3, 120)).toBe((tops.at(-1) ?? 0) + 120 + CARD_GAP);
    expect(contentHeight(topsOf([]), 0, 120)).toBe(8 + 120 + 8);
  });

  it('draws every card without a window', () => {
    expect(short(piecesOf(cards(3), [], 3, undefined, []))).toEqual(['c0', 'c1', 'c2', 'gap']);
  });
});

describe('a drop among the items a list draws', () => {
  const item = (at?: number): HTMLElement => {
    const element = document.createElement('div');
    if (at !== undefined) element.setAttribute('data-drag-at', String(at));
    return element;
  };

  it('lands where the items drawn say they are', () => {
    const drawn = [item(40), item(41), item(42)];
    expect(placeAmong(drawn, 0)).toBe(40);
    expect(placeAmong(drawn, 2)).toBe(42);
    expect(placeAmong(drawn, 3)).toBe(43);
  });

  it('counts the items themselves where they do not say', () => {
    expect(placeAmong([item(), item()], 1)).toBe(1);
    expect(placeAmong([], 0)).toBe(0);
  });
});
