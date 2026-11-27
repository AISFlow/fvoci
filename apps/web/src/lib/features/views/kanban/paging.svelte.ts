/**
 * Which column of the board is looked at, and keeping it in view.
 *
 * The board is a row of panels that scrolls sideways: the columns, then
 * what stands after them (`data-panel`). In a pane that fits one column the
 * board comes to rest on a panel (CSS scroll snap) and `current` is the one
 * it rests on. Where several columns fit, the board scrolls freely and
 * `current` only changes when the panel it names has left the view.
 *
 * So a pane that changes between the two (a device unfolded, a window
 * resized, a card opened beside the board) comes back to the column that
 * was looked at: in the middle where one fits, in view where several do.
 */
import { untrack } from 'svelte';

import { nearestRest } from './kanban.ts';

/** How far from the left edge a column is put that is brought into view among several, in pixels. */
const LEFT_MARGIN = 12;

export class Paging {
  /** The element that scrolls. For `bind:this`. */
  scroller = $state<HTMLElement | null>(null);
  /** The panel that is looked at, by its place in the row. */
  current = $state(0);
  readonly #one: () => boolean;
  /**
   * The width of the board's box as its observer last saw it: a scroll at another is the
   * layout's, not the reader's. The box the observer watches, so none is waited for in vain.
   */
  #width = 0;

  /**
   * Call while a component is being set up. `one`: the pane fits one column
   * at a time. `count`: how many panels there are.
   */
  constructor(one: () => boolean, count: () => number) {
    this.#one = one;
    $effect(() => {
      const within = this.scroller;
      const snapping = one();
      if (!within) return;
      // On the first frame too, with no animation to wait for.
      this.#width = within.offsetWidth;
      untrack(() => {
        this.#place(within, snapping);
      });
    });
    /** The widths last seen of the panels. */
    const widths = new WeakMap<Element, number>();
    $effect(() => {
      const within = this.scroller;
      if (!within) return;
      // WebKit gives the columns their new widths from container queries a frame after the
      // board has its own, and snaps the board anew on the way: where one column fits, the
      // panel looked at is placed again once they have them.
      const observer = new ResizeObserver((entries) => {
        let resized = within.offsetWidth !== this.#width;
        let returned = false;
        for (const { target, contentRect } of entries) {
          if (target === within) continue;
          const seen = widths.get(target);
          // A panel taken out of the row (a column a pointer drags) or put back is no new width.
          if (seen && contentRect.width && seen !== contentRect.width) resized = true;
          if (seen === 0 && contentRect.width) returned = true;
          widths.set(target, contentRect.width);
        }
        this.#width = within.offsetWidth;
        if (!this.#one()) return;
        if (resized && !within.hasAttribute('data-dragging')) this.#place(within, true);
        // Put back, where it was or elsewhere in the row: the board rests on what it rests on now.
        else if (returned) this.#follow(within);
      });
      observer.observe(within, { box: 'border-box' });
      // The panels of the row as they come and go: another view may bring as many others.
      const watch = (): void => {
        for (const panel of this.#panels()) observer.observe(panel);
      };
      const row = new MutationObserver((records) => {
        for (const { removedNodes } of records) {
          for (const node of removedNodes) if (node instanceof Element) observer.unobserve(node);
        }
        watch();
      });
      row.observe(within, { childList: true });
      watch();
      return () => {
        row.disconnect();
        observer.disconnect();
      };
    });
    // A panel that went away (a column hidden or deleted) leaves the one before it looked at.
    $effect(() => {
      const last = Math.max(0, count() - 1);
      if (untrack(() => this.current) > last) this.current = last;
    });
  }

  /** Puts the panel looked at in the middle where one fits, or in view where several do. */
  #place(within: HTMLElement, snapping: boolean): void {
    const element = this.#panels()[this.current];
    if (!element) return;
    if (snapping) {
      const rest = this.#restOf(element, within);
      if (!Number.isNaN(rest)) within.scrollLeft = rest;
    } else if (!this.#wholeInView(element, within)) {
      within.scrollLeft = element.offsetLeft - LEFT_MARGIN;
    }
  }

  #panels(): HTMLElement[] {
    return [...(this.scroller?.querySelectorAll<HTMLElement>(':scope > [data-panel]') ?? [])];
  }

  /** Where the board has to be scrolled to for a panel to sit in the middle. */
  #restOf(element: HTMLElement, within: HTMLElement): number {
    // A panel that is not laid out (a column a pointer drags) rests nowhere.
    if (element.offsetParent === null) return Number.NaN;
    return element.offsetLeft - (within.clientWidth - element.clientWidth) / 2;
  }

  /** Some part of the panel shows. */
  #inView(element: HTMLElement, within: HTMLElement): boolean {
    return (
      element.offsetParent !== null &&
      element.offsetLeft < within.scrollLeft + within.clientWidth &&
      element.offsetLeft + element.offsetWidth > within.scrollLeft
    );
  }

  /** All of the panel shows. */
  #wholeInView(element: HTMLElement, within: HTMLElement): boolean {
    return (
      element.offsetParent !== null &&
      element.offsetLeft >= within.scrollLeft &&
      element.offsetLeft + element.offsetWidth <= within.scrollLeft + within.clientWidth
    );
  }

  /** The board scrolled: by a finger, the wheel, a drag at its edge, or `show`. */
  readonly onScroll = (): void => {
    const within = this.scroller;
    if (!within) return;
    // The board has a new width that is not laid out yet: WebKit snaps it anew on the way.
    if (within.offsetWidth !== this.#width) return;
    this.#follow(within);
  };

  /** Takes the panel the board is scrolled to for the one looked at. */
  #follow(within: HTMLElement): void {
    const all = this.#panels();
    if (this.#one()) {
      this.current = nearestRest(
        all.map((element) => this.#restOf(element, within)),
        within.scrollLeft,
      );
      return;
    }
    const looked = all[this.current];
    if (looked && this.#inView(looked, within)) return;
    this.current = Math.max(
      0,
      all.findIndex((element) => this.#inView(element, within)),
    );
  }

  /** Back to the first panel: another view is shown in the same place. */
  reset(): void {
    this.current = 0;
    if (this.scroller) this.scroller.scrollLeft = 0;
  }

  /** Brings a panel to the middle: a tab was pressed. */
  show(at: number): void {
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    this.#panels()[at]?.scrollIntoView({
      behavior: reduced ? 'auto' : 'smooth',
      inline: 'center',
      block: 'nearest',
    });
  }
}
