/**
 * The slides the frame draws at once (src/viewer-frame/pptx-view.ts): no
 * more pixels of pictures together than one slide may show, and a slide in
 * view drawn all the same, in place of slides drawn out of view.
 */
import type { PresentationData, SlideHandle } from '@aiden0z/pptx-renderer';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { SlideOrder } from '$lib/file-viewer/slide-order.ts';
import { PIXELS_DRAWN, PptxView } from '../../src/viewer-frame/pptx-view.ts';
import type { Report } from '../../src/viewer-frame/renderer.ts';

/** Where each slide's box is in the frame's view, by its page: [top, bottom]. */
let places = new Map<number, [number, number]>();
let observer: { callback: IntersectionObserverCallback } | undefined;

class FakeObserver {
  constructor(callback: IntersectionObserverCallback) {
    observer = { callback };
  }
  /** Every box is observed; `sight` says which are near the view. */
  observe(): void {
    return undefined;
  }
  disconnect(): void {
    return undefined;
  }
}

/** Tells the view which boxes are near the view now (all with a place). */
function sight(boxes: Element[]): void {
  const entries = boxes.map((box) => {
    const page = Number((box as HTMLElement).dataset.page);
    return {
      target: box,
      isIntersecting: places.has(page),
    } as unknown as IntersectionObserverEntry;
  });
  observer?.callback(entries, {} as IntersectionObserver);
}

beforeEach(() => {
  places = new Map();
  vi.stubGlobal('IntersectionObserver', FakeObserver);
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
    this: HTMLElement,
  ) {
    const [top, bottom] = places.get(Number(this.dataset.page)) ?? [-10_000, -9_000];
    return { top, bottom, left: 0, right: 100, width: 100, height: bottom - top } as DOMRect;
  });
  vi.stubGlobal('innerHeight', 900);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const LABELS = { chart: 'chart', pageTooLarge: 'too large', download: 'Download', hidden: '숨김' };

/**
 * A view of `pixels.length` slides, each showing that many pixels, of which
 * those numbered in `hidden` are hidden from a show; the pages drawn, and
 * the pages the view told of.
 */
function view(
  pixels: number[],
  hidden: number[] = [],
): { view: PptxView; drawn: () => number[]; told: number[] } {
  const presentation = {
    width: 960,
    height: 540,
    slides: pixels.map((_, index) => ({ slidePath: `ppt/slides/slide${index + 1}.xml` })),
  } as unknown as PresentationData;
  const live = new Set<number>();
  const made = new Map(pixels.map((each, index) => [`ppt/slides/slide${index + 1}.xml`, each]));
  const place = document.createElement('div');
  const told: number[] = [];
  const report: Report = {
    sheets: () => undefined,
    hidden: () => undefined,
    shown: () => undefined,
    page: (page) => told.push(page),
    failed: () => undefined,
    download: () => undefined,
  };
  const shown = new PptxView(
    presentation,
    place,
    report,
    (index): SlideHandle => {
      live.add(index + 1);
      return {
        element: document.createElement('div'),
        ready: Promise.resolve(),
        dispose: () => live.delete(index + 1),
      } as unknown as SlideHandle;
    },
    LABELS,
    made,
    new SlideOrder(pixels.length, hidden),
  );
  return { view: shown, drawn: () => [...live].sort((a, b) => a - b), told };
}

/** Lets the view draw what it will. */
const settle = async (): Promise<void> => {
  await vi.advanceTimersByTimeAsync(2_000);
};

describe('the slides drawn at once', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('show no more pixels together than one slide may: one only near the view waits', async () => {
    const share = PIXELS_DRAWN * 0.4;
    const { view: shown, drawn } = view([share, share, share]);
    places = new Map([
      [1, [0, 540]],
      [2, [560, 1100]],
      [3, [1120, 1660]],
    ]);
    sight(shown.boxes);
    await settle();
    expect(drawn()).toEqual([1, 2]);
  });

  it('a slide in view is drawn in place of slides drawn out of view', async () => {
    const most = PIXELS_DRAWN - 1;
    const { view: shown, drawn } = view([most, most, most]);
    places = new Map([[1, [0, 540]]]);
    sight(shown.boxes);
    await settle();
    expect(drawn()).toEqual([1]);
    // Scrolled on: slide 1 is above the view but near it, slide 2 in view.
    places = new Map([
      [1, [-600, -60]],
      [2, [0, 540]],
    ]);
    sight(shown.boxes);
    await settle();
    expect(drawn()).toEqual([2]);
  });
});

