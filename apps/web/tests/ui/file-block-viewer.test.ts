/**
 * A file block whose file the viewer shows: its name is a button that opens
 * the viewer, in the editor and where a body is only shown, and Enter on the
 * selected block opens it too, leaving the document as it was; a file of
 * another kind, too large, or missing is only downloaded; a viewer that does
 * not come leaves the file to its download. The viewer opened from a block
 * (viewing.svelte.ts) gives the focus back to where it was pressed.
 */
import type { Editor } from '@tiptap/core';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { viewerChunk } from '$lib/components/file-viewer/chunk.svelte.ts';
import { viewerLinkOf, viewing } from '$lib/components/file-viewer/viewing.svelte.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import BodyView from '$lib/editor/BodyView.svelte';
import {
  setFileSource,
  type FileDetails,
  type FileSource,
  type FileViewerLink,
} from '$lib/editor/files.ts';
import type { DocNode } from '$lib/editor/schema.ts';
import { opensInViewer, VIEW_LIMIT_BYTES } from '$lib/file-viewer/extensions.ts';

import { doc, paragraph, text } from '../support/body-docs.ts';

// A caret given back to the text is scrolled into sight, which asks for the size of text: jsdom
// has none.
Range.prototype.getClientRects = function getClientRects() {
  return [] as unknown as DOMRectList;
};
Range.prototype.getBoundingClientRect = function getBoundingClientRect() {
  return new DOMRect();
};

const fileBlock = (fileId: string): DocNode => ({ type: 'file', attrs: { fileId } });

const KNOWN: Record<string, FileDetails> = {
  '7minutes.pdf': { name: '회의록 2026-11.pdf', size: 2048 },
  '7plans.zip': { name: 'plans.zip', size: 10 },
  '7huge.pdf': { name: 'scan.pdf', size: VIEW_LIMIT_BYTES + 1 },
};

/** A viewer that opens whatever it is given, or says it did not come. */
function link(opened = true) {
  return {
    opens: vi.fn((details: FileDetails) => opensInViewer(details.name, details.size)),
    open: vi.fn((_fileId: string, _details: FileDetails, _back: () => HTMLElement | null) =>
      Promise.resolve(opened),
    ),
  } satisfies FileViewerLink;
}

function source(
  view: FileViewerLink | undefined,
  bytes = vi.fn(() => Promise.resolve(new Blob(['%PDF']))),
) {
  return {
    url: (fileId: string) => `/files/wiki-1/${fileId}`,
    details: (fileId: string) => Promise.resolve(KNOWN[fileId] ?? null),
    bytes,
    view,
  } satisfies FileSource;
}

let previous: FileSource;
beforeEach(() => {
  previous = setFileSource({ url: () => undefined });
});
afterEach(() => {
  setFileSource(previous);
  viewing.forget();
  vi.restoreAllMocks();
});

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

function press(editor: Editor, key: string): boolean {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true });
  editor.view.dom.dispatchEvent(event);
  return event.defaultPrevented;
}

async function shown(container: HTMLElement): Promise<HTMLElement[]> {
  const blocks = [...container.querySelectorAll<HTMLElement>('[data-file-block]')];
  await vi.waitFor(() => {
    expect(blocks.map((block) => block.dataset.state)).not.toContain('pending');
  });
  return blocks;
}

