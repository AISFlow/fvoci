/**
 * Keeps the place a table is scrolled to while the window changes size.
 *
 * The browser keeps a scroll position for as long as the box can hold it.
 * When the box grows (the window widens, a device unfolds or turns) and the
 * table fits, it cuts the position down to what is left to scroll, and does
 * not give it back when the box shrinks again. That also happens within one
 * frame: the stage of the page is worked out by script, one layout after the
 * window changed, so the pane is laid out once at a width it does not keep.
 *
 * So the place the reader scrolled to is remembered here, apart from the
 * element. A position that the layout cut is not taken for the reader's, and
 * whenever the box changes size the remembered place is put back as far as
 * it goes.
 *
 * A scroll event comes a frame after the scroll, when the box may have yet
 * another size (a window widened and narrowed again at once): there, a cut
 * position would no longer look like one. So the observer of the box, which
 * runs in the layout that cut, judges the position first, and a scroll event
 * that only tells of where the observer left the box is passed over.
 *
 * The observer itself may come late: the page's own observer runs before it
 * and may have changed the stage, so that the box it is told of is gone
 * again. A position it finds then is judged by nothing but its direction.
 */

import { stillLaidOut } from '$lib/layout/observed.ts';

/** One direction of scrolling. */
export interface Kept {
  /** Where the reader scrolled to. */
  intended: number;
  /** The size of the box when that was last true of the element. */
  box: number;
}

/**
 * The reader's place after a scroll event: the new position, unless the
 * layout cut it. That is the case when the box has another size than it had
 * at the reader's place and the position now sits at the very end, short of
 * that place: no scrolling of the reader ends there.
 */
export function afterScroll(kept: Kept, position: number, max: number, box: number): Kept {
  const cut = box !== kept.box && position < kept.intended && position >= max - 1;
  return cut ? kept : { intended: position, box };
}

export interface ScrollKeeperOptions {
  /** Called when the table starts or stops being scrolled sideways: for the shadow of the pinned column. */
  onSideways?: (scrolled: boolean) => void;
}

export class ScrollKeeper {
  #element: HTMLElement | undefined;
  #x: Kept = { intended: 0, box: 0 };
  #y: Kept = { intended: 0, box: 0 };
  #sideways = false;
  /** Where the element stood when it was last looked at: an event at the same place is no news. */
  #at = { left: 0, top: 0 };
  readonly #options: ScrollKeeperOptions;

  constructor(options: ScrollKeeperOptions = {}) {
    this.#options = options;
  }

  /** Follows the element that scrolls. For `{@attach}`. */
  readonly attach = (element: HTMLElement): (() => void) => {
    this.#element = element;
    this.#x = { intended: element.scrollLeft, box: element.clientWidth };
    this.#y = { intended: element.scrollTop, box: element.clientHeight };
    this.#at = { left: element.scrollLeft, top: element.scrollTop };
    element.addEventListener('scroll', this.#onScroll, { passive: true });
    // Without the observer (a unit test) positions are only remembered.
    const observer =
      typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(this.#restore);
    observer?.observe(element);
    return () => {
      element.removeEventListener('scroll', this.#onScroll);
      observer?.disconnect();
      this.#element = undefined;
    };
  };

  /** Where the reader is, as remembered. */
  get place(): { x: number; y: number } {
    return { x: this.#x.intended, y: this.#y.intended };
  }

  /** Scrolls to the start and forgets the place: another view is shown. */
  reset(): void {
    const element = this.#element;
    if (!element) return;
    element.scrollLeft = 0;
    element.scrollTop = 0;
    this.#x = { intended: 0, box: element.clientWidth };
    this.#y = { intended: 0, box: element.clientHeight };
    this.#at = { left: 0, top: 0 };
    this.#tell(false);
  }

  #tell(sideways: boolean): void {
    if (sideways === this.#sideways) return;
    this.#sideways = sideways;
    this.#options.onSideways?.(sideways);
  }

  /**
   * Takes in where the element stands, if it moved since it was last looked at: the reader's
   * scroll, unless the layout cut it. `laidOut` is false when the box is no longer the one
   * the layout cut it in: then only a position further on is surely the reader's.
   */
  #read(element: HTMLElement, laidOut = true): void {
    const at = { left: element.scrollLeft, top: element.scrollTop };
    if (at.left === this.#at.left && at.top === this.#at.top) return;
    this.#at = at;
    const width = element.clientWidth;
    const height = element.clientHeight;
    if (laidOut) {
      this.#x = afterScroll(this.#x, at.left, element.scrollWidth - width, width);
      this.#y = afterScroll(this.#y, at.top, element.scrollHeight - height, height);
      return;
    }
    if (at.left > this.#x.intended) this.#x = { intended: at.left, box: width };
    if (at.top > this.#y.intended) this.#y = { intended: at.top, box: height };
  }

  readonly #onScroll = (): void => {
    const element = this.#element;
    if (!element) return;
    this.#read(element);
    this.#tell(element.scrollLeft > 0);
  };

  /** The box changed size: back to the reader's place, as far as the new box lets it. */
  readonly #restore = (entries?: readonly ResizeObserverEntry[]): void => {
    const element = this.#element;
    if (!element) return;
    // First what this layout or the reader did to the place, of which no event has told yet.
    this.#read(element, stillLaidOut(element, entries?.at(-1)));
    const left = Math.min(this.#x.intended, element.scrollWidth - element.clientWidth);
    const top = Math.min(this.#y.intended, element.scrollHeight - element.clientHeight);
    if (left > element.scrollLeft) element.scrollLeft = left;
    if (top > element.scrollTop) element.scrollTop = top;
    // Where the place holds in the new box, the new box is the one it is true of.
    if (element.scrollLeft === this.#x.intended) this.#x.box = element.clientWidth;
    if (element.scrollTop === this.#y.intended) this.#y.box = element.clientHeight;
    this.#at = { left: element.scrollLeft, top: element.scrollTop };
    this.#tell(element.scrollLeft > 0);
  };
}
