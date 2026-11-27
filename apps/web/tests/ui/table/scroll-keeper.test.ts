/**
 * The place a table is scrolled to, kept apart from the element: a position
 * the layout cut is not taken for the reader's.
 */
import { describe, expect, it, vi } from 'vitest';

import { afterScroll, ScrollKeeper, type Kept } from '$lib/features/views/table/scroll-keeper.ts';

describe('the place after a scroll event', () => {
  const kept: Kept = { intended: 100, box: 570 };

  it('is the new position when the box is as it was: the reader scrolled', () => {
    expect(afterScroll(kept, 40, 215, 570)).toEqual({ intended: 40, box: 570 });
    expect(afterScroll(kept, 215, 215, 570)).toEqual({ intended: 215, box: 570 });
  });

  it('stays when the box changed and the position sits at the new end, short of the place', () => {
    // The pane grew to 834 pixels: 15 are left to scroll, and the browser cut 100 down to 15.
    expect(afterScroll(kept, 15, 15, 834)).toBe(kept);
    // Nothing left to scroll at all.
    expect(afterScroll(kept, 0, 0, 2000)).toBe(kept);
  });

  it('is the new position when the box changed but the position is not at the end', () => {
    // The reader scrolled back in the wider box.
    expect(afterScroll(kept, 5, 15, 834)).toEqual({ intended: 5, box: 834 });
  });

  it('is the new position when it is further than the place: the reader scrolled on', () => {
    expect(afterScroll(kept, 150, 150, 700)).toEqual({ intended: 150, box: 700 });
  });

  it('is the new position when the content shrank in the same box: there is nothing to go back to', () => {
    // A column was hidden: the box is the same, the end moved in.
    expect(afterScroll(kept, 60, 60, 570)).toEqual({ intended: 60, box: 570 });
  });
});

/** An element that scrolls, as far as the keeper reads it. jsdom lays nothing out. */
function scroller(): HTMLElement & { resize(width: number, scrollWidth?: number): void } {
  const element = document.createElement('div');
  let clientWidth = 570;
  let scrollWidth = 785;
  let scrollLeft = 0;
  Object.defineProperties(element, {
    clientWidth: { get: () => clientWidth },
    offsetWidth: { get: () => clientWidth },
    offsetHeight: { get: () => 400 },
    scrollWidth: { get: () => scrollWidth },
    clientHeight: { get: () => 400 },
    scrollHeight: { get: () => 400 },
    scrollLeft: {
      get: () => scrollLeft,
      // The browser never holds a position beyond the end.
      set: (value: number) => {
        scrollLeft = Math.max(0, Math.min(value, scrollWidth - clientWidth));
      },
    },
  });
  return Object.assign(element, {
    resize(width: number, content = scrollWidth): void {
      clientWidth = width;
      scrollWidth = content;
      // What a layout does to a position that no longer fits.
      element.scrollLeft = scrollLeft;
    },
  });
}

