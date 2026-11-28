/**
 * The tools around a body (EditorTools.svelte): the bubble over chosen text
 * and over a chosen image, the bar over a phone's keyboard, and an image
 * opened larger, in the editor and where a body is only shown. Runs the real
 * components in a simulated page, without layout.
 */
import type { Editor } from '@tiptap/core';
import { NodeSelection, TextSelection } from '@tiptap/pm/state';
import { fireEvent, render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import BodyEditor from '$lib/editor/BodyEditor.svelte';
import BodyView from '$lib/editor/BodyView.svelte';
import { setFileSource, type FileSource } from '$lib/editor/files.ts';
import { toStoredDoc, type DocNode } from '$lib/editor/schema.ts';

import {
  callout,
  doc,
  image,
  link,
  math,
  paragraph,
  tasks,
  task,
  text,
  toggle,
} from '../support/body-docs.ts';

vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: () => Promise.resolve({ ok: false, reason: 'failed' }),
}));

// Choosing scrolls the selection into sight, which asks for the size of text: jsdom has none.
Range.prototype.getClientRects = function getClientRects() {
  return [] as unknown as DOMRectList;
};
Range.prototype.getBoundingClientRect = function getBoundingClientRect() {
  return new DOMRect();
};

let previous: FileSource | undefined;

beforeEach(() => {
  previous = setFileSource({ url: (fileId) => `/files/board-1/${fileId}` });
});

