/**
 * A key is read from where the caret is, also when the page has not yet told
 * the editor that it moved: a browser under load hands the editor the next
 * key before the `selectionchange` of the one before (End, then the arrow
 * right at once). Here the caret is moved where only the page sees it, and
 * the key comes before any event.
 */
import type { Editor } from '@tiptap/core';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import BodyEditor from '$lib/editor/BodyEditor.svelte';
import { setFileSource, type FileSource } from '$lib/editor/files.ts';
import type { DocNode } from '$lib/editor/schema.ts';

import { doc, paragraph, text } from '../support/body-docs.ts';

// A selection set by a key is scrolled into sight, which asks for the size of text: jsdom has none.
Range.prototype.getClientRects = function getClientRects() {
  return [] as unknown as DOMRectList;
};
Range.prototype.getBoundingClientRect = function getBoundingClientRect() {
  return new DOMRect();
};

const fileBlock = (fileId: string): DocNode => ({ type: 'file', attrs: { fileId } });

let previous: FileSource;
beforeEach(() => {
  previous = setFileSource({ url: () => undefined });
});
afterEach(() => {
  setFileSource(previous);
});

/** An editor of "Above:" over a file block, its caret at the start of "Above:". */
function editorAtStart(): Editor {
  const { container } = render(BodyEditor, {
    doc: doc(paragraph(text('Above:')), fileBlock('7plans.zip'), paragraph(text('Below'))),
  });
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  const { editor } = root;
  editor.view.dom.focus();
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
  return editor;
}

/** The page's caret at the end of "Above:", as End leaves it, and no event yet. */
function caretToEnd(editor: Editor): void {
  const line = editor.view.dom.querySelector('p')?.firstChild;
  if (!line) throw new Error('no text in the first line');
  document.getSelection()?.collapse(line, 'Above:'.length);
}

/** A key as a browser sends it: the editor reads the arrows by their code. */
function press(editor: Editor, key: string, keyCode: number): void {
  const init = { key, keyCode, bubbles: true, cancelable: true };
  editor.view.dom.dispatchEvent(new KeyboardEvent('keydown', init));
}

describe('a key pressed right after the caret moved', () => {
  it('is an arrow that selects the block after the line, from the end of the line', () => {
    const editor = editorAtStart();
    caretToEnd(editor);
    press(editor, 'ArrowRight', 39);
    const { selection } = editor.state;
    expect(selection).toBeInstanceOf(NodeSelection);
    if (selection instanceof NodeSelection) {
      expect(selection.node.type.name).toBe('file');
    }
  });

  it('is Enter that breaks the line where the caret is, not where it was', () => {
    const editor = editorAtStart();
    caretToEnd(editor);
    press(editor, 'Enter', 13);
    expect(editor.getJSON().content.slice(0, 2)).toEqual([
      paragraph(text('Above:')),
      { type: 'paragraph' },
    ]);
  });
});
