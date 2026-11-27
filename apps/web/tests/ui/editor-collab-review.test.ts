/** Independent regressions for edits held outside the shared document. */
import { getSchema, type Editor } from '@tiptap/core';
import { prosemirrorJSONToYXmlFragment } from '@tiptap/y-tiptap';
import { render } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import type { DocNode } from '$lib/editor/schema.ts';

import { doc, math, paragraph, text } from '../support/body-docs.ts';

const schema = getSchema(bodyExtensions());

function sharedBody(body: DocNode): Y.Doc {
  const shared = new Y.Doc();
  prosemirrorJSONToYXmlFragment(schema, body, shared.getXmlFragment(BODY_FIELD));
  return shared;
}

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor');
  return root.editor;
}

function openFormula(editor: Editor): HTMLTextAreaElement {
  editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
  const field = document.querySelector<HTMLTextAreaElement>('.doc-math-editing textarea');
  if (!field) throw new Error('no formula field');
  return field;
}

function pasteImage(editor: Editor): void {
  const event = new Event('paste', { bubbles: true, cancelable: true });
  Object.assign(event, {
    clipboardData: {
      files: [new File(['png'], 'picture.png', { type: 'image/png' })],
      getData: () => '',
    },
  });
  editor.view.someProp('handlePaste', (handle) =>
    handle(editor.view, event as ClipboardEvent, editor.state.doc.slice(0, 0)),
  );
}

describe('independent collaboration regressions', () => {
  it('closing an untouched inline formula keeps the source written by another reader', () => {
    const shared = sharedBody(doc(paragraph(text('before '), math('x'), text(' after'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared });
    const editor = editorIn(container);
    const field = openFormula(editor);
    const other = new Y.Doc();
    Y.applyUpdate(other, Y.encodeStateAsUpdate(shared));
    const vector = Y.encodeStateVector(other);
    const line = other.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement;
    (line.get(1) as Y.XmlElement).setAttribute('latex', 'their-new-source');
    Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, vector), 'room');

    // The reader never typed in the field; Done merely dismisses it.
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    const formula = (shared.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement).get(
      1,
    ) as Y.XmlElement;
    expect(formula.getAttribute('latex')).toBe('their-new-source');
    other.destroy();
  });

  it('an inline formula field cannot commit new typing after the connection is lost', async () => {
    const shared = sharedBody(doc(paragraph(math('x'))));
    const { container, rerender } = render(BodyEditor, {
      doc: null,
      collab: shared,
      canWrite: true,
    });
    const field = openFormula(editorIn(container));
    field.value = 'draft-before-disconnect';
    field.dispatchEvent(new Event('input', { bubbles: true }));

    // These are the props CardBody uses for an offline room whose mode was write.
    await rerender({ doc: null, collab: shared, editable: false, canWrite: true });
    expect(editorIn(container).isEditable).toBe(false);
    if (field.isConnected && !field.disabled && !field.readOnly) {
      field.value = 'typed-after-disconnect';
      field.dispatchEvent(new Event('input', { bubbles: true }));
      field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    }
    expect(shared.getXmlFragment(BODY_FIELD).toJSON()).not.toContain('typed-after-disconnect');
  });

  it('an image upload is discarded when its own writer deletes the target paragraph', async () => {
    const shared = sharedBody(doc(paragraph(text('target')), paragraph(text('neighbor'))));
    let finish: (fileId: string) => void = () => undefined;
    const uploadFile = (): Promise<string> =>
      new Promise((resolve) => {
        finish = resolve;
      });
    const { container } = render(BodyEditor, { doc: null, collab: shared, uploadFile });
    const editor = editorIn(container);
    const targetSize = editor.state.doc.child(0).nodeSize;
    editor.commands.setTextSelection(targetSize - 1);
    pasteImage(editor);
    expect(container.querySelector('.doc-upload')).not.toBeNull();

    editor.view.dispatch(editor.state.tr.delete(0, targetSize));
    finish('uploaded-picture');
    // Its placeholder goes with its spot; the notice comes once the upload has ended.
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-upload')).toBeNull();
      expect(container.querySelector('.doc-editor-status')?.textContent).toContain(
        'removed or changed',
      );
    });

    const types: string[] = [];
    editor.state.doc.forEach((node) => types.push(node.type.name));
    expect(types).toEqual(['paragraph']);
  });
});
