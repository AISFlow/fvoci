/**
 * The slides of a presentation in the frame: a box of each slide's size at
 * once, and in them only the slides in view (and a screen's height around
 * them), drawn one at a time.
 *
 * The renderer draws with the frame's own layout, on the frame's thread,
 * which the app shares. So a slide is drawn, cleaned (`pptx-clean.ts`) and
 * put in its box, the renderer is let fit its text again on the frames
 * after (it does so two frames on), and only then, the thread given back
 * once more, is the next slide drawn. What one slide may ask is bounded in
 * the worker (`pptx-ceilings.ts`). A slide that scrolls far away is taken
 * out of the page.
 *
 * A slide is drawn at its own size and scaled with a transform, so the
 * renderer's measuring sees the sizes it laid out; zooming scales the boxes
 * without drawing again.
 *
 * A slide hidden from a show is left out of the column (`SlideOrder`) until
 * the reader shows the hidden slides; then it is in its place, marked with
 * a word over it and its drawing faded. The slide the reader is on stays
 * where it was in the view as the others come or go.
 */
import type { PresentationData, SlideHandle, SlideRendererOptions } from '@aiden0z/pptx-renderer';

import type { Labels } from '$lib/file-viewer/protocol.ts';
import type { SlideOrder } from '$lib/file-viewer/slide-order.ts';

import { cleanSlide } from './pptx-clean.ts';
import { PageFollower, type Drawn, type Report } from './renderer.ts';
import { yieldThread } from './yield-thread.ts';

/** The room around the slides, in CSS pixels, at each side. */
const GUTTER = 16;
/**
 * The most slides drawn at once, whatever the deck's slide size and the
 * zoom put in view: the frame's memory is the app's process's.
 */
const DRAWN_LIMIT = 16;
/**
 * The most pixels of pictures the slides drawn at once may show together:
 * what one slide may show (`SLIDE_LIMITS.pixels` in `pptx-ceilings.ts`). A
 * slide in view takes the place of slides drawn out of view, the farthest
 * first; one only near the view waits for room. A slide in view is drawn
 * whatever it shows when no other in view is drawn.
 */
export const PIXELS_DRAWN = 150_000_000;
/**
 * How long a frame, or the renderer's own fitting (which waits for frames),
 * is waited for: a hidden page has none, and the slide is drawn all the same.
 */
const FRAME_WAIT = 100;

/** Draws the slide at `index` (from 0). */
export type DrawSlide = (index: number, options: SlideRendererOptions) => SlideHandle;

/** The next frame, or `FRAME_WAIT` milliseconds, whichever comes first. */
const nextFrame = (): Promise<void> =>
  new Promise((resolve) => {
    const waited = setTimeout(resolve, FRAME_WAIT);
    requestAnimationFrame(() => {
      clearTimeout(waited);
      resolve();
    });
  });

const wait = (milliseconds: number): Promise<void> =>
  new Promise((resolve) => {
    setTimeout(resolve, milliseconds);
  });

/** One slide: its box in the column, and the slide drawn in it while it is in view. */
class Slide {
  readonly box = document.createElement('div');
  readonly number: number;
  /** The pixels of the pictures it shows. */
  readonly pixels: number;
  /** The word over a slide hidden from a show; none on the others. */
  readonly #mark: HTMLElement[] = [];
  handle: SlideHandle | undefined;
  /** The renderer could not draw it: it stays an empty box and is not tried again. */
  failed = false;

  constructor(number: number, pixels: number, hidden: string | undefined) {
    this.number = number;
    this.pixels = pixels;
    this.box.className = 'page slide';
    this.box.dataset.page = String(number);
    if (hidden !== undefined) {
      this.box.classList.add('slide-hidden');
      const mark = document.createElement('span');
      mark.className = 'slide-mark';
      mark.textContent = hidden;
      this.#mark.push(mark);
      this.box.append(mark);
    }
  }

