/**
 * Tables in the editor of a body: what the slash menu offers for them and
 * does, the limits a reader's change keeps to, what pasting a table makes,
 * and that an editor bound to a shared document pads only what its own
 * change left unfilled, leaving to the server what a merge left so.
 */
import { Editor } from '@tiptap/core';
import { Collaboration } from '@tiptap/extension-collaboration';
import { TextSelection } from '@tiptap/pm/state';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { toStoredDoc, validateDoc, type DocNode } from '$lib/editor/schema.ts';
import { tableEntries } from '$lib/editor/table-menu.ts';
import { documentOf } from '$lib/state/memory/room-docs.ts';
import { mendTables } from '$lib/state/memory/table-mend.ts';

const editors: Editor[] = [];

function create(content: DocNode, shared?: Y.Doc): Editor {
  const made = new Editor({
    element: document.createElement('div'),
    extensions: [
      ...(shared ? [Collaboration.configure({ document: shared, field: BODY_FIELD })] : []),
      ...bodyExtensions({ history: !shared }),
    ],
    ...(shared ? {} : { content }),
  });
  editors.push(made);
  return made;
}

afterEach(() => {
  for (const made of editors.splice(0)) made.destroy();
});

const stored = (editor: Editor): DocNode => toStoredDoc(editor.getJSON() as DocNode);
const paragraph = (text: string): DocNode =>
  text === '' ? { type: 'paragraph' } : { type: 'paragraph', content: [{ type: 'text', text }] };
const row = (texts: string[], type = 'tableCell'): DocNode => ({
  type: 'tableRow',
  content: texts.map((text) => ({ type, content: [paragraph(text)] })),
});
const table = (...rows: DocNode[]): DocNode => ({ type: 'table', content: rows });
const doc = (...content: DocNode[]): DocNode => ({ type: 'doc', content });

/** The texts of a document's first table, row by row. */
function cells(editor: Editor): string[][] {
  const found: string[][] = [];
  editor.state.doc.firstChild?.forEach((line) => {
    const texts: string[] = [];
    line.forEach((cell) => texts.push(cell.textContent));
    found.push(texts);
  });
  return found;
}

/** The position inside the text of the cell at `row`, `column` of the first table. */
function inCell(editor: Editor, at: [number, number]): number {
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (found >= 0) return false;
    if (node.type.name !== 'tableRow') return true;
    const index = editor.state.doc.resolve(pos).index(1);
    if (index !== at[0]) return false;
    let offset = pos + 1;
    node.forEach((cell, _, column) => {
      if (column === at[1]) found = offset + 2;
      offset += cell.nodeSize;
    });
    return false;
  });
  return found;
}

function putCaret(editor: Editor, at: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, at)));
}

/** A paste of what the clipboard holds, by type, as the page hands it over (it has no clipboard of its own). */
function paste(editor: Editor, data: Record<string, string>): void {
  const event = new Event('paste', { bubbles: true, cancelable: true });
  const clipboardData = {
    getData: (type: string) => data[type] ?? '',
    types: Object.keys(data),
    files: [],
    items: [],
  };
  Object.defineProperty(event, 'clipboardData', { value: clipboardData });
  editor.view.dom.dispatchEvent(event);
}
const pasteText = (editor: Editor, text: string): void => {
  paste(editor, { 'text/plain': text });
};
const pasteHTML = (editor: Editor, html: string): void => {
  paste(editor, { 'text/html': html, 'text/plain': '' });
};

const offered = (editor: Editor): string[] => tableEntries(editor).map((entry) => entry.value);
function run(editor: Editor, value: string): void {
  const entry = tableEntries(editor).find((each) => each.value === value);
  if (!entry) throw new Error(`${value} is not offered`);
  entry.run(editor);
}

const wide = (rows: number, columns: number): DocNode =>
  doc(
    table(...Array.from({ length: rows }, () => row(Array.from({ length: columns }, () => 'x')))),
  );

