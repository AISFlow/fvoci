/**
 * Places held across what others write, in a body edited together
 * (shared-place.ts): an image going up lands where it was put however the
 * room changes the body above it or in its line, and is not put in (and
 * that said) when its block is deleted or the reader may no longer write;
 * the place of a formula or a diagram and the box of a formula in a line
 * follow their own node, never a neighbour, and close when it is deleted.
 */
import { getSchema, type Editor } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';
import { prosemirrorJSONToYXmlFragment } from '@tiptap/y-tiptap';
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import { openPlace, placeKey, placeOf } from '$lib/editor/editing-place.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { setFileSource } from '$lib/editor/files.ts';
import type { DocNode } from '$lib/editor/schema.ts';

import { doc, math, mathBlock, paragraph, text } from '../support/body-docs.ts';

vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: () => Promise.resolve({ ok: false, reason: 'failed' }),
}));

setFileSource({ url: (fileId) => `/files/${fileId}` });

const schema = getSchema(bodyExtensions());

/** The server's document and the reader's, with the room between them. */
function room(body: DocNode): {
  server: Y.Doc;
  reader: Y.Doc;
  others: (change: () => void) => void;
} {
  const server = new Y.Doc();
  prosemirrorJSONToYXmlFragment(schema, body, server.getXmlFragment(BODY_FIELD));
  const reader = new Y.Doc();
  Y.applyUpdate(reader, Y.encodeStateAsUpdate(server));
  return {
    server,
    reader,
    /** Somebody else changes the server's document; the reader gets it, as from the room. */
    others(change) {
      Y.applyUpdate(server, Y.encodeStateAsUpdate(reader, Y.encodeStateVector(server)));
      const before = Y.encodeStateVector(server);
      change();
      Y.applyUpdate(reader, Y.encodeStateAsUpdate(server, before), 'room');
    },
  };
}

const paragraphOf = (value: string): Y.XmlElement => {
  const element = new Y.XmlElement('paragraph');
  element.insert(0, [new Y.XmlText(value)]);
  return element;
};

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

/** Pastes an image file the way a browser reports it. */
function pasteImage(editor: Editor): void {
  const event = new Event('paste', { bubbles: true, cancelable: true }) as Event & {
    clipboardData: { files: File[]; getData: () => string };
  };
  event.clipboardData = {
    files: [new File(['png'], 'a.png', { type: 'image/png' })],
    getData: () => '',
  };
  editor.view.someProp('handlePaste', (handle) =>
    handle(editor.view, event as unknown as ClipboardEvent, editor.state.doc.slice(0, 0)),
  );
}

/** The blocks of the editor's document, a word each. */
const blocks = (editor: Editor): string[] => {
  const found: string[] = [];
  editor.state.doc.forEach((node) => {
    found.push(node.type.name === 'paragraph' ? node.textContent : node.type.name);
  });
  return found;
};

/** An upload that ends when the test says so. */
function slowUpload(): { upload: (file: File) => Promise<string>; finish: () => void } {
  let finish = (): void => undefined;
  const upload = (): Promise<string> =>
    new Promise((resolve) => {
      finish = () => {
        resolve('file-1');
      };
    });
  return {
    upload,
    finish: () => {
      finish();
    },
  };
}

afterEach(() => {
  document.querySelectorAll('.doc-math-editing').forEach((node) => {
    node.remove();
  });
});

