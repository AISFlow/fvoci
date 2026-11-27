/**
 * The outline of the page shown: its headings, the one being read, and how
 * it is shown (`mode`), for the page's screen and its parts (`OutlineNav`,
 * `OutlineButton`, `OutlineColumns`).
 *
 * The headings are read from the body as it is drawn (outline-dom.ts), so the
 * read-only view and the editor give the same outline, and what others write
 * comes in with the editor's own drawing. They are read again only when a
 * change of the body touched a heading, at most once a frame: typing in a
 * paragraph costs a look at where the change was and nothing more.
 *
 * The heading being read is the last one whose top has passed a line a
 * quarter of the way down the visible part of the page, found by halving the
 * list (a few measurements a frame, however long the page), again whenever
 * the page scrolls or it, its column or what is above it changes size. A
 * heading the reader chose stays the one marked until they move the page
 * themselves (one near the end of the page cannot reach the line): until it
 * scrolls after a press, a wheel or a key that moves the page, not after
 * typing.
 *
 * Beside the page when the page's column has room for both, the page giving
 * up some of its width for it (`side`), unless the reader folded it away,
 * which this device remembers; otherwise behind a button (`button`). None
 * for a page with fewer than two headings, but its place is kept (`where`):
 * the page does not move as a second heading is written or taken out.
 */
import { untrack } from 'svelte';

import {
  focusHeading,
  lastAtOrAbove,
  outlineOf,
  sameOutline,
  scrollToHeading,
  touchesHeadings,
  type OutlineEntry,
} from './outline-dom.ts';

export type { OutlineEntry } from './outline-dom.ts';

/** Where the device remembers that the reader folded the outline away. */
export const OUTLINE_STORAGE_KEY = 'fvoci:wiki-outline';

/** The width the outline takes beside the page, in rem. */
const OUTLINE_WIDTH = 12;
/** The narrowest the page is drawn beside the outline, in rem: so much of its width it gives up for it. */
const NARROWEST_PAGE = 40;
/** How far below the head of the page a heading that is gone to stays, in pixels. */
const MARGIN = 16;
/** The events by which the reader may be moving the page themselves: a scroll after one is theirs. */
const MOVES = ['wheel', 'touchstart', 'pointerdown'] as const;
/** Keys that write wherever they are pressed, an input method's among them. */
const WRITING_KEYS = new Set(['Enter', 'Backspace', 'Delete', 'Process', 'Unidentified', 'Dead']);
/** The letters of the browser's Find with Control or Command: it takes the page elsewhere. */
const FIND_KEYS = new Set(['f', 'g']);
/** Keys that move nothing by themselves: the key they are held with tells. */
const MODIFIER_KEYS = new Set([
  'Shift',
  'Control',
  'Alt',
  'AltGraph',
  'Meta',
  'CapsLock',
  'Fn',
  'FnLock',
  'NumLock',
  'ScrollLock',
  'Symbol',
  'SymbolLock',
  'Hyper',
  'Super',
  'OS',
]);

export type OutlineMode = 'none' | 'side' | 'button';

/**
 * Whether a key writes: a letter or a space typed into what is edited (with
 * AltGr too), paste, cut and undo there, Enter, the deleting keys, an input
 * method's keys. The page scrolls after one only to keep the caret in sight.
 * Any other key may move the page: the arrows, PageUp and PageDown, Home and
 * End, Tab, a space outside what is edited, the browser's Find.
 */
function writes(event: KeyboardEvent): boolean {
  if (event.isComposing || WRITING_KEYS.has(event.key)) return true;
  const edited = event.target instanceof HTMLElement && event.target.isContentEditable;
  if (!edited || event.key.length !== 1) return false;
  const chord = (event.ctrlKey || event.metaKey) && !event.altKey;
  return !chord || !FIND_KEYS.has(event.key.toLowerCase());
}

function remembered(): boolean {
  try {
    return localStorage.getItem(OUTLINE_STORAGE_KEY) === 'folded';
  } catch {
    return false;
  }
}

const rem = (): number => parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;

/** Shut, whatever it is given: a `$derived` of it shuts again whenever that changes. */
function shutOn(..._given: unknown[]): boolean {
  return false;
}

export class PageOutline {
  /** The headings with words, in their order. */
  entries = $state.raw<readonly OutlineEntry[]>([]);
  /** The place in `entries` of the heading being read; -1 above the first. */
  current = $state(-1);
  /** The reader folded it away from beside the page. */
  folded = $state(remembered());

