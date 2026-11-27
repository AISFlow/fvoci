/**
 * Formulas and the emoji list in the editor, in a simulated page: what
 * typing turns into, what pasted markup becomes, and when the list of emoji
 * is asked for. Runs a real editor without layout. Diagrams are in
 * editor-diagrams.test.ts.
 */
import { Editor } from '@tiptap/core';
import { NodeSelection, Selection } from '@tiptap/pm/state';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { bodyExtensions, type SuggestionEntry } from '$lib/editor/extensions.ts';
import { MAX_LATEX, toStoredDoc, validateDoc, type DocNode } from '$lib/editor/schema.ts';

import { doc, math, mathBlock, mention, paragraph, text } from '../support/body-docs.ts';

// The editor draws a diagram under its source; the library for that needs a browser.
const drawDiagram = vi.fn((_source: string) => Promise.resolve({ ok: false, reason: 'failed' }));
vi.mock('$lib/editor/diagram.ts', () => ({
  drawDiagram: (source: string) => drawDiagram(source),
}));

let editor: Editor | undefined;
const search = vi.fn((_query: string): SuggestionEntry[] => [{ value: '😄', label: '😄 smile' }]);
const started = vi.fn();

function create(content?: DocNode | string): Editor {
  const element = document.createElement('div');
  document.body.append(element);
  editor = new Editor({
    element,
    injectCSS: false,
    extensions: bodyExtensions({
      // The list is told when it opens and whenever its entries change; both are kept.
      emoji: { search, render: () => ({ onStart: started, onUpdate: started }) },
    }),
    ...(content === undefined ? {} : { content }),
  });
  return editor;
}

const stored = (target: Editor): DocNode => toStoredDoc(target.getJSON() as DocNode);

/** Puts the cursor at the end, without the scrolling a simulated page cannot do. */
function toEnd(target: Editor): void {
  target.view.dispatch(target.state.tr.setSelection(Selection.atEnd(target.state.doc)));
}

/** Types text the way a keyboard does: one character at a time, through the editor's input handling. */
function type(target: Editor, value: string): void {
  for (const character of value) {
    const { from, to } = target.state.selection;
    const insert = () => target.state.tr.insertText(character, from, to);
    const handled = target.view.someProp('handleTextInput', (handle) =>
      handle(target.view, from, to, character, insert),
    );
    if (!handled) target.view.dispatch(insert());
  }
}

afterEach(() => {
  editor?.destroy();
  editor = undefined;
  document.body.replaceChildren();
  search.mockClear();
  started.mockClear();
  drawDiagram.mockClear();
});

describe('typing a formula', () => {
  it('turns `$…$` into a formula in the line when the closing dollar is typed', () => {
    const target = create();
    type(target, 'energy $E = mc^2');
    expect(stored(target)).toEqual(doc(paragraph(text('energy $E = mc^2'))));
    type(target, '$');
    expect(stored(target)).toEqual(doc(paragraph(text('energy '), math('E = mc^2'))));
    type(target, ' and on');
    expect(stored(target)).toEqual(
      doc(paragraph(text('energy '), math('E = mc^2'), text(' and on'))),
    );
    expect(validateDoc(stored(target))).toEqual([]);
  });

  it.each([
    ['한글 뒤 $x$', doc(paragraph(text('한글 뒤 '), math('x')))],
    ['($a+b$', doc(paragraph(text('('), math('a+b')))],
    ['$\\frac{1}{2}$', doc(paragraph(math('\\frac{1}{2}')))],
    ['$a$ $b$', doc(paragraph(math('a'), text(' '), math('b')))],
  ])('%j becomes a formula', (typed, expected) => {
    const target = create();
    type(target, typed);
    expect(stored(target)).toEqual(expected);
  });

  it.each([
    // Money: the dollar that would close has a space in front of it.
    ['$5 and $'],
    ['$5 and $10'],
    ['costs 5$ or 6$'],
    // A dollar inside a word, an escaped one, an empty pair, a space after the opening one.
    ['US$5$'],
    ['\\$x$'],
    ['$$'],
    ['$ x$'],
    ['$x $'],
  ])('%j stays text', (typed) => {
    const target = create();
    type(target, typed);
    expect(stored(target)).toEqual(doc(paragraph(text(typed))));
  });

  it('does not make a formula inside code', () => {
    const target = create(doc({ type: 'codeBlock', content: [text('x')] }));
    toEnd(target);
    type(target, ' $a$');
    expect(stored(target)).toEqual(doc({ type: 'codeBlock', content: [text('x $a$')] }));
  });

  it('turns `$$…$$` alone on a line into a block formula, with a line to go on writing', () => {
    const target = create();
    type(target, '$$\\int_0^1 x \\, dx$');
    expect(stored(target).content?.[0]?.type).toBe('paragraph');
    type(target, '$');
    expect(stored(target)).toEqual(doc(mathBlock('\\int_0^1 x \\, dx'), paragraph()));
    type(target, 'next');
    expect(stored(target)).toEqual(doc(mathBlock('\\int_0^1 x \\, dx'), paragraph(text('next'))));
    expect(validateDoc(stored(target))).toEqual([]);
  });

  it('leaves `$$…$$` as text where a block formula cannot go', () => {
    const first = create();
    type(first, 'see $$x$$');
    expect(stored(first).content?.every((node) => node.type === 'paragraph')).toBe(true);

    // The first block of a list entry has to stay a paragraph.
    const list = create(
      doc({ type: 'bulletList', content: [{ type: 'listItem', content: [paragraph()] }] }),
    );
    toEnd(list);
    type(list, '$$x$$');
    expect(validateDoc(stored(list))).toEqual([]);
    expect(JSON.stringify(stored(list))).not.toContain('blockMath');
  });
  // `$$` and a space opens the place of a new block formula: editor-formula-blocks.test.ts.
});

