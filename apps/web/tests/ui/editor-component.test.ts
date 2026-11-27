/**
 * The product's editor component (BodyEditor.svelte) against the shape its
 * callers were promised: when it reads the document, what it reports, what
 * it refuses to touch, and that it brings no style sheet of its own.
 */
import type { Editor } from '@tiptap/core';
import { Selection } from '@tiptap/pm/state';
import { render } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import BodyEditor from '$lib/editor/BodyEditor.svelte';
import { setFileSource } from '$lib/editor/files.ts';
import { countTasks, plainText } from '$lib/editor/body-text.ts';
import { validateDoc, type DocNode } from '$lib/editor/schema.ts';

import {
  doc,
  HAND_MADE,
  image,
  math,
  mention,
  paragraph,
  text,
  WITH_DIAGRAMS,
} from '../support/body-docs.ts';

vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: () => Promise.resolve({ ok: false, reason: 'failed' }),
}));

/** The editor inside a rendered component: the library hangs it on its root element. */
function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

function typeAtEnd(editor: Editor, value: string): void {
  const end = Selection.atEnd(editor.state.doc);
  editor.view.dispatch(editor.state.tr.setSelection(end).insertText(value, end.from));
}

/** Pastes or drops files the way a browser reports it. */
function give(editor: Editor, files: File[]): boolean {
  const event = new Event('paste', { bubbles: true, cancelable: true }) as Event & {
    clipboardData: { files: File[]; getData: () => string };
  };
  event.clipboardData = { files, getData: () => '' };
  const handled = editor.view.someProp('handlePaste', (handle) =>
    handle(editor.view, event as unknown as ClipboardEvent, editor.state.doc.slice(0, 0)),
  );
  return Boolean(handled);
}

afterEach(() => {
  document.querySelectorAll('.doc-suggestions, .doc-math-editing').forEach((node) => {
    node.remove();
  });
});