  /** The width of the page's column, its padding left out; 0 until measured. */
  #width = $state(0);
  /** The height of what shows of the page's column, in pixels. */
  #height = $state(0);
  /** The width the page itself is drawn in, in rem. */
  #page = 48;
  #source: HTMLElement | undefined;
  #scroller: HTMLElement | undefined;
  #bar: HTMLElement | undefined;
  /** The heading the reader chose, marked until they move the page themselves. */
  #chosen: HTMLElement | undefined;
  /** The reader pressed, turned the wheel or pressed a key that moves the page: a scroll now is theirs. */
  #moving = false;
  /**
   * Where the scroll to a heading gone to is going, how far it had left to go,
   * and whether a key that may move the page came during it: a scroll that
   * comes nearer is that one still, not the reader's.
   */
  #going: { to: number; left: number; keyed: boolean } | undefined;
  #stale = false;
  #frame: number | undefined;

  /** `page` is the widest the page itself is drawn, in rem (`max-w-3xl`). */
  constructor(page = 48) {
    this.#page = page;
  }

  /** The widest the page is drawn beside the outline, and the outline's own width, in rem. */
  get page(): number {
    return this.#page;
  }
  readonly side = OUTLINE_WIDTH;
  /** How much of the page's column shows, in pixels: the outline beside it is no taller. */
  get height(): number {
    return this.#height;
  }

  /** There is room beside the page for the outline, the page drawn narrower as far as it must. */
  readonly room = $derived(
    this.#width > 0 && this.#width >= (NARROWEST_PAGE + OUTLINE_WIDTH) * rem(),
  );

  /**
   * Where the outline stands when the page has headings enough for one: beside
   * the page or behind the button. Kept for a page with fewer, so that the
   * page is drawn where it would be with one.
   */
  readonly where = $derived<'side' | 'button'>(this.room && !this.folded ? 'side' : 'button');

  readonly mode = $derived<OutlineMode>(this.entries.length < 2 ? 'none' : this.where);

  /**
   * The list behind the button is out. It is shut again whenever the button goes
   * or changes what it does (the outline goes beside the page, or away, or there
   * comes room to unfold it): a list left out behind a button that went would
   * come back unasked with it.
   */
  open = $derived(shutOn(this.mode, this.room));

  /** Folds it away from beside the page, or puts it back; remembered on the device. */
  setFolded(folded: boolean): void {
    this.folded = folded;
    try {
      localStorage.setItem(OUTLINE_STORAGE_KEY, folded ? 'folded' : 'beside');
    } catch {
      // Not remembered: it still holds for this visit.
    }
  }