/** The numbers of the boxes laid out. */
const numbers = (boxes: Element[]): number[] =>
  boxes.map((box) => Number((box as HTMLElement).dataset.page));

describe('slides hidden from a show', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(window, 'scrollBy').mockImplementation(() => undefined);
    HTMLElement.prototype.scrollIntoView = () => undefined;
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('are left out of the column; the first slide drawn is the first a show gives', async () => {
    const { view: shown, drawn } = view([1, 1, 1, 1], [1, 3]);
    expect(numbers(shown.boxes)).toEqual([2, 4]);
    const first = shown.drawFirst();
    await settle();
    await first;
    expect(drawn()).toEqual([2]);
  });

  it('come back in place, marked in words, and go again with their drawing', async () => {
    const { view: shown, drawn, told } = view([1, 1, 1, 1], [2, 3]);
    shown.showHidden(true);
    expect(numbers(shown.boxes)).toEqual([1, 2, 3, 4]);
    const marked = shown.boxes.filter((box) => box.classList.contains('slide-hidden'));
    expect(numbers(marked)).toEqual([2, 3]);
    expect(marked.map((box) => box.textContent)).toEqual(['숨김', '숨김']);
    places = new Map([[3, [0, 540]]]);
    sight(shown.boxes);
    await settle();
    expect(drawn()).toEqual([3]);
    // The mark stays over the slide drawn.
    expect(shown.boxes[2]?.lastElementChild?.textContent).toBe('숨김');
    // Left out while on hidden slide 3: the reader lands on 4, the next a show gives.
    shown.goTo(3);
    shown.showHidden(false);
    expect(numbers(shown.boxes)).toEqual([1, 4]);
    expect(drawn()).toEqual([]);
    expect(told.at(-1)).toBe(4);
  });

  it('come and go with the slide in view kept where it is in the view', () => {
    // Laid out as the frame would: each slide laid out 600 pixels below the one before.
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
      this: HTMLElement,
    ) {
      const column = [...(this.parentElement?.children ?? [])].filter(
        (box) => !(box as HTMLElement).hidden,
      );
      const top = this.hidden ? 0 : column.indexOf(this) * 600;
      return { top, bottom: top + 540, left: 0, right: 100, width: 100, height: 540 } as DOMRect;
    });
    const scrolled = vi.spyOn(window, 'scrollBy').mockImplementation(() => undefined);
    const { view: shown, told } = view([1, 1, 1, 1, 1], [2, 3]);
    // On slide 4, second of the slides laid out (1, 4, 5): 600 down.
    shown.goTo(4);
    shown.showHidden(true);
    // Now fourth (1, 2, 3, 4, 5): 1,800 down. The view moves by the difference.
    expect(scrolled).toHaveBeenLastCalledWith({ top: 1_200, behavior: 'instant' });
    expect(told.at(-1)).toBe(4);
    // On hidden slide 3 (1,200 down), left out: slide 4 (then 600 down) takes its place.
    shown.goTo(3);
    shown.showHidden(false);
    expect(scrolled).toHaveBeenLastCalledWith({ top: -600, behavior: 'instant' });
    expect(told.at(-1)).toBe(4);
  });

  it('are not gone to while left out', () => {
    const { view: shown, told } = view([1, 1, 1], [2]);
    shown.goTo(2);
    expect(told).not.toContain(2);
    shown.goTo(3);
    expect(told.at(-1)).toBe(3);
  });

  it('when every slide is, are all laid out', async () => {
    const { view: shown, drawn } = view([1, 1], [1, 2]);
    expect(numbers(shown.boxes)).toEqual([1, 2]);
    const first = shown.drawFirst();
    await settle();
    await first;
    expect(drawn()).toEqual([1]);
  });
});
