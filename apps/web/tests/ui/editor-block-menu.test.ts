/**
 * The slash menu's kinds of block (block-menu.ts): found by what is typed of
 * their names, in Korean as it is composed; offered only where they can be
 * done, and what each does keeps the document one the app stores. Runs a
 * real editor in a simulated page, without layout.
 */
import { Editor } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';
import type { SuggestionProps } from '@tiptap/suggestion';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { slashEntries } from '$lib/editor/block-menu.ts';
import { bodyExtensions, type SuggestionEntry } from '$lib/editor/extensions.ts';
import { finds, initialsOf, lettersOf } from '$lib/editor/letters.ts';
import { canOpenBlockFormula, inlineFormulaOf, openBlockFormula } from '$lib/editor/math-nodes.ts';
import { toStoredDoc, validateDoc, type DocNode } from '$lib/editor/schema.ts';
import { matching } from '$lib/editor/slash-menu.ts';
import { suggestionList } from '$lib/editor/suggestion-list.ts';

import {
  bullets,
  callout,
  cell,
  doc,
  item,
  math,
  paragraph,
  quote,
  row,
  table,
  task,
  tasks,
  text,
  toggle,
} from '../support/body-docs.ts';

vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: () => Promise.resolve({ ok: false, reason: 'failed' }),
}));

// Opening a formula's place scrolls it into sight, which asks for the size of text: jsdom has none.
Range.prototype.getClientRects = function getClientRects() {
  return [] as unknown as DOMRectList;
};
Range.prototype.getBoundingClientRect = function getBoundingClientRect() {
  return new DOMRect();
};

let editors: Editor[] = [];

/** An editor of `content` with the caret at the end of the text `at`. */
function create(content: DocNode, at: string): Editor {
  const element = document.createElement('div');
  document.body.append(element);
  const editor = new Editor({ element, injectCSS: false, extensions: bodyExtensions(), content });
  editors.push(editor);
  let end = -1;
  editor.state.doc.descendants((node, pos) => {
    if (end === -1 && node.isText && node.text === at) end = pos + at.length;
  });
  if (end === -1) throw new Error(`no text ${at}`);
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, end)));
  return editor;
}

const stored = (editor: Editor): DocNode => toStoredDoc(editor.getJSON() as DocNode);

afterEach(() => {
  for (const editor of editors) editor.destroy();
  editors = [];
  document.body.replaceChildren();
});

describe('names found by what is typed', () => {
  it('takes Korean apart into the keys that type it', () => {
    expect(lettersOf('제목 1')).toBe('ㅈㅔㅁㅗㄱ1');
    expect(lettersOf('글머리')).toBe('ㄱㅡㄹㅁㅓㄹㅣ');
    // Letters typed as two keys are the two.
    expect(lettersOf('과')).toBe('ㄱㅗㅏ');
    expect(lettersOf('닭')).toBe('ㄷㅏㄹㄱ');
    expect(initialsOf('할 일 목록')).toBe('ㅎㅇㅁㄹ');
  });

  it('finds a name on the way there, as an input method shows it', () => {
    for (const typed of ['ㅈ', '제', '젬', '제모', '제목', 'ㅈㅁ', '제목1', '제목 1']) {
      expect(finds('제목 1', typed), typed).toBe(true);
    }
    expect(finds('할 일 목록', '할일')).toBe(true);
    expect(finds('할 일 목록', 'ㅎㅇ')).toBe(true);
    expect(finds('Heading 1', 'HEAD')).toBe(true);
    expect(finds('Heading 1', 'heading1')).toBe(true);
    expect(finds('제목 1', '목제')).toBe(false);
    expect(finds('제목 1', 'ㅁㅈ')).toBe(false);
    expect(finds('인용', '제')).toBe(false);
  });

  it('narrows the entries by any of their names', () => {
    const editor = create(doc(paragraph(text('a'))), 'a');
    const values = (query: string) =>
      matching(slashEntries(editor, true), query).map((entry) => entry.value);
    expect(values('제')).toEqual(['heading-1', 'heading-2', 'heading-3']);
    expect(values('h2')).toEqual(['heading-2']);
    expect(values('목록')).toEqual(['task-list', 'bullet-list', 'numbered-list']);
    expect(values('ㅅㅅ')).toEqual(['formula']);
    expect(values('이미지')).toEqual(['file']);
    expect(values('$$')).toEqual(['formula']);
    expect(values('quote')).toEqual(['quote']);
    expect(values('구분')).toEqual(['divider']);
    for (const word of ['toggle', 'details', 'collapse', '토글', '접기'])
      expect(values(word), word).toEqual(['toggle']);
    for (const word of ['callout', 'note', 'tip', 'warning', '콜아웃', '알림', '주의'])
      expect(values(word), word).toEqual(['callout']);
  });
});