describe('the slash menu for tables', () => {
  it('offers a table on a line of the document, and makes one of a row of headers and two rows', () => {
    const editor = create(doc(paragraph('')));
    putCaret(editor, 1);
    expect(offered(editor)).toEqual(['table']);
    run(editor, 'table');
    expect(cells(editor)).toEqual([
      ['', '', ''],
      ['', '', ''],
      ['', '', ''],
    ]);
    const made = stored(editor);
    expect(validateDoc(made)).toEqual([]);
    expect(made.content?.[0]?.content?.[0]?.content?.map((cell) => cell.type)).toEqual([
      'tableHeader',
      'tableHeader',
      'tableHeader',
    ]);
    // The caret is in the first cell, to write in it.
    expect(editor.state.selection.$from.node(3).type.name).toBe('tableHeader');
  });

  it('offers no table in a list entry, a quote or a cell, where none may stand', () => {
    const editor = create(
      doc(
        { type: 'bulletList', content: [{ type: 'listItem', content: [paragraph('a')] }] },
        { type: 'blockquote', content: [paragraph('b')] },
      ),
    );
    putCaret(editor, 4);
    expect(offered(editor)).toEqual([]);
    putCaret(editor, editor.state.doc.content.size - 2);
    expect(offered(editor)).toEqual([]);
    const inTable = create(doc(table(row(['a', 'b']), row(['c', 'd']))));
    putCaret(inTable, inCell(inTable, [0, 0]));
    expect(offered(inTable)).not.toContain('table');
  });

  it('adds and deletes a row and a column at the cell the reader is in, and turns the headers on and off', () => {
    const editor = create(doc(table(row(['a', 'b']), row(['c', 'd']))));
    putCaret(editor, inCell(editor, [0, 0]));
    expect(offered(editor)).toEqual([
      'table-row-add',
      'table-column-add',
      'table-row-delete',
      'table-column-delete',
      'table-header-row',
      'table-delete',
    ]);
    run(editor, 'table-row-add');
    run(editor, 'table-column-add');
    expect(cells(editor)).toEqual([
      ['a', '', 'b'],
      ['', '', ''],
      ['c', '', 'd'],
    ]);
    expect(tableEntries(editor).find((entry) => entry.value === 'table-header-row')?.label).toBe(
      'Make the first row headers',
    );
    run(editor, 'table-header-row');
    expect(stored(editor).content?.[0]?.content?.[0]?.content?.[0]?.type).toBe('tableHeader');
    expect(tableEntries(editor).find((entry) => entry.value === 'table-header-row')?.label).toBe(
      'Make the first row plain cells',
    );
    putCaret(editor, inCell(editor, [1, 1]));
    run(editor, 'table-row-delete');
    putCaret(editor, inCell(editor, [0, 1]));
    run(editor, 'table-column-delete');
    expect(cells(editor)).toEqual([
      ['a', 'b'],
      ['c', 'd'],
    ]);
    expect(validateDoc(stored(editor))).toEqual([]);
    run(editor, 'table-delete');
    expect(stored(editor).content?.some((node) => node.type === 'table')).toBe(false);
  });

  it('offers no row past 200 nor column past 20, and the commands and Tab add none either', () => {
    const editor = create(wide(200, 20));
    putCaret(editor, inCell(editor, [199, 19]));
    expect(offered(editor)).not.toContain('table-row-add');
    expect(offered(editor)).not.toContain('table-column-add');
    const before = editor.state.doc;
    editor.commands.addRowAfter();
    editor.commands.addColumnAfter();
    // Tab in the last cell adds a row everywhere else.
    editor.view.someProp('handleKeyDown', (handle) =>
      handle(editor.view, new KeyboardEvent('keydown', { key: 'Tab' })),
    );
    expect(editor.state.doc.eq(before)).toBe(true);
    // Below the limits, Tab in the last cell adds a row.
    const small = create(doc(table(row(['a']))));
    putCaret(small, inCell(small, [0, 0]));
    small.view.someProp('handleKeyDown', (handle) =>
      handle(small.view, new KeyboardEvent('keydown', { key: 'Tab' })),
    );
    expect(cells(small)).toEqual([['a'], ['']]);
  });
});

