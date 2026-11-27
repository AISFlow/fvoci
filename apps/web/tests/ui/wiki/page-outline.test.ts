/**
 * The outline of a wiki's page (page-outline.svelte.ts, outline-dom.ts,
 * OutlineNav.svelte): read from the body as it is drawn, by the read-only
 * view and by the editor bound to the room's document, again only when a
 * heading changed, and never writing into the document.
 */
import type { Editor } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import BodyView from '$lib/editor/BodyView.svelte';
import {
  headingText,
  lastAtOrAbove,
  outlineOf,
  touchesHeadings,
} from '$lib/features/wiki/outline-dom.ts';
import OutlineNav from '$lib/features/wiki/OutlineNav.svelte';
import { OUTLINE_STORAGE_KEY, PageOutline } from '$lib/features/wiki/page-outline.svelte.ts';

import { doc, heading, paragraph, text } from '../../support/body-docs.ts';

/** Frames asked for, run when the test says. */
let frames: FrameRequestCallback[] = [];

async function nextFrame(): Promise<void> {
  // The observers' records arrive in a microtask; the reading waits for the frame.
  await tick();
  const due = frames;
  frames = [];
  for (const run of due) run(0);
  await tick();
}

beforeEach(() => {
  frames = [];
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
    frames.push(callback);
    return frames.length;
  });
  vi.stubGlobal('cancelAnimationFrame', () => undefined);
  localStorage.clear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const words = (outline: PageOutline): string[] =>
  outline.entries.map((entry) => `${entry.level} ${entry.text}`);

describe('the headings of a body as drawn', () => {
  it('reads levels 1 to 3 in order, a formula as its source and a line break as a space, leaving out headings without words', () => {
    const root = document.createElement('div');
    root.innerHTML = `
      <h2 data-testid="page-title">Not the body</h2>
      <div class="doc-body">
        <h1>Intro</h1>
        <p>words</p>
        <h2>Area <span class="doc-math" data-latex="\\pi r^2"><span>garbage</span></span></h2>
        <h3>One<br>Two <span aria-hidden="true">caret</span></h3>
        <h2>  </h2>
        <h3><span data-mention>@Ada</span> notes</h3>
      </div>`;
    expect(outlineOf(root).map((entry) => `${entry.level} ${entry.text}`)).toEqual([
      '1 Intro',
      '2 Area \\pi r^2',
      '3 One Two',
      '3 @Ada notes',
    ]);
    const third = root.querySelector('h3');
    expect(third && headingText(third)).toBe('One Two');
  });

  it('tells a change of a heading from typing elsewhere', () => {
    const root = document.createElement('div');
    root.innerHTML =
      '<div class="doc-body"><h2>Head</h2><p>Para</p><ul><li><p>x</p></li></ul></div>';
    const [head, para] = [root.querySelector('h2'), root.querySelector('p')];
    const typed = (node: Node | null | undefined): MutationRecord =>
      ({
        type: 'characterData',
        target: node,
        addedNodes: [],
        removedNodes: [],
      }) as unknown as MutationRecord;
    const added = (node: Node): MutationRecord =>
      ({
        type: 'childList',
        target: root.firstElementChild,
        addedNodes: [node],
        removedNodes: [],
      }) as unknown as MutationRecord;
    expect(touchesHeadings([typed(para?.firstChild)])).toBe(false);
    expect(touchesHeadings([typed(head?.firstChild)])).toBe(true);
    expect(touchesHeadings([added(document.createElement('p'))])).toBe(false);
    expect(touchesHeadings([added(document.createElement('h3'))])).toBe(true);
    const quote = document.createElement('blockquote');
    quote.innerHTML = '<h1>Inside</h1>';
    expect(touchesHeadings([added(quote)])).toBe(true);
  });

  it('finds the last place at a line or above it by halving, asking for a few tops only', () => {
    const tops = Array.from({ length: 300 }, (_, at) => at * 100);
    const asked: number[] = [];
    const topOf = (at: number): number => {
      asked.push(at);
      return tops[at] ?? Infinity;
    };
    expect(lastAtOrAbove(300, topOf, 12_345)).toBe(123);
    expect(asked.length).toBeLessThanOrEqual(10);
    expect(lastAtOrAbove(300, topOf, -1)).toBe(-1);
    expect(lastAtOrAbove(300, topOf, 1e9)).toBe(299);
    expect(lastAtOrAbove(0, topOf, 0)).toBe(-1);
  });
});

