import { Editor } from '@tiptap/core';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import { CheckedCollaboration, hasInvalidContent } from '$lib/editor/content-check.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';

const editors: Editor[] = [];

afterEach(() => {
  for (const editor of editors.splice(0)) editor.destroy();
});

function fromRoom(shared: Y.Doc, mutate: (fragment: Y.XmlFragment) => void): void {
  const other = new Y.Doc();
  Y.applyUpdate(other, Y.encodeStateAsUpdate(shared));
  const before = Y.encodeStateVector(other);
  mutate(other.getXmlFragment(BODY_FIELD));
  Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, before), 'room');
  other.destroy();
}

describe('CheckedCollaboration remote attribute validation (H-7)', () => {
  it.each([
    [
      'path-traversal image fileId',
      (fragment: Y.XmlFragment) => {
        const img = new Y.XmlElement('image');
        img.setAttribute('fileId', '../evil');
        fragment.insert(fragment.length, [img]);
      },
    ],
    [
      'oversized tableCell colspan',
      (fragment: Y.XmlFragment) => {
        const cell = new Y.XmlElement('tableCell');
        cell.setAttribute('colspan', 999999 as unknown as string);
        const paragraph = new Y.XmlElement('paragraph');
        paragraph.insert(0, [new Y.XmlText('x')]);
        cell.insert(0, [paragraph]);
        const row = new Y.XmlElement('tableRow');
        row.insert(0, [cell]);
        const table = new Y.XmlElement('table');
        table.insert(0, [row]);
        fragment.insert(fragment.length, [table]);
      },
    ],
    [
      'invalid codeBlock language',
      (fragment: Y.XmlFragment) => {
        const block = new Y.XmlElement('codeBlock');
        block.setAttribute('language', 'bad lang!');
        block.insert(0, [new Y.XmlText('x')]);
        fragment.insert(fragment.length, [block]);
      },
    ],
  ])('pauses editing on hostile remote %s without deleting it from Y.Doc', (_label, inject) => {
    const shared = new Y.Doc();
    const initial = new Y.XmlElement('paragraph');
    initial.insert(0, [new Y.XmlText('hello')]);
    shared.getXmlFragment(BODY_FIELD).insert(0, [initial]);

    const onContentError = vi.fn();
    const editor = new Editor({
      element: document.createElement('div'),
      extensions: [
        CheckedCollaboration.configure({ document: shared, field: BODY_FIELD }),
        ...bodyExtensions({ history: false }),
      ],
      onContentError,
    });
    editors.push(editor);
    expect(hasInvalidContent(editor.state)).toBe(false);

    fromRoom(shared, inject);

    expect(hasInvalidContent(editor.state)).toBe(true);
    expect(onContentError).toHaveBeenCalledTimes(1);
    editor.view.dispatch(editor.state.tr.insertText('!', 1));
    expect(editor.state.doc.textContent).toBe('hello');
    expect(shared.getXmlFragment(BODY_FIELD).length).toBe(2);
    shared.destroy();
  });
});
