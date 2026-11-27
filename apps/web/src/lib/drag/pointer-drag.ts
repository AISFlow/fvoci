/**
 * One drag with a pointer, from press to release, on Pointer Events.
 *
 * This part knows nothing about lists or drop targets. It decides when a
 * press becomes a drag, follows the pointer wherever it goes, and makes sure
 * the drag ends exactly once: with a drop or with a cancel.
 *
 * A mouse starts dragging once it has moved a few pixels with the button
 * down, so a click stays a click. A finger (or a pen) starts dragging after
 * resting on the item for a moment; a finger that moves before that is
 * scrolling, and is left to the browser. The moment the resting finger has
 * picked the item up the device buzzes briefly, where it can.
 *
 * `immediate` is for a thing that is dragged along a scale of days, where a
 * few pixels are a day: a mouse takes the item at the press (`start` with
 * the point pressed), and the moves follow once it has gone the few pixels,
 * measured from the press. A button let go before that was a click: the
 * drag is cancelled and the click goes through. A finger still rests first,
 * so that a finger landing on an item can scroll.
 *
 * Escape cancels a drag unless `escapeCancels` is false. Where it is false,
 * putting the item back where it was is the way to cancel, and Escape does
 * nothing during a drag.
 */
import type { Point } from '$lib/logic/drop-target.ts';

import { DRAG_SETTINGS } from './settings.ts';

export interface PointerDragOptions {
  /** How far a mouse travels with the button down before the drag begins, in pixels. */
  mouseDistance: number;
  /** How long a finger rests before the drag begins, in milliseconds. */
  touchDelay: number;
  /** How far a resting finger may wander without giving up the drag, in pixels. */
  touchTolerance: number;
  /** How long the device buzzes when a resting finger begins a drag, in milliseconds. 0 for none. */
  touchVibration: number;
}

/** How a use of `pointerDrag` differs from a list's: the numbers above are the same everywhere. */
export interface PointerDragManner {
  /** A mouse takes the item at the press; a click is still a click (above). False by default. */
  immediate?: boolean;
  /** Escape during a drag cancels it. True by default. */
  escapeCancels?: boolean;
}

export const POINTER_DRAG_DEFAULTS: PointerDragOptions = {
  mouseDistance: DRAG_SETTINGS.mouseDistance,
  touchDelay: DRAG_SETTINGS.touchDelay,
  touchTolerance: DRAG_SETTINGS.touchTolerance,
  touchVibration: DRAG_SETTINGS.touchVibration,
};

/**
 * What one drag does. `start` is called once; after it exactly one of `drop`
 * and `cancel`. A press taken at once (`immediate`) starts at every press of
 * a mouse, a click included, and a click then ends with `cancel`: `start`
 * must not show anything a click would leave behind.
 */
export interface PointerDragSession {
  start(point: Point): void;
  move(point: Point): void;
  drop(point: Point): void;
  cancel(): void;
}

/**
 * Asked on every press of the primary button or first finger inside the
 * root. Returns the session for the thing under the pointer, or undefined
 * when there is nothing to drag there. Must not change anything itself:
 * most presses end as a click or a scroll.
 */
export type PointerDragSource = (event: PointerEvent) => PointerDragSession | undefined;

export interface PointerDragHandle {
  /** Cancels the drag in progress, if any. */
  cancel(): void;
  /** Cancels and removes every listener. */
  destroy(): void;
}

interface Press {
  session: PointerDragSession;
  pointerId: number;
  /** A finger or pen: starts by resting, and the browser may take the gesture for scrolling. */
  touch: boolean;
  origin: Point;
  last: Point;
  dragging: boolean;
  /**
   * A press taken at once (`immediate`) has gone far enough to be a drag:
   * the moves reach the session, and letting go is a drop, not a click.
   */
  moved: boolean;
  timer: ReturnType<typeof setTimeout> | undefined;
  /** Removes the listeners that only live while this press does. */
  release: () => void;
}

function pointOf(event: PointerEvent): Point {
  return { x: event.clientX, y: event.clientY };
}

/**
 * A short buzz, on devices that have one. Safari on iOS has no
 * `navigator.vibrate` at all, and a browser may refuse (before the page was
 * ever tapped, or by a setting of the device): neither is an error here.
 */
function buzz(duration: number): void {
  if (duration <= 0 || !('vibrate' in navigator)) return;
  try {
    navigator.vibrate(duration);
  } catch {
    // Nothing to do: the drag goes on without the buzz.
  }
}

/** Owns one root's press, its temporary listeners and its cancellation state. */
class PointerDragController {
  readonly #root: HTMLElement;
  readonly #source: PointerDragSource;
  readonly #settings: PointerDragOptions & Required<PointerDragManner>;
  readonly #notPassive = { passive: false };
  // Capture on the window sees input before handlers inside the page.
  readonly #capture = { capture: true };
  #press: Press | undefined;
  /** A pointer whose drag was cancelled while it was still down. */
  #spent: { touch: boolean; forget: () => void } | undefined;

