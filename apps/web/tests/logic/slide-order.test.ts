/**
 * A deck's slides as the viewer turns through them, its hidden slides left
 * out or shown in place (src/lib/file-viewer/slide-order.ts).
 */
import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import { SlideOrder } from '$lib/file-viewer/slide-order.ts';

/** The slides laid out, in order, by turning from the first. */
function turned(order: SlideOrder, expanded: boolean): number[] {
  const pages: number[] = [];
  for (let page: number | undefined = order.first(expanded); page !== undefined;) {
    pages.push(page);
    page = order.next(page, 1, expanded);
  }
  return pages;
}

describe("a deck's hidden slides", () => {
  it('are left out, the others keeping their numbers', () => {
    const order = new SlideOrder(6, [2, 3, 6]);
    expect(turned(order, false)).toEqual([1, 4, 5]);
    expect(order.count(false)).toBe(3);
    expect(turned(order, true)).toEqual([1, 2, 3, 4, 5, 6]);
    expect(order.count(true)).toBe(6);
    expect(order.next(4, -1, false)).toBe(1);
    expect(order.next(5, 1, false)).toBeUndefined();
    expect(order.next(1, -1, false)).toBeUndefined();
  });

  it('at the start or the end: the first and last a show gives', () => {
    const order = new SlideOrder(5, [1, 2, 5]);
    expect(order.first(false)).toBe(3);
    expect(order.first(true)).toBe(1);
    expect(order.next(4, 1, false)).toBeUndefined();
    expect(order.next(3, -1, false)).toBeUndefined();
  });

  it('when every slide is hidden, are all laid out', () => {
    const order = new SlideOrder(3, [1, 2, 3]);
    expect(order.allHidden).toBe(true);
    expect(turned(order, false)).toEqual([1, 2, 3]);
    expect(order.count(false)).toBe(3);
  });

  it('when left out, the reader on one lands on the next a show gives, else the one before', () => {
    const order = new SlideOrder(6, [2, 3, 6]);
    expect(order.landing(3, false)).toBe(4);
    expect(order.landing(2, false)).toBe(4);
    expect(order.landing(6, false)).toBe(5);
    expect(order.landing(4, false)).toBe(4);
    // Shown: the reader stays.
    expect(order.landing(3, true)).toBe(3);
  });

  it('numbers out of the deck are not hidden slides', () => {
    const order = new SlideOrder(3, [0, 4, 1.5, 2]);
    expect([...order.hidden]).toEqual([2]);
    expect(order.shows(4, true)).toBe(false);
    expect(new SlideOrder(0, []).first(false)).toBe(1);
  });

  it('turning and landing never reach a slide left out, nor leave the deck', () => {
    fc.assert(
      fc.property(
        fc.integer({ min: 1, max: 30 }),
        fc.array(fc.integer({ min: 1, max: 30 })),
        fc.boolean(),
        fc.integer({ min: 1, max: 30 }),
        (pages, hidden, expanded, at) => {
          const order = new SlideOrder(pages, hidden);
          const page = Math.min(at, pages);
          const laid = turned(order, expanded);
          expect(laid.length).toBe(order.count(expanded));
          expect(laid.every((each) => order.shows(each, expanded))).toBe(true);
          const landing = order.landing(page, expanded);
          expect(order.shows(landing, expanded)).toBe(true);
          if (order.shows(page, expanded)) expect(landing).toBe(page);
          else if (laid.some((each) => each > page)) expect(landing).toBeGreaterThan(page);
        },
      ),
    );
  });
});