  /** Puts the slide drawn in its box, under its mark. */
  show(handle: SlideHandle): void {
    this.handle = handle;
    this.box.replaceChildren(handle.element, ...this.#mark);
  }

  clear(): void {
    this.handle?.dispose();
    this.handle = undefined;
    this.box.replaceChildren(...this.#mark);
  }
}

export class PptxView implements Drawn {
  readonly #presentation: PresentationData;
  readonly #draw: DrawSlide;
  /** What the box of a chart says. */
  readonly #chart: string;
  readonly #slides: Slide[];
  readonly #visible = new Set<Slide>();
  readonly #observer: IntersectionObserver;
  readonly #follower: PageFollower;
  readonly #options: SlideRendererOptions;
  readonly #order: SlideOrder;
  /** Whether the hidden slides are in the column. */
  #expanded = false;
  #zoom = 1;
  #drawing = false;

  constructor(
    presentation: PresentationData,
    place: HTMLElement,
    report: Report,
    draw: DrawSlide,
    labels: Labels,
    pixels: ReadonlyMap<string, number>,
    order: SlideOrder,
  ) {
    this.#presentation = presentation;
    this.#draw = draw;
    this.#chart = labels.chart;
    this.#order = order;
    this.#options = {
      externalMedia: 'block',
      pdfjs: false,
      // One address for each picture, made once, for as long as the frame lasts.
      mediaUrlCache: new Map(),
      onNavigate: ({ slideIndex }) => {
        if (slideIndex !== undefined) this.goTo(slideIndex + 1);
      },
    };
    const column = document.createElement('div');
    column.className = 'pages';
    this.#slides = presentation.slides.map(
      (slide, index) =>
        new Slide(
          index + 1,
          pixels.get(slide.slidePath) ?? 0,
          order.hidden.has(index + 1) ? labels.hidden : undefined,
        ),
    );
    this.#leaveOut();
    column.append(...this.#slides.map((slide) => slide.box));
    place.replaceChildren(column);
    // The view keeps the reader's place itself as slides come and go (`showHidden`); the
    // browser's own keeping of it would move the view again after.
    document.documentElement.style.overflowAnchor = 'none';
    this.#observer = new IntersectionObserver(
      (entries) => {
        this.#onSight(entries);
      },
      // The document as the root: the frame's own viewport, whose margin is taken (it is not
      // when the root is left out, the frame being of another origin than the page).
      { root: document, rootMargin: '100% 0px' },
    );
    this.#layOut();
    for (const slide of this.#slides) this.#observer.observe(slide.box);
    this.#follower = new PageFollower(
      () => this.boxes,
      report,
      (box) => Number((box as HTMLElement).dataset.page),
    );
    let resizing: ReturnType<typeof setTimeout> | undefined;
    window.addEventListener('resize', () => {
      clearTimeout(resizing);
      resizing = setTimeout(() => {
        this.#layOut();
      }, 150);
    });
  }

  /** The boxes of the slides in the column. */
  get boxes(): Element[] {
    return this.#slides.filter((slide) => !slide.box.hidden).map((slide) => slide.box);
  }