describe('an image going up in a body edited together', () => {
  it('lands where it was put, whatever the room writes above it and in its block', async () => {
    const shared = room(doc(paragraph(text('first')), paragraph(text('second'))));
    const { upload, finish } = slowUpload();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared.reader,
      uploadFile: upload,
    });
    const editor = editorIn(container);
    // The caret at the end of "second".
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    pasteImage(editor);
    shared.others(() => {
      const fragment = shared.server.getXmlFragment(BODY_FIELD);
      fragment.insert(0, [paragraphOf('above')]);
      // In the block itself, before the place: the editor redraws the whole block.
      ((fragment.get(2) as Y.XmlElement).get(0) as Y.XmlText).insert(0, 'the ');
    });
    expect(blocks(editor)).toEqual(['above', 'first', 'the second']);
    finish();
    await vi.waitFor(() => {
      expect(blocks(editor)).toEqual(['above', 'first', 'the second', 'image']);
    });
  });

  it('goes after its line when the room splits that line, writing a new one above the text', async () => {
    const shared = room(doc(paragraph(text('first')), paragraph(text('second'))));
    const { upload, finish } = slowUpload();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared.reader,
      uploadFile: upload,
    });
    const editor = editorIn(container);
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    pasteImage(editor);
    // Typed at the start of the line, then Enter: the binding keeps the old
    // block for the new first part and writes the rest anew after it.
    shared.others(() => {
      const fragment = shared.server.getXmlFragment(BODY_FIELD);
      const line = (fragment.get(1) as Y.XmlElement).get(0) as Y.XmlText;
      line.delete(0, line.length);
      line.insert(0, 'new line');
      fragment.insert(2, [paragraphOf('second')]);
    });
    expect(blocks(editor)).toEqual(['first', 'new line', 'second']);
    finish();
    await vi.waitFor(() => {
      expect(blocks(editor)).toEqual(['first', 'new line', 'second', 'image']);
    });
  });

  it('in an empty line, follows its line when the room rewrites the blocks around it', async () => {
    const shared = room(doc(paragraph(), paragraph(text('after'))));
    const { upload, finish } = slowUpload();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared.reader,
      uploadFile: upload,
    });
    const editor = editorIn(container);
    editor.commands.setTextSelection(1);
    pasteImage(editor);
    // One change above, in and after the line: the editor replaces all of it.
    shared.others(() => {
      const fragment = shared.server.getXmlFragment(BODY_FIELD);
      fragment.insert(0, [paragraphOf('above'), paragraphOf('more')]);
      (fragment.get(2) as Y.XmlElement).insert(0, [new Y.XmlText('line')]);
      ((fragment.get(3) as Y.XmlElement).get(0) as Y.XmlText).insert(0, 'just ');
    });
    finish();
    await vi.waitFor(() => {
      expect(blocks(editor)).toEqual(['above', 'more', 'line', 'image', 'just after']);
    });
  });

  it('goes after its line when the room splits the line before its last letter', async () => {
    const shared = room(doc(paragraph(text('Hello'))));
    const { upload, finish } = slowUpload();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared.reader,
      uploadFile: upload,
    });
    const editor = editorIn(container);
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    pasteImage(editor);
    // Enter between "Hell" and "o": the "o" is written anew into a line of its own.
    shared.others(() => {
      const fragment = shared.server.getXmlFragment(BODY_FIELD);
      ((fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText).delete(4, 1);
      fragment.insert(1, [paragraphOf('o')]);
    });
    finish();
    await vi.waitFor(() => {
      expect(blocks(editor)).toEqual(['Hell', 'o', 'image']);
    });
  });

  it('is put in while the editor is read-only for a moment, and not for a reader who may only read', async () => {
    for (const canWrite of [true, false]) {
      const shared = room(doc(paragraph(text('first'))));
      const { upload, finish } = slowUpload();
      const { container, rerender } = render(BodyEditor, {
        doc: null,
        collab: shared.reader,
        uploadFile: upload,
        canWrite: true,
      });
      const editor = editorIn(container);
      editor.commands.setTextSelection(editor.state.doc.content.size - 1);
      pasteImage(editor);
      await rerender({
        doc: null,
        collab: shared.reader,
        uploadFile: upload,
        editable: false,
        canWrite,
      });
      finish();
      await vi.waitFor(() => {
        expect(container.querySelector('.doc-upload')).toBeNull();
      });
      expect(blocks(editor)).toEqual(canWrite ? ['first', 'image'] : ['first']);
      if (!canWrite) {
        expect(container.querySelector('.doc-editor-status')?.textContent.trim()).toBe(
          'The image was not put in: the description cannot be changed now.',
        );
      }
      editor.destroy();
    }
  });

  it('is not put in while the editor takes no change (content it cannot draw), and says so', async () => {
    const shared = room(doc(paragraph(text('first'))));
    const { upload, finish } = slowUpload();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared.reader,
      uploadFile: upload,
      canWrite: true,
    });
    const editor = editorIn(container);
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    pasteImage(editor);
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).insert(1, [new Y.XmlElement('futureNode')]);
    });
    finish();
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-upload')).toBeNull();
    });
    // What is said is that the body cannot be edited here, which comes first.
    expect(editor.state.doc.textContent).toBe('first');
    expect(JSON.stringify(shared.reader.getXmlFragment(BODY_FIELD).toJSON())).not.toContain(
      'image',
    );
  });

  it('is not put in when the room deletes its block, and says so', async () => {
    const shared = room(doc(paragraph(text('first')), paragraph(text('second'))));
    const { upload, finish } = slowUpload();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared.reader,
      uploadFile: upload,
    });
    const editor = editorIn(container);
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    pasteImage(editor);
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).delete(1, 1);
    });
    finish();
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-editor-status')?.textContent.trim()).toBe(
        'The image was not put in: the line it was going into was removed or changed.',
      );
    });
    expect(blocks(editor)).toEqual(['first']);
    expect(container.querySelector('.doc-upload')).toBeNull();
  });
});