describe('the keeper on an element', () => {
  it('puts the place back when the box narrows again after a layout cut it', () => {
    let observed: (() => void) | undefined;
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: () => void) {
          observed = callback;
        }
        observe(): void {
          // Nothing to do: the test calls the callback when it resizes the element.
        }
        disconnect(): void {
          observed = undefined;
        }
      },
    );
    const sideways: boolean[] = [];
    const keeper = new ScrollKeeper({
      onSideways: (scrolled) => {
        sideways.push(scrolled);
      },
    });
    const element = scroller();
    const detach = keeper.attach(element);

    element.scrollLeft = 100;
    element.dispatchEvent(new Event('scroll'));
    expect(keeper.place.x).toBe(100);
    expect(sideways).toEqual([true]);

    // One layout at a width the pane does not keep: 15 pixels are left to scroll.
    element.resize(770);
    expect(element.scrollLeft).toBe(15);
    observed?.();
    // The scroll event of the cut arrives; the place is not given up for it.
    element.dispatchEvent(new Event('scroll'));
    expect(keeper.place.x).toBe(100);

    // The pane takes its real width.
    element.resize(634);
    observed?.();
    expect(element.scrollLeft).toBe(100);
    element.dispatchEvent(new Event('scroll'));
    expect(keeper.place.x).toBe(100);

    // From here on a scroll of the reader's is taken again, also to the very start.
    element.scrollLeft = 0;
    element.dispatchEvent(new Event('scroll'));
    expect(keeper.place.x).toBe(0);
    expect(sideways).toEqual([true, false]);

    detach();
    vi.unstubAllGlobals();
  });

  it('does not pull the table back when the reader scrolled on and the event has not arrived yet', () => {
    let observed: (() => void) | undefined;
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: () => void) {
          observed = callback;
        }
        observe(): void {
          // As above.
        }
        disconnect(): void {
          observed = undefined;
        }
      },
    );
    const keeper = new ScrollKeeper();
    const element = scroller();
    const detach = keeper.attach(element);
    element.scrollLeft = 40;
    element.dispatchEvent(new Event('scroll'));
    // Scrolled to 200, and the box changes before the scroll event is delivered.
    element.scrollLeft = 200;
    element.resize(560);
    observed?.();
    expect(element.scrollLeft).toBe(200);
    detach();
    vi.unstubAllGlobals();
  });

  it('does not take a cut for the reader when its event comes after the box is narrow again', () => {
    let observed: (() => void) | undefined;
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: () => void) {
          observed = callback;
        }
        observe(): void {
          // As above.
        }
        disconnect(): void {
          observed = undefined;
        }
      },
    );
    const keeper = new ScrollKeeper();
    const element = scroller();
    const detach = keeper.attach(element);
    element.scrollLeft = 100;
    element.dispatchEvent(new Event('scroll'));

    // Widened and narrowed again within a frame: the event of the cut is delivered in the old box.
    element.resize(770);
    observed?.();
    element.resize(570);
    expect(element.scrollLeft).toBe(15);
    element.dispatchEvent(new Event('scroll'));
    expect(keeper.place.x).toBe(100);
    observed?.();
    expect(element.scrollLeft).toBe(100);
    detach();
    vi.unstubAllGlobals();
  });

  it('does not pull the table on when the reader scrolled back and the event has not arrived yet', () => {
    let observed: (() => void) | undefined;
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: () => void) {
          observed = callback;
        }
        observe(): void {
          // As above.
        }
        disconnect(): void {
          observed = undefined;
        }
      },
    );
    const keeper = new ScrollKeeper();
    const element = scroller();
    const detach = keeper.attach(element);
    element.scrollLeft = 100;
    element.dispatchEvent(new Event('scroll'));
    // Scrolled back to 40, and the box changes before the scroll event is delivered.
    element.scrollLeft = 40;
    element.resize(560);
    observed?.();
    expect(element.scrollLeft).toBe(40);
    element.dispatchEvent(new Event('scroll'));
    expect(keeper.place.x).toBe(40);
    detach();
    vi.unstubAllGlobals();
  });

  it('does not take a cut for the reader when the page changed its stage before the observer ran', () => {
    let observed: ((entries: ResizeObserverEntry[]) => void) | undefined;
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(callback: (entries: ResizeObserverEntry[]) => void) {
          observed = callback;
        }
        observe(): void {
          // As above.
        }
        disconnect(): void {
          observed = undefined;
        }
      },
    );
    const keeper = new ScrollKeeper();
    const element = scroller();
    const detach = keeper.attach(element);
    element.scrollLeft = 100;
    element.dispatchEvent(new Event('scroll'));

    // As a device that turned: laid out 770 wide once, which cut the place; the
    // page's observer answered with the sidebar, and this one is told of 770 at 634.
    element.resize(770);
    element.resize(634);
    expect(element.scrollLeft).toBe(15);
    observed?.([
      { borderBoxSize: [{ inlineSize: 770, blockSize: 400 }] } as unknown as ResizeObserverEntry,
    ]);
    expect(element.scrollLeft).toBe(100);
    element.dispatchEvent(new Event('scroll'));
    expect(keeper.place.x).toBe(100);
    detach();
    vi.unstubAllGlobals();
  });

  it('starts another view at its top left corner', () => {
    const keeper = new ScrollKeeper();
    const element = scroller();
    const detach = keeper.attach(element);
    element.scrollLeft = 80;
    element.dispatchEvent(new Event('scroll'));
    keeper.reset();
    expect(element.scrollLeft).toBe(0);
    expect(keeper.place).toEqual({ x: 0, y: 0 });
    detach();
  });
});
