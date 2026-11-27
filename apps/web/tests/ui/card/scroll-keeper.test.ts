/**
 * The place a card was scrolled to, kept while its box changes size. The
 * simulated page has no layout, so the two decisions are tested here, and
 * the order of the observer and the scroll events on an element whose sizes
 * the test sets; the scrolling itself is tested in a browser
 * (e2e/card.spec.ts, "the window changes size under an open card").
 */
import { describe, expect, it, vi } from 'vitest';

import {
  afterResize,
  afterScroll,
  keepScroll,
  type Box,
  type Kept,
} from '$lib/features/card/scroll-keeper.ts';

const PHONE = { width: 360, height: 600 };
const WIDE = { width: 768, height: 560 };

const kept = (top: number, size = PHONE): Kept => ({ top, ...size });
const box = (top: number, max: number, size = PHONE): Box => ({ top, max, ...size });

describe('after a scroll event', () => {
  it("remembers the reader's own scrolling", () => {
    expect(afterScroll(kept(0), box(240, 900))).toEqual(kept(240));
    expect(afterScroll(kept(240), box(10, 900))).toEqual(kept(10));
  });

  it('does not take a position the browser cut down in a resized box for the reader', () => {
    // 400 down on a phone; in the wide dialog there are only 50 pixels to scroll.
    expect(afterScroll(kept(400), box(50, 50, WIDE))).toEqual(kept(400));
    // Positions are fractions on some screens: nearly at the end is at the end.
    expect(afterScroll(kept(400), box(49.5, 50, WIDE))).toEqual(kept(400));
  });

  it('remembers scrolling in the resized box that is not at its end', () => {
    expect(afterScroll(kept(400), box(20, 50, WIDE))).toEqual(kept(20, WIDE));
  });

  it('remembers scrolling further down in the resized box', () => {
    expect(afterScroll(kept(100), box(300, 300, WIDE))).toEqual(kept(300, WIDE));
  });

  it('takes the end of a box that did not change size for what it is', () => {
    // The content got shorter (a comment was deleted): that is where the reader is now.
    expect(afterScroll(kept(400), box(350, 350))).toEqual(kept(350));
  });
});

describe('after the box changed size', () => {
  it('goes back to the remembered place when there is room for it again', () => {
    expect(afterResize(kept(400), box(50, 900))).toBe(400);
  });

  it('goes as near as the box can scroll', () => {
    expect(afterResize(kept(400), box(50, 120, WIDE))).toBe(120);
  });

  it('leaves the position alone when it is already there or cannot get nearer', () => {
    expect(afterResize(kept(400), box(400, 900, WIDE))).toBeUndefined();
    expect(afterResize(kept(400), box(50, 50, WIDE))).toBeUndefined();
    expect(afterResize(kept(0), box(0, 0, WIDE))).toBeUndefined();
  });

  it('never scrolls up: a position further down was the reader moving on', () => {
    expect(afterResize(kept(100), box(300, 900, WIDE))).toBeUndefined();
  });

  it('gives the place back after a round of sizes', () => {
    // Phone, 120 down. Wide: cut to 50. Phone again: back to 120.
    let memory = kept(120);
    memory = afterScroll(memory, box(50, 50, WIDE));
    expect(afterResize(memory, box(50, 50, WIDE))).toBeUndefined();
    expect(afterResize(memory, box(50, 700))).toBe(120);
    memory = afterScroll(memory, box(120, 700));
    expect(memory).toEqual(kept(120));
  });
});

/** A box that scrolls down, as far as the keeper reads it. */
function scroller(): HTMLElement & { resize(size: typeof PHONE, content: number): void } {
  const element = document.createElement('div');
  let size = PHONE;
  let scrollHeight = 1500;
  let scrollTop = 0;
  Object.defineProperties(element, {
    clientWidth: { get: () => size.width },
    clientHeight: { get: () => size.height },
    offsetWidth: { get: () => size.width },
    offsetHeight: { get: () => size.height },
    scrollHeight: { get: () => scrollHeight },
    scrollTop: {
      get: () => scrollTop,
      // The browser never holds a position beyond the end.
      set: (value: number) => {
        scrollTop = Math.max(0, Math.min(value, scrollHeight - size.height));
      },
    },
  });
  return Object.assign(element, {
    resize(to: typeof PHONE, content: number): void {
      size = to;
      scrollHeight = content;
      // What a layout does to a position that no longer fits.
      element.scrollTop = scrollTop;
    },
  });
}

describe('the keeper on a box', () => {
  /** Keeps the box and hands out what calls its observer. */
  function kept(element: HTMLElement): {
    observed(told?: typeof PHONE): void;
    detach(): void;
  } {
    let callback: ((entries: ResizeObserverEntry[]) => void) | undefined;
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(given: (entries: ResizeObserverEntry[]) => void) {
          callback = given;
        }
        observe(): void {
          // Nothing to do: the test calls the callback when it resizes the element.
        }
        disconnect(): void {
          callback = undefined;
        }
      },
    );
    const detach = keepScroll(element);
    return {
      // `told`: the size the observer is told of, when the box has another by the time it runs.
      observed: (told) =>
        callback?.(
          told
            ? [
                {
                  borderBoxSize: [{ inlineSize: told.width, blockSize: told.height }],
                } as unknown as ResizeObserverEntry,
              ]
            : [],
        ),
      detach: () => {
        detach();
        vi.unstubAllGlobals();
      },
    };
  }

  it('does not take a cut for the reader when its event comes after the box is small again', () => {
    const element = scroller();
    const keeper = kept(element);
    element.scrollTop = 400;
    element.dispatchEvent(new Event('scroll'));

    // Unfolded and folded again within a frame: the event of the cut is delivered in the old box.
    element.resize(WIDE, 610);
    keeper.observed();
    expect(element.scrollTop).toBe(50);
    element.resize(PHONE, 1500);
    element.dispatchEvent(new Event('scroll'));
    keeper.observed();
    expect(element.scrollTop).toBe(400);
    keeper.detach();
  });

  it('does not pull the card on when the reader scrolled back and the event has not arrived yet', () => {
    const element = scroller();
    const keeper = kept(element);
    element.scrollTop = 400;
    element.dispatchEvent(new Event('scroll'));
    // Scrolled back to 100, and the box changes before the scroll event is delivered.
    element.scrollTop = 100;
    element.resize({ width: 360, height: 560 }, 1500);
    keeper.observed();
    expect(element.scrollTop).toBe(100);
    keeper.detach();
  });

  it('does not take a cut for the reader when the page changed its stage before the observer ran', () => {
    const element = scroller();
    const keeper = kept(element);
    element.scrollTop = 400;
    element.dispatchEvent(new Event('scroll'));
    // Laid out wide once, which cut the place; the page's observer answered with another
    // stage, and this one is told of the wide box when it is gone again.
    element.resize(WIDE, 610);
    element.resize({ width: 600, height: 560 }, 1260);
    expect(element.scrollTop).toBe(50);
    keeper.observed(WIDE);
    expect(element.scrollTop).toBe(400);
    keeper.detach();
  });
});