describe('a table pasted', () => {
  it('as Markdown text is a table, of headers over cells, and other text stays text', () => {
    const editor = create(doc(paragraph('')));
    putCaret(editor, 1);
    pasteText(editor, '| a | **b** |\n|:--|---|\n| 1 | 2 |\n');
    const made = stored(editor);
    expect(
      made.content?.[0]?.content?.map((line) => line.content?.map((cell) => cell.type)),
    ).toEqual([
      ['tableHeader', 'tableHeader'],
      ['tableCell', 'tableCell'],
    ]);
    expect(JSON.stringify(made)).toContain('"marks":[{"type":"bold"}]');
    expect(cells(editor)).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ]);
    expect(validateDoc(made)).toEqual([]);
    const plain = create(doc(paragraph('')));
    putCaret(plain, 1);
    pasteText(plain, '| not | a table |\n**bold** stays as typed');
    expect(plain.state.doc.firstChild?.textContent).toBe('| not | a table |');
    expect(stored(plain).content?.some((node) => node.type === 'table')).toBe(false);
  });

  it('as Markdown into a cell fills the cells from there', () => {
    const editor = create(doc(table(row(['a', 'b']), row(['c', 'd']))));
    putCaret(editor, inCell(editor, [1, 1]));
    pasteText(editor, '| x | y |\n|---|---|\n| z | w |');
    expect(validateDoc(stored(editor))).toEqual([]);
    expect(editor.state.doc.childCount).toBe(1);
    // From the reader's cell on, the table grown by what it lacked.
    expect(cells(editor)).toEqual([
      ['a', 'b', ''],
      ['c', 'x', 'y'],
      ['', 'z', 'w'],
    ]);
  });

  it('alone into a cell whence it would take the table past 20 columns is its rows as lines there', () => {
    const editor = create(wide(2, 20));
    putCaret(editor, inCell(editor, [0, 19]));
    pasteHTML(editor, '<table><tr><td>p</td><td>q</td></tr><tr><td>r</td><td>s</td></tr></table>');
    const made = stored(editor);
    expect(validateDoc(made)).toEqual([]);
    expect(editor.state.doc.childCount).toBe(1);
    expect(cells(editor).map((line) => line.length)).toEqual([20, 20]);
    const last = editor.state.doc.firstChild?.firstChild?.lastChild;
    // Pasted where the caret is: at the start of the cell's line.
    expect(last?.content.content.map((block) => block.textContent)).toEqual([
      'p | q',
      'r | s',
      'x',
    ]);
  });

  it('into a cell among other blocks is its rows as lines, a block of no text after its row', () => {
    const editor = create(doc(table(row(['a', 'b']))));
    putCaret(editor, inCell(editor, [0, 1]));
    pasteHTML(
      editor,
      '<p>before</p><table><tr><td>c</td><td><hr></td></tr><tr><td>d</td><td>e</td></tr></table>',
    );
    const made = stored(editor);
    expect(validateDoc(made)).toEqual([]);
    expect(editor.state.doc.childCount).toBe(1);
    const last = editor.state.doc.firstChild?.firstChild?.lastChild;
    expect(last?.content.content.map((block) => [block.type.name, block.textContent])).toEqual([
      ['paragraph', 'before'],
      ['paragraph', 'c | '],
      ['horizontalRule', ''],
      ['paragraph', 'd | e'],
      ['paragraph', 'b'],
    ]);
  });

  it('as Markdown with a table in a list entry keeps the entry, the table in it as lines', () => {
    const editor = create(doc(paragraph('')));
    putCaret(editor, 1);
    pasteText(
      editor,
      '| a | b |\n|---|---|\n| 1 | 2 |\n\n1. step\n\n   | c | d |\n   |---|---|\n   | 3 | 4 |\n\n2. next',
    );
    const made = stored(editor);
    expect(validateDoc(made)).toEqual([]);
    expect(() => {
      editor.state.doc.check();
    }).not.toThrow();
    expect(made.content?.map((node) => node.type)).toEqual(['table', 'orderedList']);
    const list = editor.state.doc.child(1);
    expect(list.child(0).content.content.map((block) => block.textContent)).toEqual([
      'step',
      'c | d',
      '3 | 4',
    ]);
    expect(list.child(1).textContent).toBe('next');
  });

  it('into a list entry stands in the document itself', () => {
    const editor = create(
      doc({ type: 'bulletList', content: [{ type: 'listItem', content: [paragraph('item')] }] }),
    );
    putCaret(editor, 7);
    pasteText(editor, '| x |\n|---|\n| z |');
    const made = stored(editor);
    expect(validateDoc(made)).toEqual([]);
    expect(made.content?.some((node) => node.type === 'table')).toBe(true);
  });

  it('past the limits, from a page or as Markdown, is its rows as lines', () => {
    const editor = create(doc(paragraph('')));
    putCaret(editor, 1);
    const head = Array.from({ length: 21 }, (_, at) => `<th>h${String(at)}</th>`).join('');
    const body = Array.from({ length: 21 }, (_, at) => `<td><b>c${String(at)}</b></td>`).join('');
    pasteHTML(editor, `<table><tr>${head}</tr><tr>${body}</tr></table>`);
    const made = stored(editor);
    expect(made.content?.some((node) => node.type === 'table')).toBe(false);
    expect(made.content?.map((node) => node.type)).toEqual(['paragraph', 'paragraph']);
    expect(editor.state.doc.child(1).textContent).toBe(
      Array.from({ length: 21 }, (_, at) => `c${String(at)}`).join(' | '),
    );
    expect(validateDoc(made)).toEqual([]);

    const markdown = create(doc(paragraph('')));
    putCaret(markdown, 1);
    const rows = Array.from({ length: 201 }, (_, at) => `| r${String(at)} |`);
    pasteText(markdown, [rows[0], '|---|', ...rows.slice(1)].join('\n'));
    expect(stored(markdown).content?.some((node) => node.type === 'table')).toBe(false);
    expect(markdown.state.doc.childCount).toBe(201);
  });

  it('from a page with a table in a cell is parted around it, so that no table stands in a cell', () => {
    const editor = create(doc(paragraph('')));
    putCaret(editor, 1);
    pasteHTML(
      editor,
      '<table><tr><td>a<table><tr><td>inner</td></tr></table></td><td>b</td></tr></table>',
    );
    const made = stored(editor);
    expect(validateDoc(made)).toEqual([]);
    // The parser closes the outer table before the inner one and opens another after it.
    const tables: string[][][] = [];
    editor.state.doc.forEach((block) => {
      expect(block.type.name).toBe('table');
      const rows: string[][] = [];
      block.forEach((line) => {
        const texts: string[] = [];
        line.forEach((one) => texts.push(one.textContent));
        rows.push(texts);
      });
      tables.push(rows);
    });
    expect(tables).toEqual([[['a']], [['inner']], [['b']]]);
  });
});