describe('the name of a file the viewer shows', () => {
  it('is a button that opens the viewer, where a body is only shown', async () => {
    const viewer = link();
    setFileSource(source(viewer));
    const { container } = render(BodyView, { doc: doc(fileBlock('7minutes.pdf')) });
    const [block] = await shown(container);
    const button = block?.querySelector<HTMLButtonElement>('button.doc-file-open');
    expect(button?.textContent).toBe('회의록 2026-11.pdf');
    expect(button?.getAttribute('aria-label')).toBe('View 회의록 2026-11.pdf');
    expect(button?.type).toBe('button');
    expect(button?.tabIndex).toBe(0);
    // The download stays beside it.
    expect(block?.querySelector('a.doc-file-download')?.getAttribute('aria-label')).toBe(
      'Download 회의록 2026-11.pdf',
    );
    button?.click();
    expect(viewer.open).toHaveBeenCalledWith(
      '7minutes.pdf',
      KNOWN['7minutes.pdf'],
      expect.any(Function),
    );
    // The focus goes back to the button, or to the one drawn for the file in its place.
    const back = viewer.open.mock.lastCall?.[2];
    expect(back?.()).toBe(button);
    const redrawn = document.createElement('button');
    redrawn.className = 'doc-file-open';
    button?.replaceWith(redrawn);
    expect(back?.()).toBe(redrawn);
    redrawn.remove();
    expect(back?.()).toBeNull();
  });

  it('opens the viewer by Enter on the selected block in the editor, which stays as it was', async () => {
    const viewer = link();
    const bytes = vi.fn(() => Promise.resolve(new Blob(['%PDF'])));
    setFileSource(source(viewer, bytes));
    const body = doc(paragraph(text('a')), fileBlock('7minutes.pdf'), paragraph(text('b')));
    const { container } = render(BodyEditor, { doc: body });
    const editor = editorIn(container);
    await shown(container);
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 3)));
    expect(press(editor, 'Enter')).toBe(true);
    expect(viewer.open).toHaveBeenCalledTimes(1);
    expect(viewer.open.mock.lastCall?.[0]).toBe('7minutes.pdf');
    // Closed, the viewer gives the focus to the editor, whose selection the block still is.
    expect(viewer.open.mock.lastCall?.[2]()).toBe(editor.view.dom);
    // Opened, not downloaded.
    expect(bytes).not.toHaveBeenCalled();
    expect(editor.getJSON()).toEqual(body);
    expect(editor.state.selection).toBeInstanceOf(NodeSelection);
  });

  it("takes the caret out of the text while it or the download has the focus; back, the editor's is given back", async () => {
    const viewer = link();
    setFileSource(source(viewer));
    // A body that starts with the block: the browser's caret at the start would select it.
    const body = doc(fileBlock('7minutes.pdf'), paragraph(text('ab')));
    const { container } = render(BodyEditor, { doc: body });
    const editor = editorIn(container);
    const [block] = await shown(container);
    const button = block?.querySelector<HTMLButtonElement>('button.doc-file-open');
    const download = block?.querySelector<HTMLElement>('a.doc-file-download');
    const selection = document.getSelection();
    const written = (): number[] => {
      const { anchorNode, anchorOffset, focusNode, focusOffset } = selection ?? {};
      if (!anchorNode || !focusNode) return [];
      const { view } = editor;
      return [
        view.posAtDOM(anchorNode, anchorOffset ?? 0),
        view.posAtDOM(focusNode, focusOffset ?? 0),
      ];
    };
    editor.view.dom.focus();
    const { doc: now } = editor.state;
    for (const control of [button, download]) {
      for (const chosen of [TextSelection.create(now, 3), NodeSelection.create(now, 0)]) {
        const name = `${control?.className ?? ''} ${chosen instanceof NodeSelection ? 'node' : 'text'}`;
        editor.view.dispatch(editor.state.tr.setSelection(chosen));
        // The focus's events are sent as a browser sends them: jsdom moves the selection into what
        // it focuses, which a browser does not do for a button or a link. Reached by Tab (or by a
        // press), the control leaves no caret for a key to type at...
        control?.dispatchEvent(new FocusEvent('focus'));
        expect(selection?.rangeCount, name).toBe(0);
        // ...and back in the text from it (Shift+Tab), the editor's own selection is written back.
        editor.view.dom.dispatchEvent(new FocusEvent('focus', { relatedTarget: control }));
        expect(written(), name).toEqual([chosen.anchor, chosen.head]);
        expect(editor.state.selection.eq(chosen), name).toBe(true);
        // WebKit then puts its caret at the first place for one, which the editor reads: the
        // selection kept is given back.
        editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(now, 2)));
        expect(editor.state.selection.eq(chosen), name).toBe(true);
      }
    }
    // Not over a selection made after a key, or by a pointer: the user's.
    const start = (): TextSelection => TextSelection.create(editor.state.doc, 2);
    const fromName = (): void => {
      editor.view.dom.dispatchEvent(new FocusEvent('focus', { relatedTarget: button ?? null }));
    };
    fromName();
    editor.view.dom.dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', ctrlKey: true }));
    editor.view.dispatch(editor.state.tr.setSelection(start()));
    expect(editor.state.selection.eq(start())).toBe(true);
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(now, 0)));
    fromName();
    editor.view.dispatch(editor.state.tr.setSelection(start()).setMeta('pointer', true));
    expect(editor.state.selection.eq(start())).toBe(true);
    // Where the focus goes back when the viewer closes: to the name pressed from the keyboard on
    // it, by a pointer that left the focus on it (a double click), or by an assistive technology
    // (a click of no pointer); to the editor pressed by a pointer, after which the editor has it.
    const back = (): unknown => viewer.open.mock.lastCall?.[2]();
    button?.focus();
    button?.click();
    expect(back()).toBe(button);
    button?.dispatchEvent(new MouseEvent('click', { detail: 2, bubbles: true }));
    expect(back()).toBe(button);
    editor.view.dom.focus();
    button?.click();
    expect(back()).toBe(button);
    button?.dispatchEvent(new MouseEvent('click', { detail: 1, bubbles: true }));
    expect(back()).toBe(editor.view.dom);
    expect(editor.getJSON()).toEqual(body);
  });

  it('keeps the keys pressed on it, or on the download, from the editor: they are theirs', async () => {
    const viewer = link();
    setFileSource(source(viewer));
    const body = doc(paragraph(text('a')), fileBlock('7minutes.pdf'), paragraph(text('b')));
    const { container } = render(BodyEditor, { doc: body });
    const editor = editorIn(container);
    const [block] = await shown(container);
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 3)));
    const button = block?.querySelector<HTMLButtonElement>('button.doc-file-open');
    const pressed = (key: string, held: KeyboardEventInit = {}): boolean => {
      const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...held });
      button?.dispatchEvent(event);
      return event.defaultPrevented;
    };
    // Enter and Space are left to the button, which they press, and the arrows to the browser:
    // the editor takes none of them (it would have opened the viewer, or moved on).
    for (const key of ['Enter', ' ', 'ArrowDown']) expect(pressed(key), key).toBe(false);
    // A key that edits is held back by the button, so that the browser edits nothing either.
    for (const key of ['Backspace', 'Delete', 'a', '가']) expect(pressed(key), key).toBe(true);
    // The browser's shortcuts are its own (find, print), but not those it edits by; AltGr types.
    for (const held of [{ ctrlKey: true }, { metaKey: true }]) {
      for (const key of ['f', 'p']) expect(pressed(key, held), key).toBe(false);
      for (const key of ['v', 'X', 'z', 'b']) expect(pressed(key, held), key).toBe(true);
    }
    expect(pressed('@', { ctrlKey: true, altKey: true })).toBe(true);
    // So are the keys an input method takes, and a dead key.
    for (const key of ['Process', 'Dead']) expect(pressed(key), key).toBe(true);
    expect(pressed('Unidentified', { isComposing: true })).toBe(true);
    // What would be typed comes to the editor itself, which takes nothing while the name has the
    // focus; once the focus is back in the text, it does.
    const typed = (): boolean => {
      const event = new InputEvent('beforeinput', {
        inputType: 'insertText',
        data: '가',
        bubbles: true,
        cancelable: true,
      });
      editor.view.dom.dispatchEvent(event);
      return event.defaultPrevented;
    };
    // As on the name, so on the download: a link, which Enter presses.
    const download = block?.querySelector<HTMLElement>('a.doc-file-download');
    for (const control of [button, download]) {
      control?.focus();
      expect(typed(), control?.className).toBe(true);
    }
    const backspace = new KeyboardEvent('keydown', { key: 'Backspace', cancelable: true });
    download?.dispatchEvent(backspace);
    expect(backspace.defaultPrevented).toBe(true);
    editor.view.dom.focus();
    expect(typed()).toBe(false);
    expect(viewer.open).not.toHaveBeenCalled();
    expect(editor.getJSON()).toEqual(body);
    expect(editor.state.selection).toBeInstanceOf(NodeSelection);
  });

  it('is only text for a file of another kind, a file too large, and a missing one', async () => {
    const viewer = link();
    const bytes = vi.fn(() => Promise.resolve(new Blob(['x'])));
    setFileSource(source(viewer, bytes));
    const saved = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockReturnValue(undefined);
    const body = doc(fileBlock('7plans.zip'), fileBlock('7huge.pdf'), fileBlock('7gone.pdf'));
    const { container } = render(BodyEditor, { doc: body });
    const editor = editorIn(container);
    const blocks = await shown(container);
    expect(blocks.map((block) => block.querySelector('button'))).toEqual([null, null, null]);
    expect(blocks.map((block) => block.querySelector('.doc-file-name')?.tagName)).toEqual([
      'SPAN',
      'SPAN',
      'SPAN',
    ]);
    // Enter on the archive saves it, as its link does.
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 0)));
    expect(press(editor, 'Enter')).toBe(true);
    expect(saved).toHaveBeenCalledTimes(1);
    expect(viewer.open).not.toHaveBeenCalled();
  });

  it('is only text where the app has no viewer', async () => {
    setFileSource(source(undefined));
    const { container } = render(BodyView, { doc: doc(fileBlock('7minutes.pdf')) });
    const [block] = await shown(container);
    expect(block?.querySelector('button')).toBeNull();
  });

  it('leaves the file to its download when the viewer does not come', async () => {
    const viewer = link(false);
    const bytes = vi.fn(() => Promise.resolve(new Blob(['%PDF'])));
    setFileSource(source(viewer, bytes));
    vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:minutes');
    vi.spyOn(URL, 'revokeObjectURL').mockReturnValue(undefined);
    const { container } = render(BodyView, { doc: doc(fileBlock('7minutes.pdf')) });
    const [block] = await shown(container);
    block?.querySelector<HTMLButtonElement>('button.doc-file-open')?.click();
    await vi.waitFor(() => {
      expect(bytes).toHaveBeenCalledWith('7minutes.pdf');
    });
  });
});