  /** For `{@attach}` on what holds the page's body, and only that: its headings are the outline. */
  readonly source = (element: HTMLElement): (() => void) => {
    this.#source = element;
    this.#stale = true;
    const changes = new MutationObserver((records) => {
      if (touchesHeadings(records)) this.#stale = true;
      if (this.#stale) this.#soon();
    });
    changes.observe(element, { childList: true, characterData: true, subtree: true });
    this.#soon();
    return () => {
      changes.disconnect();
      if (this.#source !== element) return;
      this.#source = undefined;
      this.#chosen = undefined;
      this.#going = undefined;
      untrack(() => {
        [this.entries, this.current, this.open] = [[], -1, false];
      });
    };
  };

  /**
   * For `{@attach}` on the element the page scrolls in. Its scrollbar's room
   * is kept, whether the page overflows or not: the layout is read from the
   * width left, and a page laid out beside the outline (narrower, so taller)
   * could otherwise come to overflow, take the outline behind the button
   * (wider, shorter), no longer overflow, and so on.
   */
  readonly follow = (element: HTMLElement): (() => void) => {
    this.#scroller = element;
    const gutter = element.style.getPropertyValue('scrollbar-gutter');
    element.style.setProperty('scrollbar-gutter', 'stable');
    const measure = (): void => {
      const style = getComputedStyle(element);
      const padding = (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0);
      this.#width = element.clientWidth - padding;
      this.#height = element.clientHeight;
      // A taller or shorter column moves the reading line.
      this.#soon();
    };
    measure();
    const sized = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(measure);
    sized?.observe(element);
    const moving = (): void => {
      this.#moving = true;
      // The reader's own scroll now, wherever it goes.
      this.#going = undefined;
    };
    const keyed = (event: KeyboardEvent): void => {
      if (MODIFIER_KEYS.has(event.key)) return;
      this.#moving = !writes(event);
      if (!this.#moving || !this.#going || event.repeat) return;
      // The first such key may cut the scroll to the heading short (End, an
      // arrow, with Shift or not), its scroll to the caret on the way there; a
      // second is the reader moving the page (a held key's repeats are not).
      if (this.#going.keyed) this.#going = undefined;
      else this.#going.keyed = true;
    };
    const scrolled = (): void => {
      if (!this.#stillGoing(element.scrollTop) && this.#moving) this.#chosen = undefined;
      this.#soon();
    };
    for (const name of MOVES)
      element.addEventListener(name, moving, { passive: true, capture: true });
    element.addEventListener('keydown', keyed, { passive: true, capture: true });
    element.addEventListener('scroll', scrolled, { passive: true });
    return () => {
      sized?.disconnect();
      for (const name of MOVES) element.removeEventListener(name, moving, { capture: true });
      element.removeEventListener('keydown', keyed, { capture: true });
      element.removeEventListener('scroll', scrolled);
      element.style.setProperty('scrollbar-gutter', gutter);
      if (this.#scroller === element) this.#scroller = undefined;
    };
  };

  /**
   * For `{@attach}` on what holds the page, its head and its body: its growing
   * or shrinking moves the headings without a scroll (a picture drawn late,
   * something above the body that opens, others' writing).
   */
  readonly holder = (element: HTMLElement): (() => void) => {
    const sized =
      typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(this.#soon);
    sized?.observe(element);
    return () => {
      sized?.disconnect();
    };
  };

  /** For `{@attach}` on a bar that stays at the head of the page as it scrolls: headings go below it. */
  readonly bar = (element: HTMLElement): (() => void) => {
    this.#bar = element;
    return () => {
      if (this.#bar === element) this.#bar = undefined;
    };
  };

  /**
   * Brings the heading at `at` to the head of the page and the focus to it
   * (outline-dom.ts says where in the editor), unless it was chosen by a
   * finger and the editor would bring up the keyboard; marked as the one
   * being read. Gives whether the focus went there.
   */
  go(at: number, finger = false): boolean {
    const entry = this.entries[at];
    const scroller = this.#scroller;
    if (!entry?.element.isConnected || !scroller) return false;
    this.open = false;
    this.#chosen = entry.element;
    this.#moving = false;
    this.current = at;
    // The focus first: WebKit brings the caret into sight as the editor takes the
    // focus, which would cut a smooth scroll short; the scroll then starts from there.
    const focused = focusHeading(entry.element, finger);
    const to = scrollToHeading(scroller, entry.element, this.#head() + MARGIN);
    const left = Math.abs(to - scroller.scrollTop);
    this.#going = left < 1 ? undefined : { to, left, keyed: false };
    return focused;
  }

  /**
   * Whether a scroll to `top` is the one to a heading gone to, coming nearer
   * to where it goes (a key that moves the caret may cut it short, where it
   * rests); done once there, once a scroll goes elsewhere, and once the
   * reader presses, touches, turns the wheel or presses a second key that may
   * move the page. (Not at `scrollend`: WebKit sends one as a key cuts the
   * scroll short, before the scroll to the caret that follows.)
   */
  #stillGoing(top: number): boolean {
    const going = this.#going;
    if (!going) return false;
    const left = Math.abs(going.to - top);
    if (left > going.left + 1) {
      this.#going = undefined;
      return false;
    }
    if (left < 1) this.#going = undefined;
    else going.left = left;
    return true;
  }

  /** How far the bar at the head of the page reaches down into it, in pixels. */
  #head(): number {
    const [bar, scroller] = [this.#bar, this.#scroller];
    if (!bar?.isConnected || !scroller) return 0;
    return Math.max(0, bar.getBoundingClientRect().bottom - scroller.getBoundingClientRect().top);
  }

  /** Reads what changed at the next frame, once however many changes come before it. */
  readonly #soon = (): void => {
    if (this.#frame !== undefined) return;
    this.#frame = requestAnimationFrame(() => {
      this.#frame = undefined;
      this.#read();
    });
  };

  #read(): void {
    const source = this.#source;
    if (!source) return;
    if (this.#stale) {
      this.#stale = false;
      const read = outlineOf(source);
      if (!sameOutline(read, this.entries)) this.entries = read;
    }
    const chosen = this.#chosen
      ? this.entries.findIndex((entry) => entry.element === this.#chosen)
      : -1;
    if (chosen < 0) this.#chosen = undefined;
    const current = chosen >= 0 ? chosen : this.#reading();
    if (current !== this.current) this.current = current;
  }

  /** The heading whose top has passed the reading line, by the page as it is now. */
  #reading(): number {
    const scroller = this.#scroller;
    if (!scroller || this.entries.length === 0) return -1;
    const top = scroller.getBoundingClientRect().top + this.#head();
    const line = top + (scroller.clientHeight - this.#head()) / 4;
    return lastAtOrAbove(
      this.entries.length,
      (at) => this.entries[at]?.element.getBoundingClientRect().top ?? Infinity,
      line,
    );
  }
}