  constructor(
    root: HTMLElement,
    source: PointerDragSource,
    options: Partial<PointerDragOptions> & PointerDragManner,
  ) {
    this.#root = root;
    this.#source = source;
    this.#settings = {
      ...POINTER_DRAG_DEFAULTS,
      ...options,
      immediate: options.immediate ?? false,
      escapeCancels: options.escapeCancels ?? true,
    };
    this.#root.addEventListener('pointerdown', this.#onPointerDown);
    this.#root.addEventListener('lostpointercapture', this.#onLostCapture);
    // Present before any finger lands: iOS Safari's EventRegion
    // decides at touchstart whether a listener may cancel touchmove; attaching it later
    // on long-press drag start lets native scrolling take over and cancel the pointer.
    this.#root.addEventListener('touchmove', this.#onTouchMove, this.#notPassive);
    this.#root.addEventListener('contextmenu', this.#onContextMenu);
    // Links and images would start the browser's own drag, which cancels the pointer.
    this.#root.addEventListener('dragstart', this.#preventWhilePressed);
  }

  #finish(): Press | undefined {
    const ended = this.#press;
    if (!ended) return undefined;
    this.#press = undefined;
    clearTimeout(ended.timer);
    ended.release();
    if (ended.dragging && this.#root.hasPointerCapture(ended.pointerId)) {
      this.#root.releasePointerCapture(ended.pointerId);
    }
    return ended;
  }

  /** `down`: the pointer is still pressed, as after Escape, and not gone, as after pointercancel. */
  #cancel(down: boolean): void {
    const ended = this.#finish();
    if (!ended?.dragging) return;
    if (down) this.#ignoreUntilLifted(ended);
    ended.session.cancel();
  }

  /**
   * The rest of a cancelled press does nothing: the finger still on the
   * screen does not scroll, and the button going up is not a click.
   */
  #ignoreUntilLifted(ended: Press): void {
    const lifted = (event: PointerEvent): void => {
      if (event.pointerId !== ended.pointerId) return;
      if (event.type === 'pointerup') this.#swallowNextClick();
      forget();
    };
    const forget = (): void => {
      window.removeEventListener('pointerup', lifted, this.#capture);
      window.removeEventListener('pointercancel', lifted, this.#capture);
      this.#spent = undefined;
    };
    window.addEventListener('pointerup', lifted, this.#capture);
    window.addEventListener('pointercancel', lifted, this.#capture);
    this.#spent = { touch: ended.touch, forget };
  }

  readonly #begin = (): void => {
    if (!this.#press || this.#press.dragging) return;
    clearTimeout(this.#press.timer);
    this.#press.dragging = true;
    if (this.#press.moved) this.#takePointer();
    // Only a finger that rested gets the buzz: it is the one sign that the
    // wait is over. A mouse sees the item move.
    if (this.#press.touch) buzz(this.#settings.touchVibration);
    this.#press.session.start(this.#press.last);
  };

  /**
   * The root takes the pointer: the item under it may leave the document
   * while it is dragged, and events must keep coming when the pointer leaves
   * the window. Not before the press is a drag: a captured pointer's click
   * goes to the root, and a press taken at once may still be a click.
   */
  #takePointer(): void {
    if (!this.#press) return;
    try {
      this.#root.setPointerCapture(this.#press.pointerId);
    } catch {
      // The pointer is already gone; its pointerup or pointercancel follows.
    }
  }

  /** A drag that ends with the button going up would otherwise be followed by a click. */
  #swallowNextClick(): void {
    const swallow = (event: MouseEvent): void => {
      event.stopPropagation();
      event.preventDefault();
    };
    window.addEventListener('click', swallow, { capture: true, once: true });
    // The click, if one comes, is dispatched right after pointerup, before any timer.
    setTimeout(() => {
      window.removeEventListener('click', swallow, this.#capture);
    }, 0);
  }

  readonly #onPointerMove = (event: PointerEvent): void => {
    if (event.pointerId !== this.#press?.pointerId) return;
    this.#press.last = pointOf(event);
    const travelled = Math.hypot(
      this.#press.last.x - this.#press.origin.x,
      this.#press.last.y - this.#press.origin.y,
    );
    if (this.#press.dragging) {
      // Taken at once: the moves follow once it has gone far enough to be no click.
      if (!this.#press.moved && travelled >= this.#settings.mouseDistance) {
        this.#press.moved = true;
        this.#takePointer();
      }
      if (this.#press.moved) this.#press.session.move(this.#press.last);
      return;
    }
    if (this.#press.touch) {
      // Moved before the wait was over: this is a scroll, not a drag.
      if (travelled > this.#settings.touchTolerance) this.#finish();
    } else if (travelled >= this.#settings.mouseDistance) {
      this.#begin();
    }
  };

  readonly #onPointerUp = (event: PointerEvent): void => {
    if (event.pointerId !== this.#press?.pointerId) return;
    const ended = this.#finish();
    if (!ended?.dragging) return;
    if (!ended.moved) {
      // Taken at once and let go where it was pressed: a click, which goes through.
      ended.session.cancel();
      return;
    }
    this.#swallowNextClick();
    ended.session.drop(pointOf(event));
  };

  readonly #onPointerCancel = (event: PointerEvent): void => {
    if (event.pointerId === this.#press?.pointerId) this.#cancel(false);
  };

  readonly #onLostCapture = (event: PointerEvent): void => {
    // After a regular pointerup the press is already over and this is ignored.
    if (event.pointerId === this.#press?.pointerId && this.#press.dragging) this.#cancel(true);
  };

  readonly #onKeyDown = (event: KeyboardEvent): void => {
    if (event.key !== 'Escape' || !this.#press) return;
    if (!this.#settings.escapeCancels) {
      // Nothing else reacts to it mid-drag either: a column or a dialog does not close under it.
      if (this.#press.dragging) {
        event.preventDefault();
        event.stopPropagation();
      }
      return;
    }
    if (this.#press.dragging) {
      // Escape ends the drag and nothing else, such as a dialog around the board.
      event.preventDefault();
      event.stopPropagation();
    }
    this.#cancel(true);
  };

  /**
   * Once a finger drags, the browser must not also scroll with it. Pointer
   * Events offer no way to say so after the touch has begun (`touch-action`
   * is read when the finger lands), so the touch event is cancelled instead.
   */
  readonly #onTouchMove = (event: Event): void => {
    const held = this.#press?.dragging ? this.#press.touch : this.#spent?.touch;
    if (held && event.cancelable) event.preventDefault();
  };

  readonly #preventWhilePressed = (event: Event): void => {
    if (this.#press) event.preventDefault();
  };

  /** A long press also opens the context menu on touch screens. */
  readonly #onContextMenu = (event: Event): void => {
    if (this.#press?.touch) event.preventDefault();
  };

  readonly #onPointerDown = (event: PointerEvent): void => {
    if (this.#press || !event.isPrimary || event.button !== 0) return;
    this.#spent?.forget();
    const source = this.#source;
    const session = source(event);
    if (!session) return;

    const touch = event.pointerType !== 'mouse';
    const origin = pointOf(event);
    const target = event.target;

    window.addEventListener('pointermove', this.#onPointerMove, this.#capture);
    window.addEventListener('pointerup', this.#onPointerUp, this.#capture);
    window.addEventListener('pointercancel', this.#onPointerCancel, this.#capture);
    window.addEventListener('keydown', this.#onKeyDown, this.#capture);
    document.addEventListener('selectstart', this.#preventWhilePressed);
    // Touch events go to the element the finger landed on even after it has
    // left the document, where they no longer reach the root.
    if (touch) target?.addEventListener('touchmove', this.#onTouchMove, this.#notPassive);

    this.#press = {
      session,
      pointerId: event.pointerId,
      touch,
      origin,
      last: origin,
      dragging: false,
      // A drag that is not taken at once has gone far enough by the time it begins.
      moved: touch || !this.#settings.immediate,
      timer: touch ? setTimeout(this.#begin, this.#settings.touchDelay) : undefined,
      release: () => {
        window.removeEventListener('pointermove', this.#onPointerMove, this.#capture);
        window.removeEventListener('pointerup', this.#onPointerUp, this.#capture);
        window.removeEventListener('pointercancel', this.#onPointerCancel, this.#capture);
        window.removeEventListener('keydown', this.#onKeyDown, this.#capture);
        document.removeEventListener('selectstart', this.#preventWhilePressed);
        target?.removeEventListener('touchmove', this.#onTouchMove);
      },
    };
    if (!touch && this.#settings.immediate) this.#begin();
  };

  cancel(): void {
    this.#cancel(true);
  }

  destroy(): void {
    this.#cancel(false);
    this.#spent?.forget();
    this.#root.removeEventListener('pointerdown', this.#onPointerDown);
    this.#root.removeEventListener('lostpointercapture', this.#onLostCapture);
    this.#root.removeEventListener('touchmove', this.#onTouchMove);
    this.#root.removeEventListener('contextmenu', this.#onContextMenu);
    this.#root.removeEventListener('dragstart', this.#preventWhilePressed);
  }
}

export function pointerDrag(
  root: HTMLElement,
  source: PointerDragSource,
  options: Partial<PointerDragOptions> & PointerDragManner = {},
): PointerDragHandle {
  const controller = new PointerDragController(root, source, options);
  return {
    cancel() {
      controller.cancel();
    },
    destroy() {
      controller.destroy();
    },
  };
}
