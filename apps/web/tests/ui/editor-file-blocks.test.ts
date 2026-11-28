/**
 * The file block in the editor and where a body is only shown: a file that is
 * no picture becomes one when it is pasted, dropped or chosen through the
 * slash menu; it shows its file's name and size from the file source and
 * saves the file under that name, by its link or by Enter while selected; a
 * file that is not there says so and saves nothing.
 */
import type { Editor } from '@tiptap/core';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import BodyEditor from '$lib/editor/BodyEditor.svelte';
import BodyView from '$lib/editor/BodyView.svelte';
import { kindOf } from '$lib/editor/file-view.ts';
import {
  fileDetails,
  setFileSource,
  type FileDetails,
  type FileSource,
} from '$lib/editor/files.ts';
import { validateDoc, type DocNode } from '$lib/editor/schema.ts';
import { matching } from '$lib/editor/slash-menu.ts';
import { CARD_WORDS, FILE_DROPPED, lostNote, PAGE_WORDS } from '$lib/features/card/body-notes.ts';

import { code, doc, image, paragraph, text } from '../support/body-docs.ts';

const fileBlock = (fileId: string): DocNode => ({ type: 'file', attrs: { fileId } });

const KNOWN: Record<string, FileDetails> = {
  '7minutes.pdf': { name: '회의록 2026-11.pdf', size: 2048 },
  '7sheet.xlsx': { name: 'budget.xlsx', size: 10 },
};

/** A source that knows two files, reads every other as missing, and hands out bytes. */
function source(bytes = vi.fn((_fileId: string) => Promise.resolve(new Blob(['x'])))): FileSource {
  return {
    url: (fileId) => `/files/wiki-1/${fileId}`,
    details: (fileId) => Promise.resolve(KNOWN[fileId] ?? null),
    bytes,
  };
}

let previous: FileSource;
beforeEach(() => {
  previous = setFileSource(source());
});
afterEach(() => {
  setFileSource(previous);
  document.querySelectorAll('.doc-suggestions').forEach((node) => {
    node.remove();
  });
  vi.restoreAllMocks();
});

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

/** Pastes files the way a browser reports it. */
function give(editor: Editor, files: File[]): boolean {
  const event = new Event('paste', { bubbles: true, cancelable: true }) as Event & {
    clipboardData: { files: File[]; getData: () => string };
  };
  event.clipboardData = { files, getData: () => '' };
  return Boolean(
    editor.view.someProp('handlePaste', (handle) =>
      handle(editor.view, event as unknown as ClipboardEvent, editor.state.doc.slice(0, 0)),
    ),
  );
}

function press(editor: Editor, key: string): boolean {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  editor.view.dom.dispatchEvent(event);
  return event.defaultPrevented;
}

const pdf = (name = 'minutes.pdf'): File => new File(['%PDF'], name, { type: 'application/pdf' });

/** A turn of the event loop: what the editor's suggestions ask for has been asked by then. */
const aTurn = (): Promise<void> =>
  new Promise((resolve) => {
    setTimeout(resolve, 0);
  });

/**
 * Pastes two files at `at` in `body`, lets the second be up before the
 * first, and gives the body stored and what the editor said at the end.
 */
async function twoGivenTogether(
  body: DocNode,
  at: (editor: Editor) => number,
  files = [pdf('a.pdf'), pdf('b.pdf')],
) {
  const finishes: ((fileId: string) => void)[] = [];
  const uploadFile = vi.fn(
    (_file: File) =>
      new Promise<string>((resolve) => {
        finishes.push(resolve);
      }),
  );
  const onChange = vi.fn<(body: DocNode | null) => void>();
  const { container, unmount } = render(BodyEditor, { doc: body, onChange, uploadFile });
  const editor = editorIn(container);
  editor.view.dispatch(
    editor.state.tr.setSelection(TextSelection.create(editor.state.doc, at(editor))),
  );
  expect(give(editor, files)).toBe(true);
  expect(container.querySelectorAll('.doc-upload')).toHaveLength(files.length);
  finishes[1]?.(`7${files[1]?.name ?? ''}`);
  await aTurn();
  finishes[0]?.(`7${files[0]?.name ?? ''}`);
  await vi.waitFor(() => {
    expect(container.querySelector('.doc-upload')).toBeNull();
  });
  await aTurn();
  const said = container.querySelector('[role="status"]')?.textContent;
  const stored = onChange.mock.lastCall?.[0];
  unmount();
  return { stored, said };
}