afterEach(() => {
  if (previous) setFileSource(previous);
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

/** A phone: a touch screen of the compact width. */
function onAPhone(): void {
  vi.spyOn(window, 'matchMedia').mockImplementation((query: string): MediaQueryList => {
    const list = new EventTarget() as MediaQueryList;
    Object.assign(list, { matches: true, media: query, onchange: null });
    return list;
  });
}

/** What was found, or the test stops: it is not there. */
function found<T>(value: T | null | undefined, what: string): T {
  if (value === null || value === undefined) throw new Error(`no ${what}`);
  return value;
}

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

const stored = (editor: Editor): DocNode => toStoredDoc(editor.getJSON() as DocNode);
const bubble = (): HTMLElement | null => document.querySelector('.doc-bubble');
const tool = (format: string): HTMLButtonElement => {
  const found = document.querySelector<HTMLButtonElement>(`[data-format="${format}"]`);
  if (!found) throw new Error(`no ${format}`);
  return found;
};
/** The buttons of the tools that Tab stops at: one in each bar. */
const tabStops = (): (string | undefined)[] =>
  [...document.querySelectorAll<HTMLElement>('[role="toolbar"] [tabindex="0"]')].map(
    (button) => button.dataset.format,
  );
const lightbox = (): HTMLDialogElement | null =>
  document.querySelector('[data-testid="image-lightbox"]');

/** An editor of `content`, focused, with `from`…`to` chosen. */
async function choose(content: DocNode, from: number, to: number, props = {}): Promise<Editor> {
  const { container } = render(BodyEditor, { doc: content, ...props });
  const editor = editorIn(container);
  editor.view.dom.focus();
  editor.view.dispatch(
    editor.state.tr.setSelection(TextSelection.create(editor.state.doc, from, to)),
  );
  await tick();
  return editor;
}

describe('the bubble over chosen text', () => {
  it('shows over chosen text only, and puts a mark on and off', async () => {
    const editor = await choose(doc(paragraph(text('hello world'))), 1, 6);
    expect(bubble()?.getAttribute('aria-label')).toBe('Formatting');
    expect(
      [...(bubble()?.querySelectorAll('button') ?? [])].map((button) => button.dataset.format),
    ).toEqual(['bold', 'italic', 'strike', 'code', 'link', 'formula']);
    expect(tabStops()).toEqual(['bold']);
    await fireEvent.click(tool('bold'));
    expect(stored(editor)).toEqual(doc(paragraph(text('hello', { type: 'bold' }), text(' world'))));
    expect(tool('bold').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(tool('bold'));
    expect(stored(editor)).toEqual(doc(paragraph(text('hello world'))));

    editor.commands.setTextSelection(3);
    await tick();
    expect(bubble()).toBeNull();
  });

  it('keeps the selection in the editor on a press', async () => {
    await choose(doc(paragraph(text('hello world'))), 1, 6);
    const press = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    tool('italic').dispatchEvent(press);
    expect(press.defaultPrevented).toBe(true);
  });

  it('asks for the address of a link, and refuses one that is not a link', async () => {
    const editor = await choose(doc(paragraph(text('see the site'))), 9, 13);
    await fireEvent.click(tool('link'));
    const field = document.querySelector<HTMLInputElement>('.doc-link-field');
    expect(field).not.toBeNull();
    expect(document.activeElement).toBe(field);
    if (!field) return;
    await fireEvent.input(field, { target: { value: 'javascript:alert(1)' } });
    await fireEvent.keyDown(field, { key: 'Enter' });
    expect(document.querySelector('[role="alert"]')?.textContent).toBe(
      'This address cannot be a link.',
    );
    expect(JSON.stringify(stored(editor))).not.toContain('link');

    await fireEvent.input(field, { target: { value: 'example.com' } });
    await fireEvent.keyDown(field, { key: 'Enter' });
    expect(stored(editor)).toEqual(
      doc(paragraph(text('see the '), text('site', link('https://example.com')))),
    );
  });

  it('takes a host and its port for a web address, and leaves the Enter of a syllable alone', async () => {
    const editor = await choose(doc(paragraph(text('see the site'))), 9, 13);
    await fireEvent.click(tool('link'));
    const field = found(document.querySelector<HTMLInputElement>('.doc-link-field'), 'field');
    await fireEvent.input(field, { target: { value: 'localhost:3000/x' } });
    // Safari ends a syllable with an Enter it marks only by its key code.
    await fireEvent.keyDown(field, { key: 'Enter', keyCode: 229 });
    expect(JSON.stringify(stored(editor))).not.toContain('link');
    await fireEvent.keyDown(field, { key: 'Enter' });
    expect(stored(editor)).toEqual(
      doc(paragraph(text('see the '), text('site', link('https://localhost:3000/x')))),
    );
  });

  it('keeps the focus in the editor on a press between its buttons', async () => {
    await choose(doc(paragraph(text('hello world'))), 1, 6);
    const press = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    found(bubble(), 'bubble').dispatchEvent(press);
    expect(press.defaultPrevented).toBe(true);
  });

  it('shows over text chosen by a drag that ended in a drop', async () => {
    const editor = await choose(doc(paragraph(text('hello world'))), 1, 6);
    // ProseMirror asks where a press is; a simulated page has no layout to say it.
    Object.defineProperty(document, 'elementFromPoint', { value: () => null, configurable: true });
    await fireEvent.mouseDown(editor.view.dom);
    expect(bubble()).toBeNull();
    await fireEvent(document, new Event('dragend', { bubbles: true }));
    expect(bubble()).not.toBeNull();
    Reflect.deleteProperty(document, 'elementFromPoint');
  });

  it('goes back on Escape without letting it reach what the body is in', async () => {
    await choose(doc(paragraph(text('see the site'))), 9, 13);
    await fireEvent.click(tool('link'));
    const outside = vi.fn();
    document.addEventListener('keydown', outside);
    const field = document.querySelector<HTMLInputElement>('.doc-link-field');
    if (field) await fireEvent.keyDown(field, { key: 'Escape' });
    document.removeEventListener('keydown', outside);
    expect(outside).not.toHaveBeenCalled();
    expect(document.querySelector('.doc-link-field')).toBeNull();
  });

  it('makes chosen text a formula', async () => {
    const editor = await choose(doc(paragraph(text('so x^2 holds'))), 4, 7);
    await fireEvent.click(tool('formula'));
    expect(stored(editor)).toEqual(doc(paragraph(text('so '), math('x^2'), text(' holds'))));
  });

  it('goes along with the arrows', async () => {
    await choose(doc(paragraph(text('hello world'))), 1, 6);
    tool('bold').focus();
    await fireEvent.keyDown(tool('bold'), { key: 'ArrowRight' });
    expect(document.activeElement).toBe(tool('italic'));
    await fireEvent.keyDown(tool('italic'), { key: 'End' });
    expect(document.activeElement).toBe(tool('formula'));
    // Tab comes back to the button focused last.
    expect(tabStops()).toEqual(['formula']);
  });
});

describe('an image opened larger', () => {
  it('opens from the bubble of a chosen image, and closes by Escape, beside it, or its button', async () => {
    const { container } = render(BodyEditor, { doc: doc(image('pic', 'chart')) });
    const editor = editorIn(container);
    editor.view.dom.focus();
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 0)));
    await tick();
    expect(tool('zoom').textContent.trim()).toBe('View larger');

    const open = async (): Promise<HTMLDialogElement> => {
      await fireEvent.click(tool('zoom'));
      const shown = lightbox();
      if (!shown) throw new Error('not opened');
      return shown;
    };
    let shown = await open();
    expect(shown.getAttribute('aria-label')).toBe('chart');
    expect(shown.querySelector('img')?.getAttribute('src')).toBe('/files/board-1/pic');

    const outside = vi.fn();
    document.addEventListener('keydown', outside);
    await fireEvent.keyDown(shown, { key: 'Escape' });
    document.removeEventListener('keydown', outside);
    expect(outside).not.toHaveBeenCalled();
    expect(lightbox()).toBeNull();
    // The focus is back in the editor, the image still chosen.
    expect(document.activeElement).toBe(editor.view.dom);
    expect(editor.state.selection).toBeInstanceOf(NodeSelection);

    shown = await open();
    await fireEvent.click(shown);
    expect(lightbox()).toBeNull();

    shown = await open();
    await fireEvent.click(found(shown.querySelector('img'), 'image'));
    expect(lightbox()).not.toBeNull();
    await fireEvent.click(found(shown.querySelector('button[aria-label="Close"]'), 'close'));
    expect(lightbox()).toBeNull();
  });

  it('opens on a press where the editor is read-only, not where it is written in', async () => {
    const { container, rerender } = render(BodyEditor, {
      doc: doc(image('pic')),
      editable: false,
    });
    const picture = (): HTMLImageElement =>
      found(container.querySelector<HTMLImageElement>('.ProseMirror img'), 'image');
    // The press is the image's alone: what the body is in (a card that joins on a press) has none.
    const outside = vi.fn();
    container.addEventListener('click', outside);
    await fireEvent.click(picture());
    expect(outside).not.toHaveBeenCalled();
    expect(lightbox()?.getAttribute('aria-label')).toBe('Image');
    await fireEvent.click(found(lightbox()?.querySelector('button'), 'close'));

    await rerender({ editable: true });
    await fireEvent.click(picture());
    expect(lightbox()).toBeNull();
  });

  it('opens from a body only shown, by a press or by its keys, and is a button by its description', async () => {
    const { container } = render(BodyView, { doc: doc(image('pic', 'chart'), image('two')) });
    const [first, second] = [...container.querySelectorAll('img')];
    expect(first?.getAttribute('role')).toBe('button');
    expect(first?.tabIndex).toBe(0);
    expect(first?.getAttribute('aria-label')).toBe('View larger: chart');
    expect(second?.getAttribute('aria-label')).toBe('View larger');
    await fireEvent.click(found(first, 'image'));
    expect(lightbox()?.querySelector('img')?.getAttribute('src')).toBe('/files/board-1/pic');
    await fireEvent.keyDown(found(lightbox(), 'lightbox'), { key: 'Escape' });
    await fireEvent.keyDown(found(second, 'image'), { key: 'Enter' });
    expect(lightbox()?.querySelector('img')?.getAttribute('src')).toBe('/files/board-1/two');
  });

  it('does not open where a press of the body is the caller’s', async () => {
    const { container } = render(BodyView, { doc: doc(image('pic')), zoomImages: false });
    const picture = found(container.querySelector('img'), 'image');
    expect(picture.hasAttribute('role')).toBe(false);
    await fireEvent.click(picture);
    expect(lightbox()).toBeNull();
  });
});

