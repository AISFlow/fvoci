/**
 * Board views in the editor of a body and where a body is read: where one
 * may be put (in the document and in a toggle; not in a list, a quote, a
 * callout or a cell; ten at most, however the reader's change was made), what
 * draws it, a body that may have none
 * (a comment's, an entry's), the slash menu's entry, and a pasted address
 * of a view: a link, offered as a board view once the reader is known to
 * see it. Runs a real editor in a simulated page, without layout.
 */
import { Editor } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';
import { ySyncPluginKey } from '@tiptap/y-tiptap';
import type { EditorView } from '@tiptap/pm/view';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { slashEntries } from '$lib/editor/block-menu.ts';
import { mountBoardViews, type MountBoardView } from '$lib/editor/board-view-read.ts';
import { boardViewCount, boardViewEntry, type BoardViewOptions } from '$lib/editor/board-view.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { renderBody } from '$lib/editor/render.ts';
import { MAX_BOARD_VIEWS, toStoredDoc, validateDoc, type DocNode } from '$lib/editor/schema.ts';
import type { ChoiceList } from '$lib/editor/suggestion-list.ts';
import type { SuggestionEntry } from '$lib/editor/extensions.ts';

import {
  bullets,
  callout,
  cell,
  doc,
  item,
  paragraph,
  quote,
  row,
  table,
  text,
  toggle,
} from '../support/body-docs.ts';

const VIEW = '01930000-0000-7000-8000-0000000b0001';
const OTHER = '01930000-0000-7000-8000-0000000b0002';
const ADDRESS = `https://fvoci.test/w/ws/boards/board/${VIEW}`;

const boardView = (viewId: string): DocNode => ({ type: 'boardView', attrs: { viewId } });

const editors: Editor[] = [];

function create(content: DocNode, boardViewOptions: BoardViewOptions = {}): Editor {
  const made = new Editor({
    element: document.createElement('div'),
    extensions: bodyExtensions({ history: true, boardView: boardViewOptions }),
    content,
  });
  editors.push(made);
  return made;
}

afterEach(() => {
  for (const made of editors.splice(0)) made.destroy();
});

const stored = (editor: Editor): DocNode => toStoredDoc(editor.getJSON() as DocNode);

/** The position just inside the text `words`, at its end. */
function after(editor: Editor, words: string): number {
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (found < 0 && node.isText && node.text?.includes(words))
      found = pos + node.text.indexOf(words) + words.length;
    return found < 0;
  });
  if (found < 0) throw new Error(`no ${words}`);
  return found;
}

function caret(editor: Editor, at: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, at)));
}

describe('where a board view goes', () => {
  it('takes the place of the empty line the caret is on, and selects itself', () => {
    const editor = create(doc(paragraph(text('before')), paragraph()));
    caret(editor, after(editor, 'before') + 2);
    expect(editor.commands.setBoardView(VIEW)).toBe(true);
    expect(stored(editor)).toEqual(doc(paragraph(text('before')), boardView(VIEW)));
    expect(editor.state.selection.toJSON()).toMatchObject({ type: 'node' });
  });

  it('goes after the line of text the caret is in, and in what a toggle hides', () => {
    const editor = create(doc(toggle([text('summary')], paragraph(text('inside')))));
    caret(editor, after(editor, 'inside'));
    expect(editor.commands.setBoardView(VIEW)).toBe(true);
    expect(stored(editor)).toEqual(
      doc(toggle([text('summary')], paragraph(text('inside')), boardView(VIEW))),
    );
    expect(validateDoc(stored(editor))).toEqual([]);
  });

  it.each([
    ['a list', doc(bullets(item(paragraph(text('words')))))],
    ['a quote', doc(quote(paragraph(text('words'))))],
    ['a callout', doc(callout('info', paragraph(text('words'))))],
    ['a cell', doc(table(row(cell({}, paragraph(text('words'))))))],
    ['the summary of a toggle', doc(toggle([text('words')], paragraph()))],
  ])('is not put in %s', (_where, content) => {
    const editor = create(content);
    caret(editor, after(editor, 'words'));
    const before = stored(editor);
    expect(editor.commands.setBoardView(VIEW)).toBe(false);
    expect(stored(editor)).toEqual(before);
    expect(
      slashEntries(
        editor,
        false,
        boardViewEntry(async () => VIEW),
      ),
    ).not.toContainEqual(expect.objectContaining({ value: 'board-view' }));
  });

  it(`is not put in a body that shows ${MAX_BOARD_VIEWS}`, () => {
    const ten = Array.from({ length: MAX_BOARD_VIEWS }, () => boardView(VIEW));
    const editor = create(doc(...ten, paragraph(text('last'))));
    caret(editor, after(editor, 'last'));
    expect(editor.commands.setBoardView(OTHER)).toBe(false);
    expect(validateDoc(doc(...ten, boardView(OTHER)))).toEqual([
      `more than ${MAX_BOARD_VIEWS} board views`,
    ]);
  });

  it(`goes past ${MAX_BOARD_VIEWS} by no change of the reader's, but by another's`, () => {
    const ten = Array.from({ length: MAX_BOARD_VIEWS }, () => boardView(VIEW));
    const editor = create(doc(...ten, paragraph(text('last'))));
    const node = editor.schema.nodeFromJSON(boardView(OTHER));
    const end = editor.state.doc.content.size;
    // A block pasted, dropped or put in by any command of the reader's.
    editor.view.dispatch(editor.state.tr.insert(end, node));
    editor.commands.insertContentAt(end, boardView(OTHER));
    expect(boardViewCount(editor.state.doc)).toBe(MAX_BOARD_VIEWS);
    // Another's change is the shared document's.
    editor.view.dispatch(
      editor.state.tr.insert(end, node).setMeta(ySyncPluginKey, { isChangeOrigin: true }),
    );
    expect(boardViewCount(editor.state.doc)).toBe(MAX_BOARD_VIEWS + 1);
    // A body past them still takes what the reader types.
    caret(editor, after(editor, 'last'));
    editor.commands.insertContent('!');
    expect(editor.state.doc.textContent).toContain('last!');
  });

  it('is offered by the slash menu, which asks for the view and puts it in', async () => {
    const editor = create(doc(paragraph(text('words'))));
    caret(editor, after(editor, 'words'));
    let chosen: (viewId: string | undefined) => void = () => undefined;
    const choose = () =>
      new Promise<string | undefined>((resolve) => {
        chosen = resolve;
      });
    const entry = slashEntries(editor, false, boardViewEntry(choose)).find(
      (one) => one.value === 'board-view',
    );
    expect(entry?.words).toEqual(expect.arrayContaining(['embed', '임베드', '보드']));
    expect(slashEntries(editor, false).some((one) => one.value === 'board-view')).toBe(false);
    entry?.run(editor);
    chosen(VIEW);
    await Promise.resolve();
    await Promise.resolve();
    expect(stored(editor)).toEqual(doc(paragraph(text('words')), boardView(VIEW)));
  });
});