// A block formula is changed in its place under it: editor-formula-blocks.test.ts.
describe('changing a formula in a line', () => {
  const open = (target: Editor, position: number): HTMLTextAreaElement => {
    target.view.dispatch(
      target.state.tr.setSelection(NodeSelection.create(target.state.doc, position)),
    );
    target.view.someProp('handleKeyDown', (handle) =>
      handle(target.view, new KeyboardEvent('keydown', { key: 'Enter' })),
    );
    const field = document.querySelector<HTMLTextAreaElement>('.doc-math-editing textarea');
    if (!field) throw new Error('the box did not open');
    return field;
  };
  const press = (field: HTMLTextAreaElement, init: KeyboardEventInit): void => {
    field.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...init }));
  };

  it('opens on Enter with the source, and applies a new one', () => {
    const target = create(doc(paragraph(text('a '), math('x^2'), text(' b'))));
    const field = open(target, 3);
    expect(field.value).toBe('x^2');
    field.value = '  y^3  ';
    press(field, { key: 'Enter' });
    expect(stored(target)).toEqual(doc(paragraph(text('a '), math('y^3'), text(' b'))));
    expect(document.querySelector('.doc-math-editing')).toBeNull();
  });

  it('opens on a press, too', () => {
    const target = create(doc(paragraph(math('x'))));
    target.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    expect(document.querySelector<HTMLTextAreaElement>('.doc-math-editing textarea')?.value).toBe(
      'x',
    );
  });

  it('removes the formula when the source is emptied, by hand or with the button', () => {
    const target = create(doc(paragraph(text('a '), math('x'), text(' b'))));
    const field = open(target, 3);
    field.value = '   ';
    press(field, { key: 'Enter' });
    expect(stored(target)).toEqual(doc(paragraph(text('a  b'))));

    const second = create(doc(paragraph(math('x')), paragraph(text('after'))));
    open(second, 1);
    document
      .querySelector<HTMLButtonElement>('.doc-math-editing button[data-action="remove"]')
      ?.click();
    expect(stored(second)).toEqual(doc(paragraph(), paragraph(text('after'))));
  });

  it('changes nothing on Escape, and nothing while an input method is composing', () => {
    const target = create(doc(paragraph(math('x'))));
    const field = open(target, 1);
    field.value = 'changed';
    // While an input method is composing, no key is a command.
    press(field, { key: 'Enter', isComposing: true });
    press(field, { key: 'Escape', isComposing: true });
    expect(document.querySelector('.doc-math-editing')).not.toBeNull();
    press(field, { key: 'Escape' });
    expect(document.querySelector('.doc-math-editing')).toBeNull();
    expect(stored(target)).toEqual(doc(paragraph(math('x'))));
  });

  it('applies what was written when something else is pressed', () => {
    const target = create(doc(paragraph(math('x'))));
    const field = open(target, 1);
    field.value = 'kept';
    document.body.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    expect(stored(target)).toEqual(doc(paragraph(math('kept'))));
  });

  it('does not open when the editor is read-only', () => {
    const target = create(doc(paragraph(math('x'))));
    target.setEditable(false);
    target.view.dom.querySelector<HTMLElement>('.doc-math')?.click();
    expect(document.querySelector('.doc-math-editing')).toBeNull();
  });

  it('never keeps a source longer than the limit', () => {
    const target = create(doc(paragraph(math('x'))));
    const field = open(target, 1);
    field.value = 'y'.repeat(MAX_LATEX + 50);
    press(field, { key: 'Enter' });
    expect(stored(target)).toEqual(doc(paragraph(math('y'.repeat(MAX_LATEX)))));
    expect(validateDoc(stored(target))).toEqual([]);
  });
});

