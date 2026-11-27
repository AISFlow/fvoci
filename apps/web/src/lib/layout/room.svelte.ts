/**
 * What a screen is laid out by. None of it is the User-Agent, and none of it
 * is a guess at the kind of device.
 *
 * - **Room.** How wide the box is that a part sits in. Two boxes are
 *   measured: `page`, the window less what the device keeps at its left and
 *   right edges, and `pane`, what is left of that for the board. The widths
 *   at which the layout changes are written once, in styles/app.css; classes
 *   use them as container queries, and this file reads the same variables
 *   for what a class cannot decide.
 * - **Pointer and hover** decide sizes and what may stay hidden, never the
 *   layout. In markup those are the `roomy:` and `can-hover:` variants.
 *
 * Nothing here is read once and kept. A box is measured whenever it changes
 * (a fold opens, the window is split, the device turns), and a width from
 * the style sheet is turned into pixels at the moment it is compared, so a
 * change of the reader's text size is followed too.
 */
import { MediaQuery } from 'svelte/reactivity';

/**
 * The three stages of a page. `compact`: a drawer, sheets, a card that fills
 * the screen. `medium`: a strip of the sidebar, popovers, a card that is a
 * panel at the side. `expanded`: a fixed sidebar, a card that is a dialog.
 */
export type Stage = 'compact' | 'medium' | 'expanded';

/** A length the style sheet gives in rem, in pixels at the reader's text size. */
function boundary(name: string): number {
  const root = document.documentElement;
  const style = getComputedStyle(root);
  const value = style.getPropertyValue(name).trim();
  // A page without the style sheet (a unit test) has no boundaries: everything is wide.
  if (!value.endsWith('rem')) return 0;
  return parseFloat(value) * parseFloat(style.fontSize);
}

/** The size of the content of one element, kept up to date. */
export class Room {
  /** 0 until the element is on the page. */
  width = $state(0);
  height = $state(0);

  /** False while nothing was measured: nobody should act on a width of 0. */
  readonly measured = $derived(this.width > 0);

  /** Follows an element from now on. For `{@attach}`. */
  readonly attach = (element: HTMLElement): (() => void) => {
    const style = getComputedStyle(element);
    this.width =
      element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    this.height =
      element.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
    const observer = new ResizeObserver((entries) => {
      const entry = entries.at(-1);
      if (!entry) return;
      this.width = entry.contentRect.width;
      this.height = entry.contentRect.height;
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  };
}

/** The page: the window less the edges the device keeps. */
export class PageRoom extends Room {
  /**
   * The stage by width alone. A page too low for the wide layouts (a phone
   * on its side) is `compact` whatever its width.
   */
  readonly stage = $derived.by<Stage>(() => {
    if (!this.measured) return 'expanded';
    if (this.height > 0 && this.height < boundary('--stage-min-height')) return 'compact';
    if (this.width >= boundary('--container-expanded')) return 'expanded';
    return this.width >= boundary('--container-medium') ? 'medium' : 'compact';
  });
}

/** What the board has, once the sidebar has taken its part. */
export class PaneRoom extends Room {
  /** Two columns of a kanban fit side by side. */
  readonly columns = $derived(this.width >= boundary('--container-columns'));
  /** A wide pane: wider columns of a kanban and a table. */
  readonly header = $derived(this.width >= boundary('--container-header'));
  /** The header of a view fits in one row, with every tool. */
  readonly row = $derived(this.width >= boundary('--container-view-row'));
}

/** The room the device keeps at each edge, in pixels: for parts that are placed by script. */
class Insets {
  top = $state(0);
  right = $state(0);
  bottom = $state(0);
  left = $state(0);
  /** The height the on-screen keyboard covers where the page was not shrunk for it. */
  keyboard = $state(0);
  #probe: HTMLElement | undefined;

  /** Starts reading the values; they change when the device turns or folds. */
  watch(): () => void {
    this.#probe = document.createElement('div');
    this.#probe.style.cssText =
      'position:fixed;visibility:hidden;pointer-events:none;' +
      'padding:var(--safe-top) var(--safe-right) var(--safe-bottom) var(--safe-left)';
    document.body.append(this.#probe);
    this.refresh();
    window.addEventListener('resize', this.refresh);
    return () => {
      window.removeEventListener('resize', this.refresh);
      this.#probe?.remove();
      this.#probe = undefined;
    };
  }

  readonly refresh = (): void => {
    if (!this.#probe) return;
    const style = getComputedStyle(this.#probe);
    this.top = parseFloat(style.paddingTop) || 0;
    this.right = parseFloat(style.paddingRight) || 0;
    this.bottom = parseFloat(style.paddingBottom) || 0;
    this.left = parseFloat(style.paddingLeft) || 0;
  };

  /** The distance a popover keeps from each edge of the screen. */
  get collisionPadding(): { top: number; right: number; bottom: number; left: number } {
    return {
      top: 8 + this.top,
      right: 8 + this.right,
      // A popover next to a field stays above the keyboard that came up for it.
      bottom: 8 + this.bottom + this.keyboard,
      left: 8 + this.left,
    };
  }
}

/**
 * Whether the main pointer is a finger. For the one thing that is neither
 * layout nor size: a finger has not asked for the keyboard, so nothing that
 * opens under one puts the focus into a field.
 */
export const fingerPointer = new MediaQuery('(pointer: coarse)');

/** The page. The entry point attaches it to the element the app is mounted in. */
export const page = new PageRoom();

/** What the board has. The shell attaches it to the box the board is drawn in. */
export const pane = new PaneRoom();

export const insets = new Insets();