describe('what draws a board view', () => {
  it('draws the view into its place in the editor, and stops when the block goes', () => {
    const stop = vi.fn();
    const mount = vi.fn<MountBoardView>((place, viewId) => {
      place.textContent = `view ${viewId}`;
      return stop;
    });
    const editor = create(doc(boardView(VIEW), paragraph(text('after'))), { mount });
    expect(mount).toHaveBeenCalledOnce();
    const place = editor.view.dom.querySelector('.doc-board-view-place');
    expect(place?.textContent).toBe(`view ${VIEW}`);
    expect(place?.closest('[data-board-view]')?.getAttribute('data-view-id')).toBe(VIEW);
    editor.commands.setContent(doc(paragraph(text('after'))));
    expect(stop).toHaveBeenCalledOnce();
  });

  it('says that the view cannot be seen without what draws it', () => {
    const editor = create(doc(boardView(VIEW)));
    expect(editor.view.dom.querySelector('.doc-board-view-unseen')).not.toBeNull();
  });

  it('draws the views of a body read, and one emptied for a reader by link says it is unseen', () => {
    const root = document.createElement('div');
    const rendered = renderBody(doc(boardView(VIEW), paragraph(text('between')), boardView('')));
    if (!rendered.ok) throw new Error('not drawn');
    root.append(rendered.fragment);
    const mount = vi.fn<MountBoardView>((place, viewId) => {
      place.dataset.drawn = viewId;
      return () => {
        delete place.dataset.drawn;
      };
    });
    const stop = mountBoardViews(root, mount);
    const places = [...root.querySelectorAll<HTMLElement>('[data-board-view]')];
    expect(places.map((place) => place.dataset.drawn)).toEqual([VIEW, undefined]);
    expect(places[1]?.querySelector('.doc-board-view-unseen')).not.toBeNull();
    stop();
    expect(places[0]?.dataset.drawn).toBeUndefined();
  });
});

describe('a body that may have none', () => {
  it('makes no board view of a pasted one, and offers none', () => {
    const editor = create(doc(paragraph(text('words'))), { allowed: false });
    caret(editor, after(editor, 'words'));
    editor.commands.insertContent(
      `<div data-board-view="" data-view-id="${VIEW}"></div><p>pasted</p>`,
    );
    expect(JSON.stringify(stored(editor))).not.toContain('boardView');
    expect(slashEntries(editor, false).some((one) => one.value === 'board-view')).toBe(false);
  });
});

/** A list that remembers what it offered and lets the test choose. */
function fakeList() {
  const shown: { entries: SuggestionEntry[]; choose: (entry: SuggestionEntry) => void }[] = [];
  const closed = vi.fn();
  const list: ChoiceList = {
    open: (_view: EditorView, entries, choose) => {
      shown.push({ entries, choose });
    },
    keyDown: (event) => event.key === 'Escape',
    close: closed,
  };
  return { shown, closed, choices: () => list };
}