describe('the bar over a phone’s keyboard', () => {
  it('shows while the editor has the focus, and does what it names', async () => {
    onAPhone();
    const { container } = render(BodyEditor, {
      doc: doc(paragraph(text('buy milk'))),
      uploadFile: () => Promise.resolve('x.png'),
    });
    const editor = editorIn(container);
    const bar = (): HTMLElement | null => container.querySelector('[role="toolbar"]');
    expect(bar()).toBeNull();
    editor.view.dom.focus();
    await tick();
    expect(bar()?.dataset.testid).toBe('mobile-editor-toolbar');
    expect([...(bar()?.querySelectorAll('button') ?? [])].map((b) => b.dataset.format)).toEqual([
      'heading-1',
      'heading-2',
      'heading-3',
      'task-list',
      'bullet-list',
      'numbered-list',
      'quote',
      'toggle',
      'callout',
      'table',
      'file',
      'bold',
      'italic',
      'strike',
      'code',
      'link',
      'formula',
      'undo',
      'redo',
    ]);
    expect(tool('undo').disabled).toBe(true);
    expect(tabStops()).toEqual(['heading-1']);
    await fireEvent.click(tool('task-list'));
    expect(stored(editor)).toEqual(doc(tasks(task(false, paragraph(text('buy milk'))))));
    expect(tool('task-list').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(tool('undo'));
    expect(stored(editor)).toEqual(doc(paragraph(text('buy milk'))));
    // A toggle and a callout, each made of the line and taken back the same way.
    await fireEvent.click(tool('toggle'));
    expect(stored(editor)).toEqual(doc(toggle([text('buy milk')], paragraph())));
    expect(tool('toggle').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(tool('toggle'));
    expect(stored(editor)).toEqual(doc(paragraph(text('buy milk'))));
    await fireEvent.click(tool('callout'));
    expect(stored(editor)).toEqual(doc(callout('info', paragraph(text('buy milk')))));
    expect(tool('callout').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(tool('callout'));
    expect(stored(editor)).toEqual(doc(paragraph(text('buy milk'))));

    // Chosen text gets no bubble of marks: they are on the bar.
    editor.view.dispatch(
      editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1, 4)),
    );
    await tick();
    expect(bubble()).toBeNull();

    editor.view.dom.blur();
    await tick();
    expect(bar()).toBeNull();
  });

  it('shows for an editor given the focus as it is made', async () => {
    onAPhone();
    const { container } = render(BodyEditor, {
      doc: doc(paragraph(text('today'))),
      onEditor: (made: Editor) => {
        made.view.focus();
      },
    });
    await tick();
    expect(container.querySelector('[data-testid="mobile-editor-toolbar"]')).not.toBeNull();
  });

  it('puts a link on chosen text from an address asked for in the bar', async () => {
    onAPhone();
    const editor = await choose(doc(paragraph(text('see the site'))), 9, 13);
    // A press on a button that cannot be pressed now goes through to the bar, which keeps the focus.
    const press = new MouseEvent('mousedown', { bubbles: true, cancelable: true });
    found(document.querySelector('[data-testid="mobile-editor-toolbar"]'), 'bar').dispatchEvent(
      press,
    );
    expect(press.defaultPrevented).toBe(true);
    expect(tool('formula').disabled).toBe(false);
    await fireEvent.click(tool('link'));
    const field = found(document.querySelector<HTMLInputElement>('.doc-link-field'), 'field');
    expect(field.closest('[data-testid="mobile-editor-toolbar"]')).not.toBeNull();
    expect(document.activeElement).toBe(field);
    await fireEvent.input(field, { target: { value: 'example.com' } });
    await fireEvent.keyDown(field, { key: 'Enter' });
    expect(stored(editor)).toEqual(
      doc(paragraph(text('see the '), text('site', link('https://example.com')))),
    );
    expect(document.activeElement).toBe(editor.view.dom);
    expect(document.querySelector('.doc-link-field')).toBeNull();
    expect(tool('link').getAttribute('aria-pressed')).toBe('true');
  });
});