describe('the outline of the read-only view', () => {
  it('follows the body as it is drawn anew, and has no mode below two headings', async () => {
    const outline = new PageOutline();
    const { container, rerender } = render(BodyView, {
      doc: doc(heading(1, text('Only')), paragraph(text('body'))),
    });
    const stop = outline.source(container);
    await nextFrame();
    expect(words(outline)).toEqual(['1 Only']);
    expect(outline.mode).toBe('none');
    await rerender({
      doc: doc(heading(1, text('Only')), heading(2, text('Second')), heading(3, text('Third'))),
    });
    await nextFrame();
    expect(words(outline)).toEqual(['1 Only', '2 Second', '3 Third']);
    // No column measured: behind the button.
    expect(outline.mode).toBe('button');
    stop();
    expect(outline.entries).toEqual([]);
  });

  it('is beside the page where its column has room for it and the page at its narrowest, unless folded away, which the device keeps', () => {
    const outline = new PageOutline();
    outline.entries = outlineOf(headings(2));
    const scroller = document.createElement('div');
    // The page gives up its width from 48rem down to 40rem; the outline is 12rem.
    Object.defineProperty(scroller, 'clientWidth', { value: 52 * 16 });
    const stop = outline.follow(scroller);
    expect(outline.mode).toBe('side');
    // The width is read with the scrollbar's room kept, so the page coming to overflow does not change it.
    expect(scroller.style.getPropertyValue('scrollbar-gutter')).toBe('stable');
    // Its place is kept for a page with too few headings: the page is drawn there all the same.
    outline.entries = outlineOf(headings(1));
    expect([outline.mode, outline.where]).toEqual(['none', 'side']);
    outline.entries = outlineOf(headings(2));
    outline.setFolded(true);
    expect(outline.mode).toBe('button');
    expect(localStorage.getItem(OUTLINE_STORAGE_KEY)).toBe('folded');
    expect(new PageOutline().folded).toBe(true);
    outline.setFolded(false);
    expect(new PageOutline().folded).toBe(false);
    stop();
    expect(scroller.style.getPropertyValue('scrollbar-gutter')).toBe('');
    const narrow = document.createElement('div');
    Object.defineProperty(narrow, 'clientWidth', { value: 52 * 16 - 1 });
    const stopNarrow = outline.follow(narrow);
    expect(outline.mode).toBe('button');
    outline.entries = outlineOf(headings(1));
    expect([outline.mode, outline.where]).toEqual(['none', 'button']);
    outline.entries = outlineOf(headings(2));

    // The list behind the button, left out as the outline went away, is shut when it comes back.
    outline.open = true;
    outline.entries = outlineOf(headings(1));
    expect(outline.mode).toBe('none');
    outline.entries = outlineOf(headings(2));
    expect(outline.mode).toBe('button');
    expect(outline.open).toBe(false);
    // And when, folded away, there comes room beside the page: the button then unfolds it.
    outline.setFolded(true);
    outline.open = true;
    stopNarrow();
    outline.follow(scroller);
    expect(outline.room).toBe(true);
    expect(outline.mode).toBe('button');
    expect(outline.open).toBe(false);
    outline.setFolded(false);
  });

  it('keeps the heading chosen marked through typing, and lets go of it once a key that moves the page, a press or the wheel scrolls it', async () => {
    const outline = new PageOutline();
    const scroller = document.createElement('div');
    scroller.scrollTo = vi.fn();
    const body = headings(3);
    scroller.append(body);
    document.body.append(scroller);
    outline.follow(scroller);
    outline.source(body);
    await nextFrame();
    // Nothing is measured here: every top is 0, at the reading line, so the last is read.
    expect(outline.current).toBe(2);
    const inBody = body.querySelector('h2') as HTMLElement;
    // Typed into what is edited (jsdom does not work this out from `contenteditable`).
    Object.defineProperty(inBody, 'isContentEditable', { value: true });
    const scrolled = async (): Promise<void> => {
      scroller.dispatchEvent(new Event('scroll'));
      await nextFrame();
    };
    const key = (init: KeyboardEventInit): void => {
      inBody.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...init }));
    };
    outline.go(0);
    // Typing scrolls the page only to keep the caret in sight: a letter, Enter, an input method's key.
    for (const typed of [{ key: 'a' }, { key: 'Enter' }, { key: 'Process', isComposing: true }]) {
      key(typed);
      await scrolled();
      expect(outline.current).toBe(0);
    }
    // A key that moves the page, then the page scrolled: the reading line's again.
    key({ key: 'PageDown' });
    await scrolled();
    expect(outline.current).toBe(2);
    // Paste, cut and undo write, as does a letter typed with AltGr (Control and Alt); Shift
    // alone, held for a capital, moves nothing.
    outline.go(0);
    for (const chord of [
      { key: 'Shift', shiftKey: true },
      { key: 'v', ctrlKey: true },
      { key: 'z', metaKey: true },
      { key: 'ę', ctrlKey: true, altKey: true },
    ]) {
      key(chord);
      await scrolled();
      expect(outline.current).toBe(0);
    }
    // Tab, and the browser's Find, may scroll the page.
    for (const moved of [
      { key: 'Tab' },
      { key: 'f', ctrlKey: true },
      { key: 'g', metaKey: true },
    ]) {
      outline.go(0);
      key(moved);
      await scrolled();
      expect(outline.current).toBe(2);
    }
    // A key that moves the page and then a letter: the scroll is the caret's.
    outline.go(0);
    key({ key: 'ArrowDown' });
    key({ key: 'b' });
    await scrolled();
    expect(outline.current).toBe(0);
    // The scroll of a heading gone to is not the reader's, even after a key that moves the page.
    key({ key: 'ArrowDown' });
    outline.go(0);
    await scrolled();
    expect(outline.current).toBe(0);
    // A press that scrolls nothing keeps it; the wheel, as it scrolls, lets go of it.
    inBody.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    await nextFrame();
    expect(outline.current).toBe(0);
    scroller.dispatchEvent(new Event('wheel'));
    await scrolled();
    expect(outline.current).toBe(2);
    scroller.remove();
  });

  it('puts the focus on the heading chosen, which gives it back as it was once it leaves', async () => {
    const outline = new PageOutline();
    const scroller = document.createElement('div');
    const scrolled = vi.fn();
    scroller.scrollTo = scrolled;
    const { container } = render(BodyView, {
      doc: doc(heading(2, text('Same')), paragraph(text('a')), heading(2, text('Same'))),
    });
    scroller.append(container);
    document.body.append(scroller);
    outline.follow(scroller);
    outline.source(container);
    await nextFrame();
    const second = container.querySelectorAll('h2')[1];
    expect(outline.go(1)).toBe(true);
    // The one chosen of two with the same words.
    expect(document.activeElement).toBe(second);
    expect(outline.current).toBe(1);
    expect(scrolled).toHaveBeenCalledTimes(1);
    second?.blur();
    expect(second?.hasAttribute('tabindex')).toBe(false);
    scroller.remove();
  });
});

