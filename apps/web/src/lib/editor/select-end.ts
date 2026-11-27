/**
 * Moves a Tiptap editor's selection to the end of its document synchronously
 * using ProseMirror's `Selection.atEnd`, handling trailing atom blocks (images,
 * files, horizontal rules) without pulling `@tiptap/pm/state` into the entry chunk.
 */
import type { Editor } from '@tiptap/core';
import { Selection } from '@tiptap/pm/state';

export function selectEnd(editor: Editor): void {
  const { state, view } = editor;
  view.dispatch(state.tr.setSelection(Selection.atEnd(state.doc)).scrollIntoView());
}