describe('the viewer opened from a file block', () => {
  it('reads the file by its source, and gives the focus back to what its opener finds', async () => {
    vi.spyOn(viewerChunk, 'load').mockResolvedValue(true);
    const bytes = vi.fn((_fileId: string, _signal?: AbortSignal) =>
      Promise.resolve(new Blob(['%PDF'])),
    );
    const view = viewerLinkOf({ bytes });
    const details = KNOWN['7minutes.pdf'] ?? { name: '', size: 0 };
    expect(view?.opens(details)).toBe(true);
    expect(view?.opens({ name: 'plans.zip', size: 1 })).toBe(false);

    const editor = document.body.appendChild(document.createElement('div'));
    editor.tabIndex = -1;
    const button = document.body.appendChild(document.createElement('button'));
    // The focus goes back to what the opener finds: here, as from a selected block, the editor.
    expect(await view?.open('7minutes.pdf', details, () => editor)).toBe(true);
    expect(viewing.file).toMatchObject({ name: '회의록 2026-11.pdf', size: 2048 });
    const signal = new AbortController().signal;
    await viewing.file?.read(signal);
    expect(bytes).toHaveBeenCalledWith('7minutes.pdf', signal);
    // A press of another file while one is open opens no second viewer, nor this one on it.
    expect(await view?.open('7plans.pdf', { name: 'plans.pdf', size: 1 }, () => button)).toBe(true);
    expect(viewing.file?.name).toBe('회의록 2026-11.pdf');
    // Closed, the viewer gives the focus back once it is gone (its dialog has let go of it).
    button.focus();
    viewing.close();
    expect(viewing.file).toBeUndefined();
    expect(document.activeElement).toBe(button);
    viewing.returnFocus();
    expect(document.activeElement).toBe(editor);

    const shownOnly = document.body.appendChild(document.createElement('button'));
    await view?.open('7minutes.pdf', details, () => shownOnly);
    viewing.close();
    viewing.returnFocus();
    expect(document.activeElement).toBe(shownOnly);
    // Given back once only.
    button.focus();
    viewing.returnFocus();
    expect(document.activeElement).toBe(button);

    // Let go of without the focus moving: the screen it was over is gone.
    await view?.open('7minutes.pdf', details, () => shownOnly);
    viewing.forget();
    viewing.returnFocus();
    expect(viewing.file).toBeUndefined();
    expect(document.activeElement).toBe(button);
    editor.remove();
    button.remove();
    shownOnly.remove();
  });

  it('is none for a source that cannot read bytes, and bytes that cannot be read are none', async () => {
    expect(viewerLinkOf({})).toBeUndefined();
    vi.spyOn(viewerChunk, 'load').mockResolvedValue(true);
    const view = viewerLinkOf({ bytes: () => Promise.reject(new Error('403')) });
    await view?.open('7minutes.pdf', { name: 'a.pdf', size: 1 }, () => document.body);
    expect(await viewing.file?.read()).toBeUndefined();
  });
});

describe('the files the viewer is likely to show', () => {
  it.each([
    ['a.pdf', 1, true],
    ['보고서.DOCX', 1, true],
    ['표.xlsx', 1, true],
    ['a.XLSM', 1, true],
    ['a.xls', 1, false],
    ['발표.pptx', 1, true],
    ['a.ppt', 1, false],
    ['기안.hwp', 1, true],
    ['기안.HWPX', 1, true],
    ['a.hwt', 1, false],
    ['a.pdf', VIEW_LIMIT_BYTES, true],
    ['a.pdf', VIEW_LIMIT_BYTES + 1, false],
    ['a.zip', 1, false],
    ['pdf', 1, false],
    ['a.pdf.exe', 1, false],
    ['a.', 1, false],
  ])('%s of %i bytes: %s', (name, size, opens) => {
    expect(opensInViewer(name, size)).toBe(opens);
  });
});