  /** Takes the slides not laid out out of the column (and out of view: their drawing goes). */
  #leaveOut(): void {
    for (const slide of this.#slides) {
      const out = !this.#order.shows(slide.number, this.#expanded);
      if (out && !slide.box.hidden) slide.clear();
      slide.box.hidden = out;
    }
  }

  /** The scale at the zoom: the slide as wide as the frame allows (its scroll bar left out), at most its own size. */
  #scale(): number {
    const room = Math.max(1, document.documentElement.clientWidth - 2 * GUTTER);
    return Math.min(1, room / this.#presentation.width) * this.#zoom;
  }

  #layOut(): void {
    const scale = this.#scale();
    const { width, height } = this.#presentation;
    for (const { box, handle } of this.#slides) {
      box.style.width = `${Math.floor(width * scale)}px`;
      box.style.height = `${Math.floor(height * scale)}px`;
      if (handle) handle.element.style.transform = `scale(${scale})`;
    }
  }

  /** Draws one slide into its box and lets the renderer finish fitting it. */
  async #drawSlide(slide: Slide): Promise<void> {
    const handle = this.#draw(slide.number - 1, this.#options);
    const { element } = handle;
    cleanSlide(element, this.#chart);
    element.style.transformOrigin = '0 0';
    element.style.transform = `scale(${this.#scale()})`;
    slide.show(handle);
    const clean = (): void => {
      if (slide.handle === handle) cleanSlide(element, this.#chart);
    };
    // The renderer's fitting is ready on frames of its own; whenever it is, what it set is cleaned.
    const ready = handle.ready.then(clean, () => undefined);
    await Promise.race([ready, wait(FRAME_WAIT)]);
    // The renderer fits text again two frames on: that is part of drawing this slide.
    await nextFrame();
    await nextFrame();
    // What the renderer set meanwhile (its pictures' recolouring, its refitting) is cleaned too.
    clean();
  }

  /**
   * The slide to draw next: of those near the view not drawn yet, one in the
   * frame's view before one only near it, the first first. A slide in view
   * takes the place of slides drawn out of view, the farthest first, while
   * `DRAWN_LIMIT` slides are drawn or their pictures and its own would be
   * past `PIXELS_DRAWN`; one only near the view waits for room.
   */
  #next(): Slide | undefined {
    const inView = (slide: Slide): boolean => {
      const { top, bottom } = slide.box.getBoundingClientRect();
      return bottom > 0 && top < window.innerHeight;
    };
    const [slide] = [...this.#visible]
      .filter((each) => !each.handle && !each.failed)
      .sort((a, b) => Number(inView(b)) - Number(inView(a)) || a.number - b.number);
    if (!slide) return undefined;
    let drawn = this.#slides.filter((each) => each.handle);
    const room = (): boolean =>
      drawn.length < DRAWN_LIMIT &&
      (drawn.length === 0 ||
        drawn.reduce((sum, each) => sum + each.pixels, 0) + slide.pixels <= PIXELS_DRAWN);
    if (inView(slide)) {
      const middle = window.innerHeight / 2;
      const distance = (each: Slide): number =>
        Math.abs(each.box.getBoundingClientRect().top - middle);
      const away = drawn.filter((each) => !inView(each)).sort((a, b) => distance(b) - distance(a));
      for (const each of away) {
        if (room()) break;
        each.clear();
        drawn = drawn.filter((other) => other !== each);
      }
    }
    return room() ? slide : undefined;
  }

  /** Draws the slides in view one after another, the thread given back between them. */
  async #drawInView(): Promise<void> {
    if (this.#drawing) return;
    this.#drawing = true;
    try {
      for (let slide = this.#next(); slide; slide = this.#next()) {
        await this.#drawSlide(slide).catch(() => {
          slide.clear();
          slide.failed = true;
        });
        await yieldThread();
      }
    } finally {
      this.#drawing = false;
    }
  }

  #onSight(entries: IntersectionObserverEntry[]): void {
    for (const entry of entries) {
      const slide = this.#slides[Number((entry.target as HTMLElement).dataset.page) - 1];
      if (!slide) continue;
      if (entry.isIntersecting) this.#visible.add(slide);
      else {
        this.#visible.delete(slide);
        slide.clear();
      }
    }
    void this.#drawInView();
  }

  /** Draws the first slide laid out, the one in view when the presentation opens; rejects when it cannot be drawn. */
  async drawFirst(): Promise<void> {
    const first = this.#slides[this.#order.first(this.#expanded) - 1];
    if (!first) return;
    // The slides in view wait for it (the observer tells of them once the column is laid out).
    this.#drawing = true;
    try {
      await this.#drawSlide(first);
    } finally {
      this.#drawing = false;
    }
    void this.#drawInView();
  }

  goTo(page: number): void {
    const box = this.#slides[page - 1]?.box;
    // A link to a slide left out goes nowhere, as one to a slide that is not there.
    if (!box || box.hidden) return;
    box.scrollIntoView({ block: 'start' });
    this.#follower.pin(page);
  }

  /**
   * The slide in view (as the frame follows it, not as the app last heard)
   * stays where it is in the view; when it is left out, the slide the reader
   * lands on (`SlideOrder.landing`) takes its place. Either is told.
   */
  showHidden(shown: boolean): void {
    if (shown === this.#expanded) return;
    const page = this.#follower.current || this.#order.first(this.#expanded);
    const from = this.#slides[page - 1];
    const to = this.#slides[this.#order.landing(page, shown) - 1];
    const before = from && !from.box.hidden ? from.box.getBoundingClientRect().top : undefined;
    this.#expanded = shown;
    this.#leaveOut();
    if (!to) return;
    if (before === undefined) to.box.scrollIntoView({ block: 'start' });
    else window.scrollBy({ top: to.box.getBoundingClientRect().top - before, behavior: 'instant' });
    this.#follower.pin(to.number);
  }

  zoom(zoom: number): void {
    const scroller = document.scrollingElement;
    const ratio = scroller ? scroller.scrollTop / Math.max(1, scroller.scrollHeight) : 0;
    this.#zoom = zoom;
    this.#layOut();
    if (scroller) scroller.scrollTop = ratio * scroller.scrollHeight;
  }
}