/** Where the menu is asked for: the caret at the end of the text named. */
const PLACES: Record<string, [DocNode, string]> = {
  'an empty line': [doc(paragraph(text('x')), paragraph()), 'x'],
  'a line with text': [doc(paragraph(text('words'))), 'words'],
  'a list entry': [doc(bullets(item(paragraph(text('one'))))), 'one'],
  'a to-do': [doc(tasks(task(false, paragraph(text('do'))))), 'do'],
  'a quote': [doc(quote(paragraph(text('said')))), 'said'],
  'a cell': [doc(table(row(cell({}, paragraph(text('c'))), cell({}, paragraph())))), 'c'],
  "a toggle's summary": [doc(toggle([text('why')], paragraph(text('because')))), 'why'],
  'what a toggle hides': [doc(toggle([text('why')], paragraph(text('because')))), 'because'],
  'a callout': [doc(callout('tip', paragraph(text('mind')))), 'mind'],
};

/** A place by its name. */
function placeNamed(name: string): [DocNode, string] {
  const place = PLACES[name];
  if (!place) throw new Error(`no place ${name}`);
  return place;
}

describe('the kinds of block', () => {
  it('are offered in their order, the table after them, the file last', () => {
    const editor = create(...placeNamed('a line with text'));
    expect(slashEntries(editor, true).map((entry) => entry.value)).toEqual([
      'heading-1',
      'heading-2',
      'heading-3',
      'task-list',
      'bullet-list',
      'numbered-list',
      'quote',
      'toggle',
      'callout',
      'code-block',
      'formula',
      'divider',
      'table',
      'file',
    ]);
    expect(slashEntries(editor, false).map((entry) => entry.value)).not.toContain('file');
  });

  it('offer no heading as the first line of a list entry, and change a table first in it', () => {
    const listed = create(...placeNamed('a list entry'));
    const offered = slashEntries(listed, false).map((entry) => entry.value);
    expect(offered.filter((value) => value.startsWith('heading'))).toEqual([]);
    expect(offered).not.toContain('table');
    // Nor a toggle or a callout, which stand where a table does.
    expect(offered).not.toContain('toggle');
    expect(offered).not.toContain('callout');
    const inCallout = slashEntries(create(...placeNamed('a callout')), false);
    expect(inCallout.map((entry) => entry.value)).not.toContain('table');
    const hidden = slashEntries(create(...placeNamed('what a toggle hides')), false);
    expect(hidden.map((entry) => entry.value)).toContain('table');

    const inCell = create(...placeNamed('a cell'));
    const first = slashEntries(inCell, false)[0]?.value;
    expect(first).not.toBe('heading-1');
  });

  for (const [place, [content, at]] of Object.entries(PLACES)) {
    it(`keep the document one the app stores, each where it is offered: ${place}`, () => {
      const probe = create(content, at);
      const offered = slashEntries(probe, false).map((entry) => entry.value);
      expect(offered.length).toBeGreaterThan(0);
      for (const value of offered) {
        const editor = create(content, at);
        const entry = slashEntries(editor, false).find((found) => found.value === value);
        entry?.run(editor);
        expect(validateDoc(stored(editor)), `${place}: ${value}`).toEqual([]);
      }
    });
  }
});