describe('a file given to the editor', () => {
  it('becomes a file block once it is up, its place held meanwhile', async () => {
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
    const file = pdf();
    expect(give(editor, [file])).toBe(true);
    expect(uploadFile).toHaveBeenCalledWith(file);
    expect(container.querySelector('.doc-upload')?.getAttribute('aria-label')).toBe(
      'Uploading the file…',
    );
    // Going up is told where every upload is (the stack), not again under the body.
    await Promise.resolve();
    expect(container.querySelector('[role="status"]')?.textContent).toBe('');
    expect(onChange).not.toHaveBeenCalled();

    finish('7minutes.pdf');
    await vi.waitFor(() => {
      expect(onChange).toHaveBeenCalled();
    });
    const reported = onChange.mock.lastCall?.[0];
    expect(reported?.content).toContainEqual(fileBlock('7minutes.pdf'));
    expect(validateDoc(reported)).toEqual([]);
    const block = container.querySelector<HTMLElement>('[data-file-block]');
    await vi.waitFor(() => {
      expect(block?.dataset.state).toBe('shown');
    });
    expect(block?.getAttribute('aria-label')).toBe('File: 회의록 2026-11.pdf, 2.0 KiB');
    expect(block?.querySelector('.doc-file-name')?.textContent).toBe('회의록 2026-11.pdf');
    expect(block?.querySelector('.doc-file-download')?.getAttribute('href')).toBe(
      '/files/wiki-1/7minutes.pdf',
    );
  });

  it('puts files given together in, in the order given, whichever is up first', async () => {
    const minutes = paragraph(text('Minutes:'));
    // On an empty line, which the first takes the place of.
    const onEmpty = await twoGivenTogether(
      doc(minutes, paragraph()),
      (editor) => TextSelection.atEnd(editor.state.doc).from,
    );
    expect(onEmpty.stored?.content).toEqual([minutes, fileBlock('7a.pdf'), fileBlock('7b.pdf')]);
    expect(onEmpty.said).toBe('');
    // At the end of a line, after it.
    const atEnd = await twoGivenTogether(
      doc(minutes),
      (editor) => TextSelection.atEnd(editor.state.doc).from,
    );
    expect(atEnd.stored?.content).toEqual([minutes, fileBlock('7a.pdf'), fileBlock('7b.pdf')]);
    // In the middle of a line, which they split.
    const inside = await twoGivenTogether(doc(paragraph(text('Before after'))), () => 7);
    expect(inside.stored?.content).toEqual([
      paragraph(text('Before')),
      fileBlock('7a.pdf'),
      fileBlock('7b.pdf'),
      paragraph(text(' after')),
    ]);
  });

  it('makes a picture an image and any other file a file block, given together', async () => {
    const picture = new File(['x'], 'chart.png', { type: 'image/png' });
    const { stored } = await twoGivenTogether(
      doc(paragraph(text('Both:'))),
      (editor) => TextSelection.atEnd(editor.state.doc).from,
      [picture, new File(['x'], 'notes.txt', { type: 'text/plain' })],
    );
    expect(stored?.content).toEqual([
      paragraph(text('Both:')),
      image('7chart.png'),
      fileBlock('7notes.txt'),
    ]);
  });

  it('is told to the caller as dropped when its editor was made anew while it went up', async () => {
    let finish: (fileId: string) => void = () => undefined;
    const uploadFile = vi.fn(
      (_file: File) =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    const onUploadDropped = vi.fn();
    const { container, unmount } = render(BodyEditor, {
      doc: doc(paragraph(text('x'))),
      uploadFile,
      onUploadDropped,
    });
    expect(give(editorIn(container), [pdf()])).toBe(true);
    unmount();
    finish('7minutes.pdf');
    await vi.waitFor(() => {
      expect(onUploadDropped).toHaveBeenCalledWith('file');
    });
    expect(lostNote(FILE_DROPPED, CARD_WORDS)).toMatch(
      /^A file you added was not put in: the description/,
    );
    expect(lostNote(FILE_DROPPED, PAGE_WORDS)).toMatch(
      /^A file you added was not put in: the page/,
    );
  });

  it('gives up the place of an upload that failed, and leaves the document as it was', async () => {
    const onChange = vi.fn();
    const uploadFile = vi.fn((_file: File) => Promise.reject(new Error('too large')));
    const { container } = render(BodyEditor, { doc: null, onChange, uploadFile });
    expect(give(editorIn(container), [pdf()])).toBe(true);
    expect(container.querySelector('.doc-upload')).not.toBeNull();
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-upload')).toBeNull();
    });
    // Why is told where every upload is (the stack), and not twice.
    expect(container.querySelector('[role="status"]')?.textContent).toBe('');
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe('a file block', () => {
  it('saves its file under its name by Enter while selected, and stays where it is', async () => {
    const bytes = vi.fn((_fileId: string) => Promise.resolve(new Blob(['%PDF'])));
    setFileSource(source(bytes));
    const saved = vi.spyOn(HTMLAnchorElement.prototype, 'click');
    const body = doc(paragraph(text('a')), fileBlock('7minutes.pdf'), paragraph(text('b')));
    const { container } = render(BodyEditor, { doc: body });
    const editor = editorIn(container);
    await vi.waitFor(() => {
      expect(container.querySelector('[data-file-block]')?.getAttribute('data-state')).toBe(
        'shown',
      );
    });
    // Put back after the test, as every mock of the file is.
    vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:minutes');
    vi.spyOn(URL, 'revokeObjectURL').mockReturnValue(undefined);
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 3)));
    expect(press(editor, 'Enter')).toBe(true);
    expect(bytes).toHaveBeenCalledWith('7minutes.pdf');
    // The bytes are saved under the file's name: the press of a link to them, not the block's own.
    await vi.waitFor(() => {
      const links = saved.mock.contexts.map((link) => link as HTMLAnchorElement);
      expect(links.find((link) => link.href === 'blob:minutes')?.download).toBe(
        '회의록 2026-11.pdf',
      );
    });
    expect(editor.getJSON()).toEqual(body);
  });

  it('says a missing file is missing and has nothing to save', async () => {
    const { container } = render(BodyEditor, { doc: doc(fileBlock('7gone.pdf')) });
    const block = container.querySelector<HTMLElement>('[data-file-block]');
    await vi.waitFor(() => {
      expect(block?.dataset.state).toBe('missing');
    });
    expect(block?.textContent).toBe('Missing file');
    expect(block?.getAttribute('aria-label')).toBe('File: Missing file');
    expect(block?.querySelector('a')).toBeNull();
  });

  it('is drawn where a body is only shown, its link reached by the keyboard', async () => {
    const { container } = render(BodyView, { doc: doc(fileBlock('7sheet.xlsx')) });
    const block = container.querySelector<HTMLElement>('[data-file-block]');
    await vi.waitFor(() => {
      expect(block?.dataset.state).toBe('shown');
    });
    expect(block?.getAttribute('aria-label')).toBe('File: budget.xlsx, 10 B');
    const link = block?.querySelector<HTMLAnchorElement>('a.doc-file-download');
    expect(link?.getAttribute('aria-label')).toBe('Download budget.xlsx');
    expect(link?.tabIndex).toBe(0);
    // Pressed, never dragged into a body as a link.
    expect(link?.getAttribute('draggable')).toBe('false');
  });

  it('saves a file pressed before its record came under the name the record gives', async () => {
    for (const [answer, name] of [
      [() => Promise.resolve({ name: '회의록 2026-11.pdf', size: 4 }), '회의록 2026-11.pdf'],
      // The record cannot be read: under the name drawn, its identifier.
      [() => Promise.reject(new Error('offline')), '7minutes.pdf'],
    ] as const) {
      let tell: () => void = () => undefined;
      const told = new Promise<void>((resolve) => {
        tell = resolve;
      });
      const details = vi.fn(() => told.then(answer));
      const bytes = vi.fn((): Promise<Blob> => Promise.resolve(new Blob(['%PDF'])));
      setFileSource({ url: (fileId) => `/files/wiki-1/${fileId}`, details, bytes });
      const saved = vi.spyOn(HTMLAnchorElement.prototype, 'click');
      vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:minutes');
      vi.spyOn(URL, 'revokeObjectURL').mockReturnValue(undefined);
      const savedAs = (): string[] =>
        saved.mock.contexts
          .map((link) => link as HTMLAnchorElement)
          .filter((link) => link.href === 'blob:minutes')
          .map((link) => link.download);
      const { container, unmount } = render(BodyView, { doc: doc(fileBlock('7minutes.pdf')) });
      const block = container.querySelector<HTMLElement>('[data-file-block]');
      expect(block?.dataset.state).toBe('pending');
      block?.querySelector<HTMLAnchorElement>('a.doc-file-download')?.click();
      await aTurn();
      // The bytes are there; the name is waited for.
      expect(bytes).toHaveBeenCalledTimes(1);
      expect(savedAs()).toEqual([]);
      tell();
      await vi.waitFor(() => {
        expect(savedAs()).toEqual([name]);
      });
      // The press waited for the block's own request of the record, and asked no other.
      expect(details).toHaveBeenCalledTimes(1);
      unmount();
      vi.restoreAllMocks();
    }
  });

  it('stays on the page when its file cannot be read, and says the file is gone if it is', async () => {
    const details = vi
      .fn((_fileId: string) => Promise.resolve<FileDetails | null>(KNOWN['7sheet.xlsx'] ?? null))
      .mockResolvedValueOnce(KNOWN['7sheet.xlsx'] ?? null)
      .mockResolvedValueOnce(null);
    const bytes = vi.fn((_fileId: string) => Promise.reject(new Error('404')));
    setFileSource({ url: (fileId) => `/files/wiki-1/${fileId}`, details, bytes });
    const { container } = render(BodyView, { doc: doc(fileBlock('7sheet.xlsx')) });
    const block = container.querySelector<HTMLElement>('[data-file-block]');
    await vi.waitFor(() => {
      expect(block?.dataset.state).toBe('shown');
    });
    block?.querySelector<HTMLAnchorElement>('a.doc-file-download')?.click();
    await vi.waitFor(() => {
      expect(block?.dataset.state).toBe('missing');
    });
    expect(bytes).toHaveBeenCalledWith('7sheet.xlsx');
    expect(details).toHaveBeenCalledTimes(2);
    expect(block?.querySelector('a')).toBeNull();
  });

  it('is known by its file in the board or wiki shown, and not asked while none is', async () => {
    let shown: string | undefined = 'wiki-1';
    const details = vi.fn((fileId: string) =>
      Promise.resolve<FileDetails>({ name: `${shown ?? ''}:${fileId}`, size: 1 }),
    );
    setFileSource({ url: () => undefined, details, scope: () => shown });
    expect(await fileDetails('7a.pdf')).toEqual({ name: 'wiki-1:7a.pdf', size: 1 });
    expect(await fileDetails('7a.pdf')).toEqual({ name: 'wiki-1:7a.pdf', size: 1 });
    shown = 'board-2';
    expect(await fileDetails('7a.pdf')).toEqual({ name: 'board-2:7a.pdf', size: 1 });
    shown = undefined;
    expect(await fileDetails('7a.pdf')).toBeUndefined();
    expect(details).toHaveBeenCalledTimes(2);
  });

  it('shows an icon for the kind of its file', () => {
    expect(
      ['a.pdf', 'b.HWP', 'c.xlsx', 'd.pptx', 'e.zip', 'f.png', 'g.mp3', 'h', 'i.bin'].map(kindOf),
    ).toEqual([
      'document',
      'document',
      'sheet',
      'slides',
      'archive',
      'image',
      'audio',
      'other',
      'other',
    ]);
  });
});