/** Chooses, in the list offered first, the entry `value`. */
function chooseOffered(
  shown: { entries: SuggestionEntry[]; choose: (entry: SuggestionEntry) => void }[],
  value: string,
): void {
  const [offered] = shown;
  const entry = offered?.entries.find((one) => one.value === value);
  if (!offered || !entry) throw new Error(`nothing offered as ${value}`);
  offered.choose(entry);
}

/** A paste of plain text, as the page hands it over. */
function pasteText(editor: Editor, words: string): void {
  const event = new Event('paste', { bubbles: true, cancelable: true });
  const clipboardData = {
    getData: (type: string) => (type === 'text/plain' ? words : ''),
    types: ['text/plain'],
    files: [],
    items: [],
  };
  Object.defineProperty(event, 'clipboardData', { value: clipboardData });
  editor.view.dom.dispatchEvent(event);
}

const settle = async (): Promise<void> => {
  for (let i = 0; i < 4; i += 1) await Promise.resolve();
};

describe('a pasted address of a view', () => {
  beforeEach(() => {
    // The page has one; the simulated page has none, and a paste of text makes one.
    vi.stubGlobal('ClipboardEvent', class extends Event {});
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function pasting(seen = true, content = doc(paragraph(text('see ')))) {
    const list = fakeList();
    const editor = create(content, {
      viewAt: (address) => (address === ADDRESS ? VIEW : undefined),
      seen: async () => seen,
      choices: list.choices,
    });
    // jsdom draws nothing: the list is placed at no particular place.
    editor.view.coordsAtPos = () => ({ left: 0, right: 0, top: 0, bottom: 0 });
    caret(editor, after(editor, 'see '));
    return { editor, list };
  }

  it('is a link until it is taken as a board view, which takes its place', async () => {
    const { editor, list } = pasting();
    pasteText(editor, ADDRESS);
    expect(editor.state.doc.textContent).toBe(`see ${ADDRESS}`);
    await settle();
    expect(list.shown.map(({ entries }) => entries.map((entry) => entry.value))).toEqual([
      ['link', 'embed'],
    ]);
    chooseOffered(list.shown, 'embed');
    expect(stored(editor)).toEqual(doc(paragraph(text('see ')), boardView(VIEW)));
    expect(list.closed).toHaveBeenCalled();
  });

  it('stays a link when so taken, when Escape is pressed, or when anything is typed', async () => {
    const kept = pasting();
    pasteText(kept.editor, ADDRESS);
    await settle();
    chooseOffered(kept.list.shown, 'link');
    expect(kept.editor.state.doc.textContent).toBe(`see ${ADDRESS}`);

    const escaped = pasting();
    pasteText(escaped.editor, ADDRESS);
    await settle();
    expect(escaped.list.shown).toHaveLength(1);
    const taken = escaped.editor.view.someProp('handleKeyDown', (handle) =>
      handle(escaped.editor.view, new KeyboardEvent('keydown', { key: 'Escape' })),
    );
    expect(taken).toBe(true);
    expect(escaped.list.closed).toHaveBeenCalled();
    expect(escaped.editor.state.doc.textContent).toBe(`see ${ADDRESS}`);

    const typed = pasting();
    pasteText(typed.editor, ADDRESS);
    await settle();
    typed.editor.commands.insertContent('!');
    expect(typed.list.closed).toHaveBeenCalled();
    // A choice made once the list is gone changes nothing.
    chooseOffered(typed.list.shown, 'embed');
    expect(JSON.stringify(stored(typed.editor))).not.toContain('boardView');
  });

  it('stays a link when taken as a board view in a body another filled meanwhile', async () => {
    const nine = Array.from({ length: MAX_BOARD_VIEWS - 1 }, () => boardView(VIEW));
    const { editor, list } = pasting(true, doc(paragraph(text('see ')), ...nine));
    pasteText(editor, ADDRESS);
    await settle();
    expect(list.shown).toHaveLength(1);
    const node = editor.schema.nodeFromJSON(boardView(OTHER));
    editor.view.dispatch(
      editor.state.tr
        .insert(editor.state.doc.content.size, node)
        .setMeta(ySyncPluginKey, { isChangeOrigin: true }),
    );
    expect(boardViewCount(editor.state.doc)).toBe(MAX_BOARD_VIEWS);
    chooseOffered(list.shown, 'embed');
    expect(editor.state.doc.textContent).toBe(`see ${ADDRESS}`);
    expect(boardViewCount(editor.state.doc)).toBe(MAX_BOARD_VIEWS);
  });

  it('is not offered as a board view of a view the reader does not see', async () => {
    const { editor, list } = pasting(false);
    pasteText(editor, ADDRESS);
    await settle();
    expect(editor.state.doc.textContent).toBe(`see ${ADDRESS}`);
    expect(list.shown).toEqual([]);
  });

  it('is pasted as any text where a board view may not stand, or for another address', async () => {
    const inList = pasting(true, doc(bullets(item(paragraph(text('see '))))));
    pasteText(inList.editor, ADDRESS);
    const other = pasting();
    pasteText(other.editor, 'https://fvoci.test/w/ws/wiki/page');
    await settle();
    expect(inList.list.shown).toEqual([]);
    expect(other.list.shown).toEqual([]);
  });
});