describe('the place of a block formula in a body edited together', () => {
  it('follows its formula through what the room writes, and closes when the formula is deleted', () => {
    const shared = room(doc(paragraph(text('above')), mathBlock('x^2'), paragraph(text('below'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    const at = editor.state.doc.child(0).nodeSize;
    expect(openPlace(editor.view, at)).toBe(true);
    expect(placeOf(editor.state)).toMatchObject({ kind: 'formula', pos: at });

    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).insert(0, [paragraphOf('new')]);
    });
    const moved = at + editor.state.doc.child(0).nodeSize;
    expect(placeOf(editor.state)).toMatchObject({ kind: 'formula', pos: moved });
    expect(editor.state.doc.nodeAt(moved)?.type.name).toBe('blockMath');

    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).delete(2, 1);
    });
    expect(placeOf(editor.state)).toBeNull();
  });
});

describe('the place of a block in a body edited together, never taken for a neighbour', () => {
  it('closes when its formula is deleted, though the next block is a formula too', () => {
    const shared = room(doc(paragraph(text('above')), mathBlock('x^2'), mathBlock('y')));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    const at = editor.state.doc.child(0).nodeSize;
    expect(openPlace(editor.view, at)).toBe(true);
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).delete(1, 1);
    });
    expect(placeOf(editor.state)).toBeNull();
    expect(editor.state.doc.child(1).attrs.latex).toBe('y');
  });

  it('stays open when one change of the room rewrites the blocks on both sides of it', () => {
    const shared = room(doc(paragraph(text('above')), mathBlock('x^2'), paragraph(text('below'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    const at = editor.state.doc.child(0).nodeSize;
    expect(openPlace(editor.view, at)).toBe(true);
    // The editor replaces everything from the first block changed to the last.
    shared.others(() => {
      const fragment = shared.server.getXmlFragment(BODY_FIELD);
      ((fragment.get(0) as Y.XmlElement).get(0) as Y.XmlText).insert(0, 'just ');
      ((fragment.get(2) as Y.XmlElement).get(0) as Y.XmlText).insert(0, 'just ');
    });
    const moved = editor.state.doc.child(0).nodeSize;
    expect(placeOf(editor.state)).toMatchObject({ kind: 'formula', pos: moved });
  });

  it('follows the first block of the body when the room writes above it', () => {
    const shared = room(doc(mathBlock('x^2'), paragraph(text('below'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    expect(openPlace(editor.view, 0)).toBe(true);
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).insert(0, [paragraphOf('new')]);
    });
    const moved = editor.state.doc.child(0).nodeSize;
    expect(placeOf(editor.state)).toMatchObject({ kind: 'formula', pos: moved });
  });

  it('holds a diagram opened in the same change that made it, once that change is shared', () => {
    const shared = room(doc(paragraph(text('first')), paragraph(text('last'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    const { schema: own } = editor.state;
    const at = editor.state.doc.child(0).nodeSize;
    const fence = own.nodes.codeBlock?.create({ language: 'mermaid' }, own.text('graph TD'));
    if (!fence) throw new Error('no code block');
    // Made and opened at once, as the fence typed in a line does, the caret in its source.
    const tr = editor.state.tr.replaceWith(at, at + editor.state.doc.child(1).nodeSize, fence);
    tr.setSelection(TextSelection.create(tr.doc, at + fence.nodeSize - 1));
    editor.view.dispatch(tr.setMeta(placeKey, { open: { kind: 'diagram', pos: at } }));
    expect(placeOf(editor.state)).toMatchObject({ kind: 'diagram', pos: at });
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).insert(0, [paragraphOf('new')]);
    });
    const moved = at + editor.state.doc.child(0).nodeSize;
    expect(placeOf(editor.state)).toMatchObject({ kind: 'diagram', pos: moved });
  });
});

describe('the place of a new formula in a body edited together', () => {
  /** Types into the editor the way a keyboard does (input rules included). */
  function type(editor: Editor, value: string): void {
    for (const character of value) {
      const { from, to } = editor.state.selection;
      const insert = () => editor.state.tr.insertText(character, from, to);
      const handled = editor.view.someProp('handleTextInput', (handle) =>
        handle(editor.view, from, to, character, insert),
      );
      if (!handled) editor.view.dispatch(insert());
    }
  }

  it('keeps what the reader wrote when somebody else writes into its line: the formula goes after it', () => {
    const shared = room(doc(paragraph(text('first')), paragraph()));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    type(editor, '$$ ');
    const field = document.querySelector<HTMLTextAreaElement>('.doc-place textarea');
    if (!field) throw new Error('no field');
    field.value = 'x^2';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    shared.others(() => {
      const line = shared.server.getXmlFragment(BODY_FIELD).get(1) as Y.XmlElement;
      line.insert(0, [new Y.XmlText('theirs')]);
    });
    expect(placeOf(editor.state)).toMatchObject({ kind: 'new-formula', lineTaken: true });
    field.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }),
    );
    expect(placeOf(editor.state)).toBeNull();
    expect(blocks(editor)).toEqual(['first', 'theirs', 'blockMath', '']);
    expect(editor.state.doc.child(2).attrs.latex).toBe('x^2');
    // In the shared document, for the room to send.
    const sent = shared.reader.getXmlFragment(BODY_FIELD).get(2) as Y.XmlElement;
    expect([sent.nodeName, sent.getAttribute('latex')]).toEqual(['blockMath', 'x^2']);
  });

  it('shows its field after a line somebody else wrote in, where the formula will go', () => {
    const shared = room(doc(paragraph()));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    editor.commands.setTextSelection(1);
    type(editor, '$$ ');
    shared.others(() => {
      const line = shared.server.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement;
      line.insert(0, [new Y.XmlText('theirs')]);
    });
    const line = editor.view.dom.querySelector('p');
    expect(line?.firstChild?.textContent).toBe('theirs');
    // The field comes after the other's text in the line.
    const field = line?.querySelector('.doc-place-new');
    expect(field).not.toBeNull();
    expect(field?.previousSibling?.textContent).toBe('theirs');
  });

  it('says so when somebody else deletes its line with a formula written in it', async () => {
    const shared = room(doc(paragraph(text('first')), paragraph()));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    type(editor, '$$ ');
    const field = document.querySelector<HTMLTextAreaElement>('.doc-place textarea');
    if (!field) throw new Error('no field');
    field.value = 'x^2';
    field.dispatchEvent(new Event('input', { bubbles: true }));
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).delete(1, 1);
    });
    expect(placeOf(editor.state)).toBeNull();
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-editor-status')?.textContent.trim()).toBe(
        'What you wrote in the formula was not kept: its line was removed or changed.',
      );
    });
  });
});