describe('an editor bound to a shared document', () => {
  /** What one document has that another lacks, given to it as the room gives it. */
  function deliver(from: Y.Doc, to: Y.Doc): void {
    Y.applyUpdate(to, Y.encodeStateAsUpdate(from, Y.encodeStateVector(to)), 'room');
  }
  const rowsIn = (shared: Y.Doc): number[] =>
    ((shared.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement).toArray() as Y.XmlElement[]).map(
      (line) => line.length,
    );

  it('pads no table a merge left unfilled, not even when its reader then writes in it', async () => {
    const start = documentOf(doc(table(row(['a', 'b']), row(['c', 'd']))));
    const [first, second] = [new Y.Doc(), new Y.Doc()];
    deliver(start, first);
    deliver(start, second);
    const mine = create(doc(), first);
    const theirs = create(doc(), second);
    await vi.waitFor(() => {
      expect(cells(mine)).toEqual([
        ['a', 'b'],
        ['c', 'd'],
      ]);
      expect(cells(theirs)).toEqual(cells(mine));
    });
    // At once: a column on one side, a row on the other.
    putCaret(mine, inCell(mine, [0, 1]));
    mine.commands.addColumnAfter();
    putCaret(theirs, inCell(theirs, [1, 0]));
    theirs.commands.addRowAfter();
    expect(rowsIn(first)).toEqual([3, 3]);
    expect(rowsIn(second)).toEqual([2, 2, 2]);
    deliver(first, second);
    deliver(second, first);
    await vi.waitFor(() => {
      expect(cells(mine)).toHaveLength(3);
      expect(cells(theirs)).toHaveLength(3);
    });
    // Each holds the merge as it is: the new row a cell short, which no editor padded.
    expect(rowsIn(first)).toEqual([3, 3, 2]);
    expect(rowsIn(second)).toEqual([3, 3, 2]);
    putCaret(mine, inCell(mine, [0, 0]));
    mine.commands.insertContent('!');
    expect(rowsIn(first)).toEqual([3, 3, 2]);

    // The room mends it, once, and both hold the mended table.
    const room = new Y.Doc();
    deliver(first, room);
    deliver(second, room);
    expect(mendTables(room)).toBeDefined();
    deliver(room, first);
    deliver(room, second);
    await vi.waitFor(() => {
      expect(rowsIn(first)).toEqual([3, 3, 3]);
      expect(rowsIn(second)).toEqual([3, 3, 3]);
      expect(stored(mine)).toEqual(stored(theirs));
    });
    expect(validateDoc(stored(mine))).toEqual([]);

    // The reader's own change that leaves a table unfilled is still padded by the editor.
    putCaret(mine, inCell(mine, [2, 2]));
    const { from } = mine.state.selection;
    const $cell = mine.state.doc.resolve(from);
    mine.view.dispatch(mine.state.tr.delete($cell.before(3), $cell.after(3)));
    expect(rowsIn(first)).toEqual([3, 3, 3]);
  });
});
