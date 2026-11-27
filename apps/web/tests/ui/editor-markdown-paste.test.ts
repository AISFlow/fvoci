/**
 * Markdown pasted into a body as text: what is taken for Markdown, what it
 * becomes, and what stays text (one line, plain text, a code block).
 */
import { Editor } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';
import { afterEach, describe, expect, it } from 'vitest';

import { bodyExtensions } from '$lib/editor/extensions.ts';
import { looksLikeMarkdown, pastedMarkdown } from '$lib/editor/markdown-paste.ts';
import { toStoredDoc, validateDoc, type DocNode } from '$lib/editor/schema.ts';

const editors: Editor[] = [];

function create(content: DocNode): Editor {
  const made = new Editor({
    element: document.createElement('div'),
    extensions: bodyExtensions(),
    content,
  });
  editors.push(made);
  return made;
}

afterEach(() => {
  for (const made of editors.splice(0)) made.destroy();
});

const stored = (editor: Editor): DocNode => toStoredDoc(editor.getJSON() as DocNode);
const doc = (...content: DocNode[]): DocNode => ({ type: 'doc', content });
const paragraph = (text = ''): DocNode =>
  text === '' ? { type: 'paragraph' } : { type: 'paragraph', content: [{ type: 'text', text }] };

/** A paste of text alone, as the page hands it over (it has no clipboard of its own). */
function pasteText(editor: Editor, text: string): void {
  const event = new Event('paste', { bubbles: true, cancelable: true });
  const clipboardData = {
    getData: (type: string) => (type === 'text/plain' ? text : ''),
    types: ['text/plain'],
    files: [],
    items: [],
  };
  Object.defineProperty(event, 'clipboardData', { value: clipboardData });
  editor.view.dom.dispatchEvent(event);
}

/** The types of the blocks of the document, top level. */
const blocks = (editor: Editor): string[] =>
  (stored(editor).content ?? []).map((node) => node.type);

function atEnd(editor: Editor): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.atEnd(editor.state.doc)));
}

describe('what is taken for Markdown', () => {
  it.each([
    ['a heading', '## Plan\nShip it'],
    ['a heading of level six', '###### Deep\nbelow'],
    ['a fence of code', 'Run this:\n```sh\nmake\n```'],
    ['a fence of tildes', '~~~\ncode\n~~~'],
    ['an open task', 'Today\n- [ ] write'],
    ['a ticked task', '- [x] done\nnext'],
    ['two entries of a list', '- one\n- two'],
    ['two numbered entries', '1. one\n2) two'],
    ['a quote', 'They said:\n> not yet'],
  ])('%s', (_kind, text) => {
    expect(looksLikeMarkdown(text)).toBe(true);
  });

  it.each([
    ['one line, even a heading', '# 3 people'],
    ['one line of a task', '- [ ] alone'],
    ['one entry of a list among lines', '- one\nand more'],
    ['a hash without a space', '#tag\n#another'],
    ['seven hashes', '####### no\nheading'],
    ['plain lines', 'Dear all,\nsee you at 3.'],
    ['a quote mark without a space', '>not\n>quoted'],
    ['dates as Korean writes them', '2026. 11. 26. 주간 회의\n2026. 11. 27. 워크숍'],
    ['dates without the year', '11. 26. 주간 회의\n11. 27. 워크숍'],
  ])('not %s', (_kind, text) => {
    expect(looksLikeMarkdown(text)).toBe(false);
  });
});

describe('Markdown pasted as text', () => {
  it('becomes headings, tasks, lists, quotes and code, all of which the body keeps', () => {
    const editor = create(doc(paragraph()));
    atEnd(editor);
    pasteText(
      editor,
      [
        '## Plan',
        '- [ ] write',
        '- [x] read',
        '',
        '1. first',
        '2. second',
        '',
        '> a quote',
        '',
        '```ts',
        'const a = 1;',
        '```',
      ].join('\n'),
    );
    expect(blocks(editor)).toEqual([
      'heading',
      'taskList',
      'orderedList',
      'blockquote',
      'codeBlock',
    ]);
    expect(stored(editor).content?.[0]).toMatchObject({ attrs: { level: 2 } });
    expect(editor.state.doc.child(4).textContent).toBe('const a = 1;');
    expect(validateDoc(stored(editor))).toEqual([]);
  });

  it('a single line stays text, as it was typed', () => {
    const editor = create(doc(paragraph()));
    atEnd(editor);
    pasteText(editor, '# 3 people came');
    expect(blocks(editor)).toEqual(['paragraph']);
    expect(editor.getText()).toBe('# 3 people came');
  });

  it('stays text when pasted as plain text', () => {
    const editor = create(doc(paragraph()));
    atEnd(editor);
    // What ProseMirror does for Ctrl+Shift+V (the page has no ClipboardEvent of its own).
    editor.view.pasteText('## Plan\n- [ ] write', new Event('paste') as ClipboardEvent);
    expect(blocks(editor)).toEqual(['paragraph', 'paragraph']);
    expect(editor.getText({ blockSeparator: '\n' })).toBe('## Plan\n- [ ] write');
  });

  it('stays text in a code block', () => {
    const editor = create(doc({ type: 'codeBlock', attrs: { language: null } }));
    atEnd(editor);
    pasteText(editor, '## Plan\n- [ ] write');
    expect(blocks(editor)).toEqual(['codeBlock']);
    expect(editor.state.doc.firstChild?.textContent).toBe('## Plan\n- [ ] write');
  });

  it('keeps a line that starts with a date text, among lines that are Markdown', () => {
    const editor = create(doc(paragraph()));
    atEnd(editor);
    pasteText(editor, '## 일정\n\n2026. 11. 26. 주간 회의\n\n2026. 11. 27. 워크숍');
    expect(blocks(editor)).toEqual(['heading', 'paragraph', 'paragraph']);
    expect(editor.state.doc.child(1).textContent).toBe('2026. 11. 26. 주간 회의');
    expect(editor.state.doc.child(2).textContent).toBe('2026. 11. 27. 워크숍');
  });

  it('tells a table by its lines in one pass, however long they are', () => {
    const editor = create(doc(paragraph()));
    const started = performance.now();
    // A rule, many spaces and a letter: no line under a table's head, and no Markdown.
    expect(pastedMarkdown(`a | b\n-${' '.repeat(200_000)}x\nc`, editor.schema)).toBeUndefined();
    expect(performance.now() - started).toBeLessThan(1000);
  });

  it('text that only looks like a table over a rule stays text', () => {
    const editor = create(doc(paragraph()));
    atEnd(editor);
    pasteText(editor, 'a | b\n---');
    expect(blocks(editor).includes('heading')).toBe(false);
    expect(blocks(editor).includes('table')).toBe(false);
  });
});
