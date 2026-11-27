/**
 * The editor of a body bound to a shared document (BodyEditor.svelte with
 * `collab`): it shows what the document holds and reads nothing from `doc`,
 * it has no history of its own, and its undo takes back the reader's own
 * changes only, never what came from the room.
 */
import type { Editor } from '@tiptap/core';
import { Selection } from '@tiptap/pm/state';
import { render } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';

import { doc, paragraph, text } from '../support/body-docs.ts';

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

function typeAtEnd(editor: Editor, value: string): void {
  const end = Selection.atEnd(editor.state.doc);
  editor.view.dispatch(editor.state.tr.setSelection(end).insertText(value, end.from));
}

/** A paragraph of `value` written into the shared document by somebody else. */
function fromTheRoom(shared: Y.Doc, value: string): void {
  const other = new Y.Doc();
  Y.applyUpdate(other, Y.encodeStateAsUpdate(shared));
  const before = Y.encodeStateVector(other);
  const element = new Y.XmlElement('paragraph');
  element.insert(0, [new Y.XmlText(value)]);
  const fragment = other.getXmlFragment(BODY_FIELD);
  fragment.insert(fragment.length, [element]);
  Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, before), 'room');
}

const lines = (editor: Editor): string[] => {
  const found: string[] = [];
  editor.state.doc.forEach((node) => found.push(node.textContent));
  return found;
};

describe('BodyEditor on a shared document', () => {
  it('shows the shared document and not `doc`', () => {
    const shared = new Y.Doc();
    fromTheRoom(shared, 'shared');
    const { container } = render(BodyEditor, {
      doc: doc(paragraph(text('not this'))),
      collab: shared,
    });
    expect(lines(editorIn(container))).toEqual(['shared']);
  });

  it("has no history of its own: undo takes back the reader's own typing, not what came from the room", async () => {
    const shared = new Y.Doc();
    fromTheRoom(shared, 'first');
    const { container } = render(BodyEditor, { doc: null, collab: shared });
    const editor = editorIn(container);
    expect(editor.extensionManager.extensions.some((each) => each.name === 'undoRedo')).toBe(false);

    typeAtEnd(editor, ' mine');
    // The shared undo gathers changes close together into one step.
    await new Promise((resolve) => setTimeout(resolve, 600));
    fromTheRoom(shared, 'theirs');
    expect(lines(editor)).toEqual(['first mine', 'theirs']);

    editor.commands.undo();
    expect(lines(editor)).toEqual(['first', 'theirs']);
    editor.commands.redo();
    expect(lines(editor)).toEqual(['first mine', 'theirs']);
  });

  it('writes what the reader types into the shared document', () => {
    const shared = new Y.Doc();
    fromTheRoom(shared, 'one');
    const { container } = render(BodyEditor, { doc: null, collab: shared });
    typeAtEnd(editorIn(container), ' two');
    expect(shared.getXmlFragment(BODY_FIELD).toJSON()).toBe('<paragraph>one two</paragraph>');
  });
});

describe('BodyEditor on a shared document it cannot draw', () => {
  it('keeps what it cannot draw, says so and stops editing, and asks to be made anew once it fits', async () => {
    const shared = new Y.Doc();
    fromTheRoom(shared, 'first');
    const onFitsAgain = vi.fn();
    const { container } = render(BodyEditor, { doc: null, collab: shared, onFitsAgain });
    const editor = editorIn(container);
    // Somebody with a newer version writes a node this one does not know.
    const other = new Y.Doc();
    Y.applyUpdate(other, Y.encodeStateAsUpdate(shared));
    const before = Y.encodeStateVector(other);
    other.getXmlFragment(BODY_FIELD).insert(1, [new Y.XmlElement('futureNode')]);
    Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, before), 'room');
    await vi.waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    expect(container.querySelector('.doc-editor-status')?.textContent.trim()).not.toBe('');
    // Nothing of it was deleted on this side.
    expect(shared.getXmlFragment(BODY_FIELD).length).toBe(2);

    const back = Y.encodeStateVector(other);
    other.getXmlFragment(BODY_FIELD).delete(1, 1);
    Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, back), 'room');
    await vi.waitFor(() => {
      expect(onFitsAgain).toHaveBeenCalled();
    });
    // Never made editable on what it last showed: it took nothing from the room meanwhile.
    expect(editor.isEditable).toBe(false);
  });
});
