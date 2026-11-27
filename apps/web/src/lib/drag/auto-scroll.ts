/**
 * Scrolling while dragging: a pointer held near the edge of a scrolling box
 * moves its content, faster the nearer it is to the edge.
 *
 * The caller owns the frame loop and says on every frame which boxes may
 * scroll. This part does the arithmetic of one frame and remembers where
 * every box was before it first moved it, so that a cancelled drag can put
 * them back.
 */
import { edgeScrollFactor, type Axis, type Point } from '$lib/logic/drop-target.ts';

import { DRAG_SETTINGS } from './settings.ts';

export interface AutoScrollOptions {
  /** Width of the zone at each edge in which the pointer scrolls, in pixels. */
  edge: number;
  /** Speed with the pointer at or beyond the edge, in pixels per second. */
  maxSpeed: number;
}

export const AUTO_SCROLL_DEFAULTS: AutoScrollOptions = {
  edge: DRAG_SETTINGS.scrollEdge,
  maxSpeed: DRAG_SETTINGS.scrollMaxSpeed,
};

/** A box that may scroll along one axis. */
export interface ScrollBox {
  element: HTMLElement;
  axis: Axis;
}

export class AutoScroller {
  readonly #options: AutoScrollOptions;
  /** Scroll offsets at the first time a box was moved. */
  readonly #before = new Map<HTMLElement, { left: number; top: number }>();
  /**
   * Parts of a pixel not yet scrolled. A slow scroll moves less than a pixel
   * per frame, and offsets are whole pixels on many screens.
   */
  readonly #owed = new Map<HTMLElement, number>();

  constructor(options: Partial<AutoScrollOptions> = {}) {
    this.#options = { ...AUTO_SCROLL_DEFAULTS, ...options };
  }

  /**
   * Scrolls each box as far as the pointer asks for in a frame of `elapsed`
   * milliseconds. Returns whether anything moved.
   */
  step(point: Point, boxes: readonly ScrollBox[], elapsed: number): boolean {
    let moved = false;
    for (const { element, axis } of boxes) {
      const rect = element.getBoundingClientRect();
      const factor =
        axis === 'y'
          ? edgeScrollFactor(point.y, rect.top, rect.height, this.#options.edge)
          : edgeScrollFactor(point.x, rect.left, rect.width, this.#options.edge);
      if (factor === 0) {
        this.#owed.delete(element);
        continue;
      }
      const distance =
        (this.#owed.get(element) ?? 0) + (factor * this.#options.maxSpeed * elapsed) / 1000;
      const whole = Math.trunc(distance);
      this.#owed.set(element, distance - whole);
      if (whole === 0) continue;

      const left = element.scrollLeft;
      const top = element.scrollTop;
      if (!this.#before.has(element)) this.#before.set(element, { left, top });
      if (axis === 'y') element.scrollTop = top + whole;
      else element.scrollLeft = left + whole;
      if (element.scrollLeft !== left || element.scrollTop !== top) moved = true;
    }
    return moved;
  }

  /** Puts every box back where it was before this scroller first moved it. */
  restore(): void {
    for (const [element, { left, top }] of this.#before) {
      element.scrollLeft = left;
      element.scrollTop = top;
    }
    this.clear();
  }

  /** Forgets what was moved: the drag ended and the boxes stay where they are. */
  clear(): void {
    this.#before.clear();
    this.#owed.clear();
  }
}
