/**
 * A move of the reader's caret made by a key the browser handles itself (an
 * arrow, Home, End) is in the page before the editor knows of it: the
 * browser says so a moment later. Whatever comes from the others in that
 * moment, a caret of theirs to draw or what they wrote, has the editor write
 * the selection it still holds into the page, and the move is taken back.
 *
 * So the selection of the page is read before either:
 *
 * - What the others wrote: whoever brings it is given the reading
 *   (`readsFirst`) and asks for it before the change enters the shared
 *   document. The binding then puts the selection back from where the
 *   reader moved to.
 * - A caret of theirs: the binding asks for it to be drawn with a
 *   transaction made a moment before it is applied. The reading is done in
 *   between, and where it found something the drawing is asked for again,
 *   of what the editor holds now.
 */
import { Extension } from '@tiptap/core';
import type { Transaction } from '@tiptap/pm/state';
import type { EditorView } from '@tiptap/pm/view';
import { yCursorPluginKey } from '@tiptap/y-tiptap';

export interface MovesKeptOptions {
  /**
   * Takes what has the editor read the selection of the page, to be called
   * before a change of the others is taken into the shared document; gives
   * what takes it back, as the editor goes away.
   */
  readsFirst: ((read: () => void) => () => void) | undefined;
}

/**
 * Has the editor read the page now (the selection, and whatever was written
 * and not yet read), where it waits for the browser's word. The view has no
 * public name for it: this is the reading it does on that word. Not during
 * a composition, which the editor reads in its own time. Whatever else has
 * the editor redraw at a moment of its own does this first, as the colours
 * of code blocks do (code-highlight.ts).
 */
export function readPage(view: EditorView): void {
  if (view.isDestroyed || view.composing) return;
  try {
    (view as unknown as { domObserver?: { flush?: () => void } }).domObserver?.flush?.();
  } catch {
    // What could not be read now is read on the browser's word, as it was before.
  }
}

/** Whether a transaction only has the others' carets drawn again. */
function drawsCarets(transaction: Transaction): boolean {
  const drawn = transaction.getMeta(yCursorPluginKey) as { awarenessUpdated?: boolean } | undefined;
  return (
    drawn?.awarenessUpdated === true &&
    !transaction.docChanged &&
    !transaction.selectionSet &&
    !transaction.storedMarksSet
  );
}

/** The extension that keeps a move of the reader's caret when something of the others comes right behind it. */
export const MovesKept = Extension.create<MovesKeptOptions, { leave: (() => void) | undefined }>({
  name: 'movesKept',

  addOptions() {
    return { readsFirst: undefined };
  },

  addStorage() {
    return { leave: undefined };
  },

  onCreate() {
    const { view } = this.editor;
    this.storage.leave = this.options.readsFirst?.(() => {
      readPage(view);
    });
  },

  onDestroy() {
    this.storage.leave?.();
    this.storage.leave = undefined;
  },

  dispatchTransaction({ transaction, next }) {
    const { view } = this.editor;
    if (!drawsCarets(transaction)) {
      next(transaction);
      return;
    }
    const held = view.state;
    readPage(view);
    if (view.state === held) {
      next(transaction);
      return;
    }
    // Something was read: the transaction is of a state the editor holds no more.
    const drawn: unknown = transaction.getMeta(yCursorPluginKey);
    view.dispatch(view.state.tr.setMeta(yCursorPluginKey, drawn));
  },
});