describe('a formula from the menu and the bubble', () => {
  it('opens a new block formula on its empty line, or on a new line after the caret', () => {
    const empty = create(...placeNamed('an empty line'));
    empty.commands.setTextSelection(empty.state.doc.content.size - 1);
    expect(canOpenBlockFormula(empty.state)).toBe(true);
    expect(openBlockFormula(empty)).toBe(true);
    expect(document.querySelector('.doc-place-new')?.closest('p')).not.toBeNull();
    // Nothing is stored for a formula that has no source yet.
    expect(stored(empty)).toEqual(doc(paragraph(text('x')), paragraph()));

    const written = create(...placeNamed('a line with text'));
    expect(openBlockFormula(written)).toBe(true);
    expect(written.state.doc.childCount).toBe(2);
    expect(written.state.doc.child(0).textContent).toBe('words');
  });

  it('makes the chosen text of one line a formula, and nothing else', () => {
    const editor = create(doc(paragraph(text('so a^2 + b^2 holds'))), 'so a^2 + b^2 holds');
    editor.commands.setTextSelection({ from: 4, to: 13 });
    const made = inlineFormulaOf(editor.state);
    expect(made).not.toBeNull();
    if (made) editor.view.dispatch(made);
    expect(stored(editor)).toEqual(doc(paragraph(text('so '), math('a^2 + b^2'), text(' holds'))));

    const none = create(doc(paragraph(text('one')), paragraph(text('two'))), 'two');
    none.commands.setTextSelection({ from: 2, to: 8 });
    expect(inlineFormulaOf(none.state)).toBeNull();
    none.commands.setTextSelection(3);
    expect(inlineFormulaOf(none.state)).toBeNull();
  });
});

describe('the menu under an input method', () => {
  it('leaves a key that is composing to the input method', () => {
    const editor = create(...placeNamed('an empty line'));
    const entries = slashEntries(editor, false);
    const choose = vi.fn();
    const list = suggestionList({ kind: 'slash', label: () => 'Add a block' })();
    list.onStart?.({
      editor,
      items: entries,
      command: choose,
      clientRect: () => new DOMRect(),
    } as unknown as SuggestionProps<SuggestionEntry, SuggestionEntry>);
    const key = (init: KeyboardEventInit): boolean =>
      list.onKeyDown?.({
        view: editor.view,
        event: new KeyboardEvent('keydown', { cancelable: true, ...init }),
        range: { from: 0, to: 0 },
      }) ?? false;
    // The Enter that ends a syllable, and the arrows of a list of candidates, are the input method's.
    expect(key({ key: 'Enter', isComposing: true })).toBe(false);
    expect(key({ key: 'ArrowDown', isComposing: true })).toBe(false);
    expect(choose).not.toHaveBeenCalled();
    expect(key({ key: 'ArrowDown' })).toBe(true);
    expect(key({ key: 'Enter' })).toBe(true);
    expect(choose).toHaveBeenCalledWith(entries[1]);
    list.onExit?.({} as SuggestionProps<SuggestionEntry, SuggestionEntry>);
    expect(document.querySelector('[data-suggestions]')).toBeNull();
  });

  it('takes an entry pressed while a syllable is composed once it is written', () => {
    vi.useFakeTimers();
    const editor = create(...placeNamed('an empty line'));
    let composing = true;
    Object.defineProperty(editor.view, 'composing', { get: () => composing });
    const entries = slashEntries(editor, false);
    const choose = vi.fn();
    const list = suggestionList({ kind: 'slash', label: () => 'Add a block' })();
    list.onStart?.({
      editor,
      items: entries,
      command: choose,
      clientRect: () => new DOMRect(),
    } as unknown as SuggestionProps<SuggestionEntry, SuggestionEntry>);
    const option = document.querySelectorAll('[data-suggestions] [role="option"]')[1];
    option?.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    expect(choose).not.toHaveBeenCalled();
    composing = false;
    editor.view.dom.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
    vi.advanceTimersByTime(30);
    expect(choose).toHaveBeenCalledWith(entries[1]);
    list.onExit?.({} as SuggestionProps<SuggestionEntry, SuggestionEntry>);
    vi.useRealTimers();
  });
});
