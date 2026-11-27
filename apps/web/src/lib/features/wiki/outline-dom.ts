/**
 * What the outline of a page reads from the page's body as it is drawn, and
 * how it brings a heading of it to the reader. Nothing here writes into the
 * body: the editor's document stays as it is, and so does what a reader by a
 * link was sent (only what is drawn is read).
 *
 * The body is the read-only view (`BodyView`) or the editor; either draws a
 * heading of level 1 to 3 as `h1` to `h3` inside its `.doc-body`.
 */
import { MATH_CLASS, MATH_SOURCE_ATTRIBUTE } from '$lib/editor/drawing.ts';

const HEADING = 'h1, h2, h3';
const BODY_HEADINGS = '.doc-body h1, .doc-body h2, .doc-body h3';

export interface OutlineEntry {
  /** The same for as long as the heading is drawn by the same element. */
  readonly key: number;
  readonly element: HTMLElement;
  readonly level: 1 | 2 | 3;
  readonly text: string;
}

/** The text of a heading as it reads: a formula as its source, a line break as a space, nothing hidden. */
export function headingText(heading: Element): string {
  let text = '';
  const walk = (node: Node): void => {
    if (node.nodeType === Node.TEXT_NODE) {
      text += node.nodeValue ?? '';
      return;
    }
    if (!(node instanceof Element) || node.getAttribute('aria-hidden') === 'true') return;
    if (node.classList.contains(MATH_CLASS)) {
      text += ` ${node.getAttribute(MATH_SOURCE_ATTRIBUTE) ?? ''} `;
      return;
    }
    if (node.tagName === 'BR') text += ' ';
    for (const child of node.childNodes) walk(child);
  };
  walk(heading);
  return text.replace(/\s+/g, ' ').trim();
}

const keys = new WeakMap<Element, number>();
let lastKey = 0;

function keyOf(element: Element): number {
  let key = keys.get(element);
  if (key === undefined) {
    lastKey += 1;
    key = lastKey;
    keys.set(element, key);
  }
  return key;
}

/** The headings of the bodies under `root`, in their order; those without words are left out. */
export function outlineOf(root: ParentNode): OutlineEntry[] {
  const entries: OutlineEntry[] = [];
  for (const element of root.querySelectorAll<HTMLElement>(BODY_HEADINGS)) {
    const text = headingText(element);
    if (text === '') continue;
    const level = Number(element.tagName.slice(1)) as 1 | 2 | 3;
    entries.push({ key: keyOf(element), element, level, text });
  }
  return entries;
}

/** Whether two outlines are the same headings with the same words. */
export function sameOutline(a: readonly OutlineEntry[], b: readonly OutlineEntry[]): boolean {
  return (
    a.length === b.length &&
    a.every(
      (entry, at) =>
        entry.element === b[at]?.element &&
        entry.level === b[at].level &&
        entry.text === b[at].text,
    )
  );
}

/**
 * Whether changes of the body may have changed its headings: a heading's
 * words, or a heading put in or taken out. Typing elsewhere does not, and is
 * told apart from them at the cost of a few steps up from where it happened.
 */
export function touchesHeadings(records: readonly MutationRecord[]): boolean {
  const isOrHasHeading = (node: Node): boolean =>
    node instanceof Element && (node.matches(HEADING) || node.querySelector(HEADING) !== null);
  return records.some((record) => {
    const at = record.target instanceof Element ? record.target : record.target.parentElement;
    if (at?.closest(HEADING)) return true;
    return (
      record.type === 'childList' &&
      (Array.from(record.addedNodes).some(isOrHasHeading) ||
        Array.from(record.removedNodes).some(isOrHasHeading))
    );
  });
}

/**
 * The last of `count` places, in the order of their tops, whose top is at
 * `line` or above it; -1 when none is. Asks for the tops of a few only.
 */
export function lastAtOrAbove(count: number, topOf: (at: number) => number, line: number): number {
  let [low, high] = [0, count - 1];
  let found = -1;
  while (low <= high) {
    const middle = (low + high) >> 1;
    if (topOf(middle) <= line) {
      found = middle;
      low = middle + 1;
    } else {
      high = middle - 1;
    }
  }
  return found;
}

/**
 * Focuses `element` for as long as it keeps the focus. One that cannot take
 * it is made able to (`tabindex="-1"`) until it loses it again: so a body
 * that later turns editable keeps its place in the order of the Tab key.
 */
function focusOnce(element: HTMLElement): void {
  if (element.tabIndex < 0 && !element.hasAttribute('tabindex') && !element.isContentEditable) {
    element.setAttribute('tabindex', '-1');
    element.addEventListener(
      'blur',
      () => {
        element.removeAttribute('tabindex');
      },
      { once: true },
    );
  }
  element.focus({ preventScroll: true });
}

/**
 * Puts the focus on a heading of the body. In the editor the heading is the
 * editor's own: nothing is set on it (the editor would read that as a change
 * of its text); the editor takes the focus with its caret at the start of the
 * heading. A heading of the read-only view takes the focus itself.
 *
 * Under a finger the editor is left alone when it is editable: the keyboard
 * would come up over the heading the reader asked to see. Gives whether the
 * focus went to the heading.
 */
export function focusHeading(heading: HTMLElement, finger: boolean): boolean {
  const editor = heading.closest<HTMLElement>('[data-body-editor]');
  if (!editor) {
    focusOnce(heading);
    return true;
  }
  if (finger && editor.isContentEditable) return false;
  // The caret first: an editor taking the focus shows where its caret is
  // (WebKit scrolls to it whatever `preventScroll` says), here the heading.
  getSelection()?.collapse(heading, 0);
  focusOnce(editor);
  return true;
}

/**
 * Scrolls `scroller` so that the top of `heading` is `below` pixels under
 * its top edge, as far as it scrolls, without the motion where the reader
 * asked for less of it. Gives where it scrolls to.
 */
export function scrollToHeading(
  scroller: HTMLElement,
  heading: HTMLElement,
  below: number,
): number {
  const wanted =
    scroller.scrollTop +
    heading.getBoundingClientRect().top -
    scroller.getBoundingClientRect().top -
    below;
  const top = Math.max(0, Math.min(wanted, scroller.scrollHeight - scroller.clientHeight));
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  scroller.scrollTo({ top, behavior: reduced ? 'auto' : 'smooth' });
  return top;
}