describe('the slash menu', () => {
  it('finds its entries by any of their names', () => {
    const file = { value: 'file', label: '파일', words: ['file', 'attachment'], run: () => true };
    expect(matching([file], '')).toEqual([file]);
    expect(matching([file], 'FI')).toEqual([file]);
    expect(matching([file], '파')).toEqual([file]);
    expect(matching([file], 'table')).toEqual([]);
  });

  it('offers a file where files can be added, and asks for one', async () => {
    const uploadFile = vi.fn((_file: File) => new Promise<string>(() => undefined));
    const { container } = render(BodyEditor, { doc: doc(paragraph()), uploadFile });
    const editor = editorIn(container);
    const asked = vi.spyOn(HTMLInputElement.prototype, 'click').mockImplementation(function (
      this: HTMLInputElement,
    ) {
      Object.defineProperty(this, 'files', { value: [pdf()] });
      this.dispatchEvent(new Event('change'));
    });
    editor.commands.focus();
    editor.commands.insertContent('/image');
    await vi.waitFor(() => {
      expect(
        document.querySelector('[data-suggestions="slash"] [role="option"]')?.textContent,
      ).toBe('File or image');
    });
    expect(press(editor, 'Enter')).toBe(true);
    expect(asked).toHaveBeenCalledTimes(1);
    expect(uploadFile).toHaveBeenCalledTimes(1);
    expect(editor.getText()).toBe('');
    expect(document.querySelector('input[type="file"]')).toBeNull();
  });

  it('is not offered in code or after anything but a space, and offers no file without uploads', async () => {
    const uploadFile = vi.fn((_file: File) => Promise.resolve('7x.pdf'));
    const blocks = [
      'Heading 1',
      'Heading 2',
      'Heading 3',
      'To-do list',
      'Bulleted list',
      'Numbered list',
      'Quote',
      'Toggle',
      'Callout',
      'Code block',
      'Formula block',
      'Divider',
    ];
    for (const [body, props, offered] of [
      [doc(paragraph()), {}, [...blocks, 'Table']],
      [doc(paragraph()), { uploadFile }, [...blocks, 'Table', 'File or image']],
      [doc(code(undefined)), { uploadFile }, []],
      [doc(paragraph(text('http:'))), { uploadFile }, []],
    ] as const) {
      const { container, unmount } = render(BodyEditor, { doc: body, ...props });
      const editor = editorIn(container);
      // Focused as a press does, without the scrolling the editor's own command asks for later.
      editor.view.dom.focus();
      editor.view.dispatch(
        editor.state.tr.setSelection(TextSelection.atEnd(editor.state.doc)).insertText('/'),
      );
      // The entries are put in the list a turn after the menu is offered.
      await aTurn();
      const options = [...document.querySelectorAll('[data-suggestions="slash"] [role="option"]')];
      expect(options.map((option) => option.textContent)).toEqual(offered);
      if (offered.length === 0) {
        expect(document.querySelector('[data-suggestions="slash"]')).toBeNull();
      }
      unmount();
    }
  });
});