/** The source of the first formula in a line of the editor's document. */
function latexIn(editor: Editor): unknown {
  let latex: unknown;
  editor.state.doc.descendants((node) => {
    if (node.type.name === 'inlineMath') latex ??= node.attrs.latex;
  });
  return latex;
}

describe('the box of a formula in a line in a body edited together', () => {
  const box = (): HTMLTextAreaElement | null =>
    document.querySelector<HTMLTextAreaElement>('.doc-math-editing textarea');

  it('writes into its formula after the room wrote above it', () => {
    const shared = room(doc(paragraph(text('a '), math('x'), text(' b'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = box();
    if (!field) throw new Error('no box');
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).insert(0, [paragraphOf('new')]);
    });
    field.value = 'y';
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    let latex: unknown;
    editor.state.doc.descendants((node) => {
      if (node.type.name === 'inlineMath') latex = node.attrs.latex;
    });
    expect(latex).toBe('y');
    expect(blocks(editor)[0]).toBe('new');
  });

  it('stays on its formula when the room types right before it, and writes over a source changed meanwhile', () => {
    const shared = room(doc(paragraph(text('a '), math('x'), text(' b'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = box();
    if (!field) throw new Error('no box');
    shared.others(() => {
      const line = shared.server.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement;
      (line.get(0) as Y.XmlText).insert(2, 'X');
      (line.get(1) as Y.XmlElement).setAttribute('latex', 'z');
    });
    expect(box()).not.toBeNull();
    field.value = 'y';
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    const found: unknown[] = [];
    editor.state.doc.descendants((node) => {
      if (node.type.name === 'inlineMath') found.push(node.attrs.latex);
    });
    expect(found).toEqual(['y']);
    expect(editor.state.doc.textContent).toContain('a X');
  });

  it('says so when the room deletes the formula with something written in its box', async () => {
    const shared = room(doc(paragraph(text('a '), math('x'), text(' b'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = box();
    if (!field) throw new Error('no box');
    field.value = 'y^2';
    shared.others(() => {
      const line = shared.server.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement;
      line.delete(1, 1);
    });
    expect(box()).toBeNull();
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-editor-status')?.textContent.trim()).toBe(
        'What you wrote in the formula was not kept: its line was removed or changed.',
      );
    });
  });

  it('is locked while the editor is read-only, saying why, and writes again once it is not', async () => {
    const shared = room(doc(paragraph(text('a '), math('x'), text(' b'))));
    const props = { doc: null, collab: shared.reader, canWrite: true };
    const { container, rerender } = render(BodyEditor, props);
    const editor = editorIn(container);
    editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = box();
    if (!field) throw new Error('no box');
    field.value = 'y';
    await rerender({ ...props, editable: false });
    expect(field.readOnly).toBe(true);
    expect(document.querySelector('.doc-math-editing')?.textContent).toContain(
      'cannot be changed now',
    );
    // Enter does nothing while locked: nothing goes into the shared document.
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(box()).not.toBeNull();
    expect(latexIn(editor)).toBe('x');
    await rerender({ ...props, editable: true });
    expect(field.readOnly).toBe(false);
    field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(latexIn(editor)).toBe('y');
  });

  it('locked, closes on a press elsewhere, keeping nothing, and says so', async () => {
    const shared = room(doc(paragraph(text('a '), math('x'), text(' b'))));
    const props = { doc: null, collab: shared.reader, canWrite: true };
    const { container, rerender } = render(BodyEditor, props);
    const editor = editorIn(container);
    editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = box();
    if (!field) throw new Error('no box');
    field.value = 'y';
    await rerender({ ...props, editable: false });
    // A touch that only scrolls leaves it open; a press closes it.
    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    expect(box()).not.toBeNull();
    document.body.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(box()).toBeNull();
    expect(latexIn(editor)).toBe('x');
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-editor-status')?.textContent.trim()).toBe(
        'What you wrote in the formula was not kept: the description cannot be changed now.',
      );
    });
  });

  it("says a formula's loss during an upload, and the image going in does not take it away", async () => {
    const shared = room(doc(paragraph(text('a '), math('x'), text(' b'))));
    const { upload, finish } = slowUpload();
    const { container } = render(BodyEditor, {
      doc: null,
      collab: shared.reader,
      uploadFile: upload,
    });
    const editor = editorIn(container);
    editor.commands.setTextSelection(1);
    pasteImage(editor);
    editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    const field = box();
    if (!field) throw new Error('no box');
    field.value = 'y^2';
    shared.others(() => {
      (shared.server.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement).delete(1, 1);
    });
    const said = 'What you wrote in the formula was not kept: its line was removed or changed.';
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-editor-status')?.textContent.trim()).toBe(said);
    });
    finish();
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-upload')).toBeNull();
    });
    expect(container.querySelector('.doc-editor-status')?.textContent.trim()).toBe(said);
  });

  it('closes, keeping nothing, when the room deletes the formula', () => {
    const shared = room(doc(paragraph(text('a '), math('x'), text(' b'))));
    const { container } = render(BodyEditor, { doc: null, collab: shared.reader });
    const editor = editorIn(container);
    editor.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    expect(box()).not.toBeNull();
    shared.others(() => {
      shared.server.getXmlFragment(BODY_FIELD).delete(0, 1);
    });
    expect(box()).toBeNull();
  });
});