describe('BodyEditor', () => {
  it('shows the document it is given in a text field with a name, and reports nothing yet', () => {
    const onChange = vi.fn();
    const onLoad = vi.fn();
    const body = doc(paragraph(text('안녕하세요')));
    const { container } = render(BodyEditor, {
      doc: body,
      onChange,
      onLoad,
      class: 'p-4 min-h-64',
    });
    const root = container.querySelector('.ProseMirror');
    expect(root?.textContent).toBe('안녕하세요');
    expect(root?.classList.contains('doc-body')).toBe(true);
    expect(root?.classList.contains('p-4')).toBe(true);
    expect(root?.getAttribute('contenteditable')).toBe('true');
    expect(root?.getAttribute('role')).toBe('textbox');
    expect(root?.getAttribute('aria-label')).toBe('Body');
    expect(onChange).not.toHaveBeenCalled();
    // What the editor would store for the document is the document.
    expect(onLoad).toHaveBeenCalledTimes(1);
    expect(onLoad).toHaveBeenLastCalledWith(body);
  });

  it('takes `null` as an empty body, and reports an empty body as `null`', () => {
    const onChange = vi.fn();
    const onLoad = vi.fn();
    const { container } = render(BodyEditor, { doc: null, onChange, onLoad, placeholder: '내용' });
    expect(onLoad).toHaveBeenLastCalledWith(null);
    expect(container.querySelector('[data-placeholder="내용"]')).not.toBeNull();
    const editor = editorIn(container);
    typeAtEnd(editor, 'a');
    expect(onChange).toHaveBeenLastCalledWith(doc(paragraph(text('a'))));
    editor.commands.clearContent(true);
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it('reports every change in the stored form, which the validator accepts', () => {
    const onChange = vi.fn<(body: DocNode | null) => void>();
    const { container } = render(BodyEditor, { doc: null, onChange });
    const editor = editorIn(container);
    editor.commands.setContent(
      '<h2>Title</h2><p><strong>a<br>b</strong></p><pre><code>x</code></pre><ol><li>one</li></ol>',
      { emitUpdate: true },
    );
    const reported = onChange.mock.lastCall?.[0];
    if (!reported) throw new Error('nothing reported');
    expect(validateDoc(reported)).toEqual([]);
    // The stored form: no nulls, no marks on a line break, no empty attribute lists.
    expect(JSON.stringify(reported)).not.toContain('null');
    expect(reported.content?.[2]).toEqual({ type: 'codeBlock', content: [text('x')] });
  });

  it('does not read `doc` again unless `revision` goes up', async () => {
    const onChange = vi.fn();
    const onLoad = vi.fn();
    const first = doc(paragraph(text('first')));
    const second = doc(paragraph(text('second')));
    const { container, rerender } = render(BodyEditor, {
      doc: first,
      revision: 1,
      onChange,
      onLoad,
    });
    const editor = editorIn(container);
    typeAtEnd(editor, ' and typed');
    expect(onChange).toHaveBeenCalledTimes(1);

    // A new document alone: what is being written stays.
    await rerender({ doc: second, revision: 1, onChange, onLoad });
    expect(container.querySelector('.ProseMirror')?.textContent).toBe('first and typed');
    expect(onLoad).toHaveBeenCalledTimes(1);

    // With the revision raised it is read, and that is no change to report.
    await rerender({ doc: second, revision: 2, onChange, onLoad });
    expect(container.querySelector('.ProseMirror')?.textContent).toBe('second');
    expect(editorIn(container)).toBe(editor);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onLoad).toHaveBeenCalledTimes(2);
    expect(onLoad).toHaveBeenLastCalledWith(second);

    // The same revision again reads nothing.
    await rerender({ doc: first, revision: 2, onChange, onLoad });
    expect(container.querySelector('.ProseMirror')?.textContent).toBe('second');
    await rerender({ doc: null, revision: 3, onChange, onLoad });
    expect(container.querySelector('.ProseMirror')?.textContent).toBe('');
    expect(onLoad).toHaveBeenLastCalledWith(null);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('undoes local typing after a revision without undoing the received document', async () => {
    const { container, rerender } = render(BodyEditor, {
      doc: doc(paragraph(text('before'))),
      revision: 1,
    });
    await rerender({ doc: doc(paragraph(text('received'))), revision: 2 });
    const editor = editorIn(container);
    typeAtEnd(editor, ' typed');
    expect(editor.getText()).toBe('received typed');
    editor.commands.undo();
    expect(editor.getText()).toBe('received');
    expect(editor.commands.undo()).toBe(false);
    editor.commands.redo();
    expect(editor.getText()).toBe('received typed');
  });

  it('loads every document the comparison with the view is run on', () => {
    for (const [name, body] of Object.entries({ ...HAND_MADE, ...WITH_DIAGRAMS })) {
      const onLoad = vi.fn<(stored: DocNode | null | undefined) => void>();
      const { unmount } = render(BodyEditor, { doc: body, onLoad });
      const loaded = onLoad.mock.lastCall?.[0];
      if (!loaded) throw new Error(`${name}: not loaded`);
      expect([name, validateDoc(loaded)]).toEqual([name, []]);
      // The same words and the same tasks. (The editor settles a few things
      // its own way that mean nothing in the stored form: it joins texts with
      // equal marks, orders marks, and writes out where a numbered list starts.)
      expect([name, plainText(loaded)]).toEqual([name, plainText(body)]);
      expect([name, countTasks(loaded)]).toEqual([name, countTasks(body)]);
      unmount();
    }
  });

  it('loads formulas and diagrams exactly as they are stored', () => {
    for (const body of [HAND_MADE.formulas, ...Object.values(WITH_DIAGRAMS)]) {
      const onLoad = vi.fn();
      const { unmount } = render(BodyEditor, { doc: body ?? null, onLoad });
      expect(onLoad).toHaveBeenLastCalledWith(body);
      unmount();
    }
  });

  it('can be made read-only and editable again', async () => {
    const body = doc(paragraph(text('x')));
    const { container, rerender } = render(BodyEditor, { doc: body, editable: false });
    const root = container.querySelector('.ProseMirror');
    expect(root?.getAttribute('contenteditable')).toBe('false');
    await rerender({ doc: body, editable: true });
    expect(root?.getAttribute('contenteditable')).toBe('true');
  });

  it('does not load a body the validator refuses: empty, read-only, and saying so', async () => {
    const onChange = vi.fn();
    const onLoad = vi.fn();
    // A node from a later version of the list, between two paragraphs worth keeping.
    const fromTheFuture = doc(
      paragraph(text('keep me')),
      { type: 'table' },
      paragraph(text('me too')),
    );
    const { container, rerender } = render(BodyEditor, {
      doc: fromTheFuture,
      onChange,
      onLoad,
      revision: 1,
    });
    const root = container.querySelector('.ProseMirror');
    expect(root?.textContent).toBe('');
    expect(root?.getAttribute('contenteditable')).toBe('false');
    expect(onLoad).toHaveBeenLastCalledWith(undefined);
    const status = container.querySelector('[role="status"]');
    expect(status?.textContent).toContain('cannot be edited here');
    expect(status?.hasAttribute('data-shown')).toBe(true);
    expect(container.querySelector('.doc-editor')?.hasAttribute('data-unreadable')).toBe(true);
    expect(onChange).not.toHaveBeenCalled();

    // Asking for it to be editable does not make it so.
    await rerender({ doc: fromTheFuture, onChange, onLoad, revision: 1, editable: true });
    expect(root?.getAttribute('contenteditable')).toBe('false');

    // A document that can be read takes its place when one comes.
    await rerender({
      doc: doc(paragraph(text('fine'))),
      onChange,
      onLoad,
      revision: 2,
      editable: true,
    });
    expect(root?.textContent).toBe('fine');
    expect(root?.getAttribute('contenteditable')).toBe('true');
    expect(status?.textContent).toBe('');
    expect(status?.hasAttribute('data-shown')).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('names mentioned people through the caller, and again when the names become known', async () => {
    const [U1, U2, U3] = [
      '01930000-0000-7000-8000-00000000c001',
      '01930000-0000-7000-8000-00000000c002',
      '01930000-0000-7000-8000-00000000c003',
    ];
    const body = doc(
      paragraph(mention(U1), text(' and '), mention(U2), text(' and '), mention(U3)),
    );
    const users = [{ id: U2, name: '이도윤' }];
    const { container, rerender } = render(BodyEditor, { doc: body, users });
    const names = () =>
      Array.from(container.querySelectorAll('[data-mention]')).map((node) => node.textContent);
    expect(names()).toEqual([`@${U1}`, '@이도윤', `@${U3}`]);
    await rerender({
      doc: body,
      users,
      resolveUser: (userId: string) => (userId === U1 ? '김하늘' : undefined),
    });
    expect(names()).toEqual(['@김하늘', '@이도윤', `@${U3}`]);
    // The document still holds identifiers only.
    expect(JSON.stringify(editorIn(container).getJSON())).not.toContain('김하늘');
  });

  it('finds a person to mention by any of their names, not the shown one alone', async () => {
    const users = [
      { id: 'u-kim', name: '김영환', also: ['yhkim', 'Younghwan Kim', 'Kim Younghwan'] },
      { id: 'u-lee', name: 'Lee', also: ['lee'] },
    ];
    const { container } = render(BodyEditor, { doc: doc(paragraph()), users });
    const editor = editorIn(container);
    const offered = (): (string | null)[] =>
      Array.from(document.querySelectorAll('[data-suggestions="mention"] [role="option"]')).map(
        (option) => option.textContent,
      );
    editor.commands.focus();
    editor.commands.insertContent('@kim');
    await vi.waitFor(() => {
      expect(offered()).toEqual(['김영환']);
    });
    editor.commands.insertContent(' x @yhk');
    await vi.waitFor(() => {
      expect(offered()).toEqual(['김영환']);
    });
  });

  it('tells the caller about focus coming and going', () => {
    const onFocus = vi.fn();
    const onBlur = vi.fn();
    const { container } = render(BodyEditor, { doc: null, onFocus, onBlur });
    const root = container.querySelector<HTMLElement>('.ProseMirror');
    root?.dispatchEvent(new FocusEvent('focus'));
    expect(onFocus).toHaveBeenCalledTimes(1);
    root?.dispatchEvent(new FocusEvent('blur'));
    expect(onBlur).toHaveBeenCalledTimes(1);
  });

  it('puts no style sheet of the editing library into the page', () => {
    document.head.querySelectorAll('style[data-tiptap-style]').forEach((node) => {
      node.remove();
    });
    render(BodyEditor, { doc: doc(paragraph(text('x'), math('y'))) });
    expect(document.querySelectorAll('style[data-tiptap-style]')).toHaveLength(0);
  });

  it('cleans up after itself', () => {
    const { container, unmount } = render(BodyEditor, { doc: doc(paragraph(math('x'))) });
    const editor = editorIn(container);
    container.querySelector<HTMLElement>('.doc-math')?.click();
    expect(document.querySelector('.doc-math-editing')).not.toBeNull();
    unmount();
    expect(editor.isDestroyed).toBe(true);
    expect(document.querySelector('.doc-math-editing')).toBeNull();
  });
});

describe('images given to the editor as files', () => {
  const picture = (name = 'chart.png'): File => new File(['x'], name, { type: 'image/png' });

  it('takes none when the caller cannot upload', () => {
    const onChange = vi.fn();
    const { container } = render(BodyEditor, { doc: null, onChange });
    expect(give(editorIn(container), [picture()])).toBe(false);
    expect(container.querySelector('.doc-upload')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('takes a picture the server would not have drawn (an SVG) as a file to download', () => {
    const uploadFile = vi.fn((_file: File) => new Promise<string>(() => undefined));
    const { container } = render(BodyEditor, { doc: null, uploadFile });
    const svg = new File(['<svg/>'], 'a.svg', { type: 'image/svg+xml' });
    expect(give(editorIn(container), [svg])).toBe(true);
    expect(uploadFile).toHaveBeenCalledWith(svg);
    expect(container.querySelector('.doc-upload')?.getAttribute('aria-label')).toBe(
      'Uploading the file…',
    );
  });

  it('puts no picture into an editor that was locked while it went up', async () => {
    let finish: (fileId: string) => void = () => undefined;
    const uploadFile = vi.fn(
      (_file: File) =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    const onChange = vi.fn<(body: DocNode | null) => void>();
    const { container, rerender } = render(BodyEditor, {
      doc: doc(paragraph(text('before'))),
      onChange,
      uploadFile,
    });
    expect(give(editorIn(container), [picture()])).toBe(true);
    await rerender({ doc: doc(paragraph(text('before'))), onChange, uploadFile, editable: false });
    finish('file-1');
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-upload')).toBeNull();
    });
    expect(JSON.stringify(editorIn(container).getJSON())).not.toContain('file-1');
  });

  it('uploads a pasted picture, holds its place meanwhile, and stores only the identifier', async () => {
    const previous = setFileSource({ url: (fileId) => `/files/board-1/${fileId}` });
    let finish: (fileId: string) => void = () => undefined;
    const uploadFile = vi.fn(
      (_file: File) =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    const onChange = vi.fn<(body: DocNode | null) => void>();
    const { container } = render(BodyEditor, {
      doc: doc(paragraph(text('before'))),
      onChange,
      uploadFile,
    });
    const editor = editorIn(container);
    const file = picture();
    expect(give(editor, [file])).toBe(true);
    expect(uploadFile).toHaveBeenCalledTimes(1);
    expect(uploadFile).toHaveBeenCalledWith(file);

    // A placeholder in the page; nothing about it in the document.
    const placeholder = container.querySelector('.doc-upload');
    expect(placeholder?.getAttribute('aria-label')).toBe('Uploading the image…');
    // Going up is told where every upload is (the stack), not again under the body.
    await Promise.resolve();
    expect(container.querySelector('[role="status"]')?.textContent).toBe('');
    expect(onChange).not.toHaveBeenCalled();
    expect(JSON.stringify(editor.getJSON())).not.toContain('image');

    finish('file-77');
    await vi.waitFor(() => {
      expect(onChange).toHaveBeenCalled();
    });
    expect(container.querySelector('.doc-upload')).toBeNull();
    const reported = onChange.mock.lastCall?.[0];
    expect(reported?.content).toContainEqual(image('file-77'));
    expect(validateDoc(reported)).toEqual([]);
    expect(container.querySelector('img[data-file-id="file-77"]')?.getAttribute('src')).toBe(
      '/files/board-1/file-77',
    );
    await vi.waitFor(() => {
      expect(container.querySelector('[role="status"]')?.textContent).toBe('');
    });
    setFileSource(previous);
  });

  it('gives up the place of an upload that failed, and leaves the document as it was', async () => {
    const onChange = vi.fn();
    const uploadFile = vi.fn((_file: File) => Promise.reject(new Error('too large')));
    const { container } = render(BodyEditor, { doc: null, onChange, uploadFile });
    expect(give(editorIn(container), [picture()])).toBe(true);
    expect(container.querySelector('.doc-upload')).not.toBeNull();
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-upload')).toBeNull();
    });
    // Why is told where every upload is (the stack), and not twice.
    expect(container.querySelector('[role="status"]')?.textContent).toBe('');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('takes no files while read-only', () => {
    const uploadFile = vi.fn((_file: File) => Promise.resolve('file-1'));
    const { container } = render(BodyEditor, { doc: null, editable: false, uploadFile });
    expect(give(editorIn(container), [picture()])).toBe(false);
    expect(uploadFile).not.toHaveBeenCalled();
  });
});