function headings(count: number): HTMLElement {
  const root = document.createElement('div');
  root.innerHTML = `<div class="doc-body">${Array.from({ length: count }, (_, at) => `<h2>H${at}</h2>`).join('')}</div>`;
  return root;
}

function editorIn(container: HTMLElement): Editor {
  const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
  if (!root?.editor) throw new Error('no editor in the component');
  return root.editor;
}

/** Somebody else writes into the shared document: `change` runs on their copy of it. */
function fromTheRoom(shared: Y.Doc, change: (body: Y.XmlFragment) => void): void {
  const other = new Y.Doc();
  Y.applyUpdate(other, Y.encodeStateAsUpdate(shared));
  const before = Y.encodeStateVector(other);
  change(other.getXmlFragment(BODY_FIELD));
  Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, before), 'room');
}

function block(type: string, words: string, level?: number): Y.XmlElement {
  const element = new Y.XmlElement(type);
  if (level !== undefined) element.setAttribute('level', level as unknown as string);
  element.insert(0, [new Y.XmlText(words)]);
  return element;
}

describe('the outline of the editor bound to the room', () => {
  it('follows what others write, reads again only for a heading, and changes nothing in the document', async () => {
    const shared = new Y.Doc();
    fromTheRoom(shared, (body) => {
      body.insert(0, [
        block('heading', 'First', 1),
        block('paragraph', 'Words'),
        block('heading', 'Second', 2),
      ]);
    });
    const { container } = render(BodyEditor, { doc: null, collab: shared });
    const editor = editorIn(container);
    const scroller = document.createElement('div');
    scroller.scrollTo = vi.fn();
    scroller.append(container);
    document.body.append(scroller);
    const bytes = Y.encodeStateAsUpdate(shared);
    const json = JSON.stringify(editor.getJSON());

    const outline = new PageOutline();
    outline.follow(scroller);
    outline.source(container);
    await nextFrame();
    expect(words(outline)).toEqual(['1 First', '2 Second']);
    // Being there, and reading the headings, wrote nothing into the document.
    expect(Y.encodeStateAsUpdate(shared)).toEqual(bytes);
    expect(JSON.stringify(editor.getJSON())).toBe(json);

    // Somebody else adds a heading at the end.
    fromTheRoom(shared, (body) => {
      body.insert(body.length, [block('heading', 'Third', 3)]);
    });
    await nextFrame();
    expect(words(outline)).toEqual(['1 First', '2 Second', '3 Third']);

    // Typing in the paragraph is not a reason to read the headings again; typing in one is.
    const reads = vi.spyOn(container, 'querySelectorAll');
    const paragraphEnd = 1 + editor.state.doc.child(0).nodeSize + 6;
    editor.view.dispatch(editor.state.tr.insertText('!', paragraphEnd));
    await nextFrame();
    expect(reads).not.toHaveBeenCalled();
    editor.view.dispatch(editor.state.tr.insertText(' heading', 6));
    await nextFrame();
    expect(reads).toHaveBeenCalledTimes(1);
    expect(words(outline)[0]).toBe('1 First heading');
    reads.mockRestore();

    // Going to a heading, and reading the outline again, write nothing into the document.
    const typedBytes = Y.encodeStateAsUpdate(shared);
    const typedJson = JSON.stringify(editor.getJSON());
    expect(typedJson).not.toBe(json);
    expect(outline.go(1)).toBe(true);
    await nextFrame();
    outline.go(0);
    await nextFrame();
    expect(Y.encodeStateAsUpdate(shared)).toEqual(typedBytes);
    expect(JSON.stringify(editor.getJSON())).toBe(typedJson);
    // The caret is at the start of the heading gone to.
    const { selection } = editor.state;
    expect(selection instanceof TextSelection && selection.$from.parent.textContent).toBe(
      'First heading',
    );
    scroller.remove();
  });
});

