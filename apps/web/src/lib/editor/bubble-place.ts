/**
 * Where the bubble over a selection stands (SelectionBubbleBar.svelte): over
 * what is chosen, its middle over the middle of it, never past an edge of
 * the window; under it where there is no room above inside the box that
 * scrolls the body, or where `below` asks (a touch screen puts its own menu
 * over a selection).
 *
 * The bubble is placed inside the editor's frame (`position: absolute`), not
 * against the window: the dialog of a card moves its content, and what is
 * fixed in it would be fixed to the dialog.
 */
import type { Editor } from '@tiptap/core';
import { NodeSelection } from '@tiptap/pm/state';

const GAP = 8;
const EDGE = 8;

/** The rectangle of what is chosen, in the window: an image's box, or the ends of the text. */
export function selectionRect(editor: Editor): DOMRect | undefined {
  const { view, state } = editor;
  const { selection } = state;
  if (selection instanceof NodeSelection) {
    const dom = view.nodeDOM(selection.from);
    return dom instanceof HTMLElement ? dom.getBoundingClientRect() : undefined;
  }
  try {
    const start = view.coordsAtPos(selection.from);
    const end = view.coordsAtPos(selection.to);
    // On two lines the ends are far apart: the whole width of the lines, then.
    const oneLine = Math.abs(start.top - end.top) < 2;
    const left = oneLine ? Math.min(start.left, end.left) : view.dom.getBoundingClientRect().left;
    const right = oneLine
      ? Math.max(start.right, end.right)
      : view.dom.getBoundingClientRect().right;
    return new DOMRect(left, start.top, right - left, end.bottom - start.top);
  } catch {
    // A position the view has not drawn (it is being made anew).
    return undefined;
  }
}

/** The box that scrolls `element` up and down; undefined where the window does. */
export function scrollingBox(element: HTMLElement): HTMLElement | undefined {
  for (let at = element.parentElement; at; at = at.parentElement) {
    const { overflowY } = getComputedStyle(at);
    if (overflowY === 'auto' || overflowY === 'scroll') return at;
  }
  return undefined;
}

/** Puts the bubble next to `target`, a rectangle in the window. */
export function placeBubble(bubble: HTMLElement, target: DOMRect, below: boolean): void {
  const frame = bubble.offsetParent;
  if (!(frame instanceof HTMLElement)) return;
  const origin = frame.getBoundingClientRect();
  const width = bubble.offsetWidth;
  const height = bubble.offsetHeight;
  const page = bubble.ownerDocument.defaultView;
  const right = (page?.innerWidth ?? origin.right) - EDGE - width;
  const left = Math.max(EDGE, Math.min(target.left + target.width / 2 - width / 2, right));
  const above = target.top - GAP - height;
  const under =
    below || above < Math.max(EDGE, scrollingBox(frame)?.getBoundingClientRect().top ?? 0);
  const top = under ? target.bottom + GAP : above;
  bubble.style.left = `${String(left - origin.left)}px`;
  bubble.style.top = `${String(top - origin.top)}px`;
  bubble.dataset.side = under ? 'bottom' : 'top';
}
