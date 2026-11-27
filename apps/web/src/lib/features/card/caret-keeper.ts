/**
 * Where the reader's caret goes when the body's editor is made: where they
 * tapped the body while it was still shown read-only (the room's document
 * had not arrived), or, when the editor is made again for another document
 * of the room while the reader was in it, where the caret was in the one
 * before. Both are held as a point within the body (from its top left
 * corner, so that scrolling the card meanwhile does not move it) and found
 * in the new editor once it is there: no position of one document is used
 * in another, or after the wait. When no line of text is found there, the
 * new editor neither takes a caret nor the focus.
 *
 * Only a press on the read-only view that did not move (a tap, a click, not
 * a scroll) counts. The caret is followed only while the editor has the
 * focus, once a frame at most. Nothing here imports the editor's code: this
 * is part of the first screen, and the editor is not (editor-chunk.svelte.ts).
 */
import type { Editor } from '@tiptap/core';

/** How long a tap on the read-only body still counts once the editor is there. */
const PRESS_KEPT = 5000;
/** How far a press may move, in pixels, and still be a tap. */
const SLOP = 8;

/** A point within the body, from its top left corner. */
interface Point {
  dx: number;
  dy: number;
}

export interface CaretKeeper {
  /** Watches the taps on the read-only view of the body: an attachment. */
  watch: (view: HTMLElement) => () => void;
  /** An editor was made: the caret is put where it belongs, if anywhere. */
  take: (editor: Editor, writable: boolean) => void;
}

/** Where the caret of an editor is drawn, within the editor; undefined when it is not drawn. */
function caretOf(editor: Editor): Point | undefined {
  try {
    const box = editor.view.dom.getBoundingClientRect();
    const drawn = editor.view.coordsAtPos(editor.state.selection.head);
    return { dx: drawn.left - box.left, dy: (drawn.top + drawn.bottom) / 2 - box.top };
  } catch {
    return undefined;
  }
}

/** Puts the caret at a point of the body in a new editor, with the focus, if a line is there. */
function placeAt(editor: Editor, point: Point): void {
  try {
    const box = editor.view.dom.getBoundingClientRect();
    const found = editor.view.posAtCoords({ left: box.left + point.dx, top: box.top + point.dy });
    if (!found || !editor.state.doc.resolve(found.pos).parent.inlineContent) return;
    editor.commands.setTextSelection(found.pos);
    // ProseMirror's own focus, which does not scroll the page.
    editor.view.focus();
  } catch {
    // Not found in this drawing (inside a widget): the new editor's own caret stands.
  }
}

export function keepCaret(): CaretKeeper {
  let press: (Point & { at: number }) | undefined;
  /** The caret of the editor the reader is in; undefined once they left it. */
  let held: Point | undefined;

  return {
    watch(view) {
      let down: { x: number; y: number } | undefined;
      const pressed = (event: PointerEvent): void => {
        // A press that opens an image larger is the image's, not a place for the caret.
        const target = event.target;
        if (target instanceof Element && target.closest('[data-zoomable]')) return;
        down = { x: event.clientX, y: event.clientY };
      };
      const released = (event: PointerEvent): void => {
        if (!down) return;
        const moved = Math.hypot(event.clientX - down.x, event.clientY - down.y);
        if (moved <= SLOP) {
          const box = view.getBoundingClientRect();
          press = { dx: event.clientX - box.left, dy: event.clientY - box.top, at: Date.now() };
        }
        down = undefined;
      };
      const cancelled = (): void => {
        down = undefined;
      };
      view.addEventListener('pointerdown', pressed);
      view.addEventListener('pointerup', released);
      view.addEventListener('pointercancel', cancelled);
      return () => {
        view.removeEventListener('pointerdown', pressed);
        view.removeEventListener('pointerup', released);
        view.removeEventListener('pointercancel', cancelled);
      };
    },

    take(editor, writable) {
      const before = held;
      const pressed = press;
      held = undefined;
      press = undefined;
      let frame: number | undefined;
      const follow = (): void => {
        if (frame !== undefined || !editor.isFocused) return;
        frame = requestAnimationFrame(() => {
          frame = undefined;
          if (!editor.isDestroyed && editor.isFocused) held = caretOf(editor) ?? held;
        });
      };
      editor.on('focus', follow);
      editor.on('selectionUpdate', follow);
      // Turned read-only (the document is sent again) is no leaving: the caret is kept for the
      // next editor. Leaving it otherwise lets it go.
      editor.on('blur', () => {
        if (editor.isEditable) held = undefined;
      });
      editor.on('destroy', () => {
        if (frame !== undefined) cancelAnimationFrame(frame);
      });
      if (!writable) return;
      if (before) placeAt(editor, before);
      else if (pressed && Date.now() - pressed.at <= PRESS_KEPT) placeAt(editor, pressed);
    },
  };
}