describe('formulas from elsewhere', () => {
  const load = (html: string): DocNode => stored(create(html));

  it('reads a formula from markup that names its source, as this editor and its library’s own write it', () => {
    expect(
      load(
        '<p>a <span data-type="inline-math" data-latex="x^2" class="doc-math"><span class="katex">drawn</span></span> b</p>' +
          '<div data-type="block-math" data-latex="\\frac{1}{2}">ignored</div>',
      ),
    ).toEqual(doc(paragraph(text('a '), math('x^2'), text(' b')), mathBlock('\\frac{1}{2}')));
  });

  it('takes nothing but the source from it', () => {
    const body = load(
      '<p><span data-type="inline-math" data-latex="x" onclick="steal()" style="color:red" data-display="yes" id="app">' +
        '<img src="https://evil.example/p.png"><script>alert(1)</script></span></p>',
    );
    expect(body).toEqual(doc(paragraph(math('x'))));
    expect(validateDoc(body)).toEqual([]);
  });

  it('does not take a formula without a source or with one that is too long', () => {
    const body = load(
      '<p>a<span data-type="inline-math">none</span><span data-type="inline-math" data-latex="">empty</span></p>' +
        `<div data-type="block-math" data-latex="${'x'.repeat(MAX_LATEX + 1)}">long</div><p>b</p>`,
    );
    expect(validateDoc(body)).toEqual([]);
    expect(JSON.stringify(body)).not.toMatch(/inlineMath|blockMath/);
  });

  it('writes a formula out as its source, for the clipboard', () => {
    const target = create(doc(paragraph(math('a<b & "c"')), mathBlock('x')));
    const html = target.getHTML();
    const read = document.createElement('div');
    read.innerHTML = html;
    const [inline, block] = Array.from(read.querySelectorAll('.doc-math'));
    expect(inline?.tagName).toBe('SPAN');
    expect(inline?.getAttribute('data-latex')).toBe('a<b & "c"');
    expect(inline?.textContent).toBe('a<b & "c"');
    expect(block?.tagName).toBe('DIV');
    expect(read.querySelector('script, .katex')).toBeNull();
    expect(target.getText()).toContain('a<b & "c"');
    // And reads it back as the same document.
    expect(stored(create(html))).toEqual(stored(target));
  });
});

describe('the list of emoji', () => {
  const typed = async (before: DocNode | undefined, value: string): Promise<Editor> => {
    const target = create(before);
    toEnd(target);
    type(target, value);
    // The list is asked for after the editor has dealt with the keystrokes.
    await new Promise((resolve) => setTimeout(resolve, 10));
    return target;
  };

  it.each([
    [':smi', 'smi'],
    ['hello :smi', 'smi'],
    // Behind Korean, Chinese and Japanese text, with no space in between.
    ['안녕:smi', 'smi'],
    ['안녕:웃', '웃'],
    ['你好:smi', 'smi'],
    ['こんにちは:smi', 'smi'],
    ['カタカナ:ロケ', 'ロケ'],
    ['(:smi', 'smi'],
  ])('is asked for %j', async (value, query) => {
    await typed(undefined, value);
    expect(search).toHaveBeenLastCalledWith(query);
    expect(started).toHaveBeenCalled();
  });

  it.each([['10:30'], ['http://x'], ['a:smi'], ['note:smi'], ['안녕: smi'], [':'], ['안녕:']])(
    'is not asked for %j',
    async (value) => {
      await typed(undefined, value);
      expect(search).not.toHaveBeenCalled();
    },
  );

  it('counts a mention or a formula in front of the colon as no letter', async () => {
    await typed(doc(paragraph(text('a'), mention('u1'))), ':smi');
    expect(search).toHaveBeenLastCalledWith('smi');
    search.mockClear();
    await typed(doc(paragraph(math('x'))), ':smi');
    expect(search).toHaveBeenLastCalledWith('smi');
  });

  it('is not offered inside code', async () => {
    await typed(doc({ type: 'codeBlock', content: [text('x')] }), ' :smi');
    expect(search).not.toHaveBeenCalled();
  });

  it('puts the chosen emoji in place of what was typed, behind Korean text too', async () => {
    const target = await typed(undefined, '안녕:smi');
    const props = started.mock.lastCall?.[0] as {
      items: SuggestionEntry[];
      command: (entry: SuggestionEntry) => void;
    };
    expect(props.items).toEqual([{ value: '😄', label: '😄 smile' }]);
    props.command({ value: '😄', label: '😄 smile' });
    expect(stored(target)).toEqual(doc(paragraph(text('안녕😄'))));
  });
});
