/**
 * The numbers that decide how dragging feels, in one place.
 *
 * These are the defaults. A caller can still override any of them for one
 * use (`pointer` and `scroll` in the options of `Sortable`, the options of
 * `pointerDrag` and `AutoScroller`); to change how dragging feels everywhere,
 * change them here.
 */
export const DRAG_SETTINGS = {
  /** How far a mouse travels with the button down before the drag begins, in pixels. */
  mouseDistance: 4,
  /** How long a finger rests on an item before the drag begins, in milliseconds. */
  touchDelay: 300,
  /** How far a resting finger may wander without giving up the drag, in pixels. */
  touchTolerance: 8,
  /**
   * How long the device buzzes when a resting finger has picked an item up,
   * in milliseconds. 0 for no buzz.
   */
  touchVibration: 15,
  /** Width of the strip along each edge of a scrolling box in which the pointer scrolls it, in pixels. */
  scrollEdge: 56,
  /** Scrolling speed with the pointer at the very edge, in pixels per second. */
  scrollMaxSpeed: 900,
  /**
   * The longest time one frame may stand for when scrolling, in milliseconds:
   * a frame that took longer (a hidden tab, a slow layout) must not turn into
   * a jump.
   */
  longestFrame: 50,
  /**
   * How much the height of the window may change during a drag without
   * ending it, in pixels: the browser of a phone slides its address bar in
   * and out. Any change of the width ends the drag (a device unfolded or
   * turned, a window put next to another).
   */
  resizeTolerance: 120,
} as const;