describe('the list of the outline', () => {
  it('is a named list of the headings, each level further in, the one being read marked', async () => {
    const outline = new PageOutline();
    outline.entries = outlineOf(
      Object.assign(document.createElement('div'), {
        innerHTML: '<div class="doc-body"><h2>Top</h2><h3>Inner</h3><h2>Next</h2></div>',
      }),
    );
    outline.current = 1;
    render(OutlineNav, { outline });
    const nav = screen.getByRole('navigation', { name: 'Contents' });
    const items = nav.querySelectorAll('button');
    expect(Array.from(items, (item) => item.textContent.trim())).toEqual(['Top', 'Inner', 'Next']);
    expect(items[1]).toHaveAttribute('aria-current', 'location');
    expect(items[0]).not.toHaveAttribute('aria-current');
    expect(items[0]?.style.paddingInlineStart).toBe('0.5rem');
    expect(items[1]?.style.paddingInlineStart).toBe('1.25rem');
    // One stop of the Tab key, at the one being read, which tells the keys that go along the list.
    expect(Array.from(items, (item) => item.tabIndex)).toEqual([-1, 0, -1]);
    expect(items[1]).toHaveAccessibleDescription(
      'The arrow keys, Home and End go from heading to heading; Enter goes to the one chosen.',
    );
    expect(items[0]).not.toHaveAccessibleDescription();
    outline.current = 2;
    await tick();
    expect(items[2]).toHaveAttribute('aria-current', 'location');
    expect(Array.from(items, (item) => item.tabIndex)).toEqual([-1, -1, 0]);
    // The arrows, Home and End go along the list, and stop at its ends.
    items[2]?.focus();
    await fireEvent.keyDown(items[2] as HTMLElement, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[2]);
    await fireEvent.keyDown(items[2] as HTMLElement, { key: 'Home' });
    expect(document.activeElement).toBe(items[0]);
    await fireEvent.keyDown(items[0] as HTMLElement, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[1]);
    await fireEvent.keyDown(items[1] as HTMLElement, { key: 'End' });
    expect(document.activeElement).toBe(items[2]);
    // Without a page to scroll, choosing one goes nowhere.
    const chose = vi.fn();
    cleanup();
    render(OutlineNav, { outline, onChose: chose });
    const first = screen.getByRole('button', { name: 'Top' });
    await fireEvent.click(first);
    expect(chose).toHaveBeenCalledWith(false);
  });

  it('goes to the heading chosen also where nobody listens for the choice (beside the page)', async () => {
    cleanup();
    const outline = new PageOutline();
    const scroller = document.createElement('div');
    const scrolled = vi.fn();
    scroller.scrollTo = scrolled;
    const body = headings(3);
    scroller.append(body);
    document.body.append(scroller);
    outline.follow(scroller);
    outline.source(body);
    await nextFrame();
    render(OutlineNav, { outline });
    await fireEvent.click(screen.getByRole('button', { name: 'H2' }));
    expect(scrolled).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(body.querySelectorAll('h2')[2]);
    expect(outline.current).toBe(2);
    scroller.remove();
  });
});
