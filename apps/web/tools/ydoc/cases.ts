/**
 * Cases written by hand, for `contracts/ydoc-vectors.json`.
 *
 * `BODIES` are bodies the shared vectors of a body do not have, where the
 * conversion has something to get wrong: a text of several runs, letters
 * outside the basic plane, an attribute the stored JSON leaves out.
 *
 * The rest is shared content no editor writes but a merge of two edits, an
 * older binding or a client that is not ours may leave in a Y document, each
 * as the children of the root fragment:
 *
 * - `READINGS` are kept as they are by every editor that did not write them,
 *   and read in a way the shared content alone does not tell.
 * - `REFUSED` are kept the same way, though what is read is no body: the
 *   editor's schema checks no value. Only the server's rules refuse them.
 * - `REPAIRS` do not fit the editor's schema: every editor that sees one
 *   deletes what fails, and what the vectors hold is what is left.
 *
 * Not here, because no reading of the document alone shows it: a node at the
 * root that the document may not hold is wrapped by a live editor, and a
 * text at the root makes its binding throw. An editor also joins
 * neighbouring texts that it wrote itself. And an
 * element named `text` is kept or takes its line with it, depending on what
 * stands beside it; the server deletes the element.
 */
import type { DocNode } from '../../src/lib/editor/schema.ts';

import type { RawNode } from './shape.ts';

export interface RawCase {
  name: string;
  root: RawNode[];
}

const ADA = '01930000-0000-7000-8000-00000000c002';
/** A block a reference names. */
const PLAN = '01930000-0000-7000-8000-00000000b002';

const text = (insert: string, attributes?: Record<string, unknown>): RawNode => ({
  text: [attributes ? { insert, attributes } : { insert }],
});
const paragraph = (...children: RawNode[]): RawNode => ({ element: 'paragraph', children });
const listItem = (...children: RawNode[]): RawNode => ({ element: 'listItem', children });
const bulletList = (...children: RawNode[]): RawNode => ({ element: 'bulletList', children });
const blockquote = (...children: RawNode[]): RawNode => ({ element: 'blockquote', children });

const run = (value: string, ...marks: string[]): DocNode =>
  marks.length > 0
    ? { type: 'text', text: value, marks: marks.map((type) => ({ type })) }
    : { type: 'text', text: value };
const lineOf = (...content: DocNode[]): DocNode => ({ type: 'paragraph', content });
const bodyOf = (...content: DocNode[]): DocNode => ({ type: 'doc', content });
const LINK = { type: 'link', attrs: { href: 'https://example.com/' } };

export interface BodyCase {
  name: string;
  body: DocNode;
}

export const BODIES: BodyCase[] = [
  {
    name: 'a text of three runs: plain, bold, plain',
    body: bodyOf(lineOf(run('plain '), run('bold', 'bold'), run(' plain'))),
  },
  {
    name: 'runs whose marks overlap, begin and end one after another',
    body: bodyOf(
      lineOf(
        run('a'),
        run('b', 'bold'),
        run('c', 'bold', 'italic'),
        run('d', 'italic'),
        run('e', 'italic', 'strike'),
        { type: 'text', text: 'f', marks: [LINK, { type: 'strike' }] },
        { type: 'text', text: 'g', marks: [LINK] },
        run('h', 'code'),
        run('i'),
      ),
    ),
  },
  {
    name: 'marks written against the order of the schema',
    body: bodyOf(
      lineOf({
        type: 'text',
        text: 'x',
        marks: [{ type: 'strike' }, { type: 'italic' }, { type: 'bold' }, LINK],
      }),
    ),
  },
  {
    name: 'runs around a node that is no text',
    body: bodyOf(
      lineOf(
        run('a', 'bold'),
        run('b'),
        { type: 'hardBreak' },
        run('c', 'italic'),
        { type: 'inlineMath', attrs: { latex: 'x' } },
        { type: 'mention', attrs: { userId: ADA } },
        { type: 'reference', attrs: { id: PLAN } },
        run('d', 'bold'),
        run('e', 'bold', 'italic'),
      ),
    ),
  },
  {
    name: 'neighbouring texts with the same marks come back as one',
    body: bodyOf(lineOf(run('a'), run('b'), run('c', 'bold'), run('d', 'bold'), run('e'))),
  },
  {
    name: 'letters outside the basic plane between marks',
    body: bodyOf(
      lineOf(run('가😀', 'bold'), run('😀나'), run('👩‍👩‍👧‍👦', 'italic'), run('𝒳y', 'bold', 'italic')),
      { type: 'heading', attrs: { level: 3 }, content: [run('한글 '), run('😀', 'strike')] },
      { type: 'codeBlock', content: [run('𝒳 = "😀"\n\t끝')] },
    ),
  },
  {
    name: 'an ordered list without a start is given one',
    body: bodyOf({
      type: 'orderedList',
      content: [{ type: 'listItem', content: [lineOf(run('n'))] }],
    }),
  },
  {
    name: 'lists inside lists, a quote and tasks',
    body: bodyOf({
      type: 'bulletList',
      content: [
        {
          type: 'listItem',
          content: [
            lineOf(run('one')),
            {
              type: 'orderedList',
              attrs: { start: 3 },
              content: [{ type: 'listItem', content: [lineOf(), lineOf(run('deep', 'code'))] }],
            },
            {
              type: 'blockquote',
              content: [
                {
                  type: 'taskList',
                  content: [
                    { type: 'taskItem', attrs: { checked: false }, content: [lineOf()] },
                    {
                      type: 'taskItem',
                      attrs: { checked: true },
                      content: [lineOf(run('done')), { type: 'horizontalRule' }],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    }),
  },
];

const entryOf = (value: string): DocNode => ({ type: 'listItem', content: [lineOf(run(value))] });

/**
 * A body as one person edits it, state after state: the first is what the
 * server opens, each one after it what the editor holds after an edit. The
 * vectors hold the updates the binding sends for each step.
 */
export const SESSION: DocNode[] = [
  bodyOf(lineOf(run('hello world')), {
    type: 'bulletList',
    content: [entryOf('one'), entryOf('two')],
  }),
  // Typing in the middle of a text.
  bodyOf(lineOf(run('hello brave world')), {
    type: 'bulletList',
    content: [entryOf('one'), entryOf('two')],
  }),
  // A mark on a word.
  bodyOf(lineOf(run('hello '), run('brave', 'bold'), run(' world')), {
    type: 'bulletList',
    content: [entryOf('one'), entryOf('two')],
  }),
  // An entry deleted, a heading put before everything.
  bodyOf(
    { type: 'heading', attrs: { level: 2 }, content: [run('Title 😀')] },
    lineOf(run('hello '), run('brave', 'bold'), run(' world')),
    { type: 'bulletList', content: [entryOf('two')] },
  ),
  // The mark taken off again, the paragraph emptied, the heading's level changed.
  bodyOf({ type: 'heading', attrs: { level: 3 }, content: [run('Title 😀')] }, lineOf(), {
    type: 'bulletList',
    content: [entryOf('two'), entryOf('three')],
  }),
  // A diagram at the end, and letters after the emoji.
  bodyOf(
    { type: 'heading', attrs: { level: 3 }, content: [run('Title 😀!')] },
    lineOf(run('again')),
    { type: 'bulletList', content: [entryOf('two'), entryOf('three')] },
    { type: 'codeBlock', attrs: { language: 'mermaid' }, content: [run('graph TD\n  A-->B')] },
  ),
];

export const READINGS: RawCase[] = [
  {
    name: 'a heading without a level is of level 1',
    root: [{ element: 'heading', children: [text('h')] }],
  },
  {
    name: 'a task item without checked is not checked',
    root: [{ element: 'taskList', children: [{ element: 'taskItem', children: [paragraph()] }] }],
  },
  {
    name: 'an ordered list without a start starts at 1',
    root: [{ element: 'orderedList', children: [listItem(paragraph(text('n')))] }],
  },
  {
    name: 'an attribute nobody knows is not read',
    root: [
      { element: 'paragraph', attrs: { colour: 'red', level: 2 }, children: [text('p')] },
      { element: 'image', attrs: { fileId: 'f.png', width: 10 } },
      { element: 'file', attrs: { fileId: 'f.pdf', name: 'minutes.pdf', size: 7 } },
    ],
  },
  {
    name: 'texts side by side are one text',
    root: [paragraph(text('a'), text('b'), text('c', { bold: {} }), text('d', { bold: {} }))],
  },
  {
    name: 'an empty text is nothing',
    root: [paragraph(text('a')), paragraph({ text: [] }), paragraph({ text: [] }, text('b'))],
  },
  {
    name: 'a document of one paragraph with an empty text is no body',
    root: [paragraph({ text: [] })],
  },
  {
    name: 'an empty text between two elements does not part them',
    root: [paragraph({ element: 'hardBreak' }, { text: [] }, { element: 'hardBreak' })],
  },
  {
    name: 'marks come in the order of the schema, however they were written',
    root: [
      paragraph({
        text: [
          {
            insert: 'x',
            attributes: {
              code: {},
              strike: {},
              italic: {},
              bold: {},
              link: { href: 'https://example.com/' },
            },
          },
        ],
      }),
    ],
  },
  {
    name: 'bold and code together, which the editor never sets, are both kept',
    root: [paragraph(text('x', { bold: {}, code: {} }))],
  },
  {
    name: 'a mark named with a hash is the mark before the hash',
    root: [paragraph(text('x', { 'bold--abcdefgh': {} }), text('y', { 'italic--AB+/=012': {} }))],
  },
  {
    name: 'a mark whose value is true is the mark',
    root: [paragraph(text('x', { bold: true }))],
  },
  {
    name: 'an attribute of a mark nobody knows is not read',
    root: [
      paragraph(text('x', { link: { href: '/boards', target: '_blank' }, bold: { weight: 9 } })),
    ],
  },
  {
    name: 'runs with the same marks after reading are one text',
    root: [paragraph(text('a', { bold: {} }), text('b', { 'bold--abcdefgh': {} }))],
  },
];

const heading = (attrs: Record<string, unknown>): RawNode => ({
  element: 'heading',
  attrs: attrs as Record<string, string>,
  children: [text('h')],
});

export const REFUSED: RawCase[] = [
  { name: 'a heading of level 7', root: [heading({ level: 7 }), paragraph(text('a'))] },
  { name: 'a heading whose level is a text', root: [heading({ level: '2' })] },
  { name: 'a heading whose level is null', root: [heading({ level: null })] },
  { name: 'an image without a file', root: [{ element: 'image' }, paragraph(text('a'))] },
  { name: 'an image whose file is empty', root: [{ element: 'image', attrs: { fileId: '' } }] },
  { name: 'a file block without a file', root: [{ element: 'file' }, paragraph(text('a'))] },
  { name: 'a file block whose file is empty', root: [{ element: 'file', attrs: { fileId: '' } }] },
  {
    name: 'a mention of nobody, and of somebody who is no identifier',
    root: [
      paragraph({ element: 'mention' }),
      paragraph({ element: 'mention', attrs: { userId: 'x' } }),
    ],
  },
  {
    name: 'a reference to nothing, and to something that is no identifier',
    root: [
      paragraph({ element: 'reference' }),
      paragraph({ element: 'reference', attrs: { id: 'x' } }),
    ],
  },
  {
    name: 'formulas without a source and with an empty one',
    root: [
      { element: 'blockMath' },
      { element: 'blockMath', attrs: { latex: '' } },
      paragraph({ element: 'inlineMath' }, text('a')),
    ],
  },
  { name: 'a link without a target', root: [paragraph(text('x', { link: {} }))] },
  { name: 'a link whose value is true', root: [paragraph(text('x', { link: true }))] },
  { name: 'a link whose value is a text', root: [paragraph(text('x', { link: '/boards' }))] },
  {
    name: 'a link that runs a script',
    root: [paragraph(text('a'), text('x', { link: { href: 'javascript:alert(1)' } }))],
  },
  {
    name: 'an ordered list that starts below zero',
    root: [{ element: 'orderedList', attrs: { start: -1 }, children: [listItem(paragraph())] }],
  },
  {
    name: 'a task item whose checked is a text',
    root: [
      {
        element: 'taskList',
        children: [{ element: 'taskItem', attrs: { checked: 'yes' }, children: [paragraph()] }],
      },
    ],
  },
  {
    name: 'a code block whose language has a space',
    root: [{ element: 'codeBlock', attrs: { language: 'c sharp' }, children: [text('x')] }],
  },
  { name: 'a text with the character U+0000', root: [paragraph(text('a\u0000b'))] },
  {
    name: 'a mark given twice, once under a name with a hash',
    root: [paragraph({ text: [{ insert: 'x', attributes: { bold: {}, 'bold--abcdefgh': {} } }] })],
  },
  {
    name: 'two links on one text, under names with a hash',
    root: [
      paragraph({
        text: [
          {
            insert: 'x',
            attributes: {
              'link--abcdefgh': { href: '/one' },
              'link--ABCDEFGH': { href: '/two' },
            },
          },
        ],
      }),
    ],
  },
];

export const REPAIRS: RawCase[] = [
  { name: 'a bullet list without entries', root: [paragraph(text('a')), bulletList()] },
  {
    name: 'an ordered list and a task list without entries',
    root: [
      { element: 'orderedList', attrs: { start: 3 } },
      paragraph(text('a')),
      { element: 'taskList' },
    ],
  },
  { name: 'a blockquote with nothing in it', root: [blockquote(), paragraph(text('a'))] },
  {
    name: 'a list entry with nothing in it, beside one that is whole',
    root: [bulletList(listItem(), listItem(paragraph(text('kept'))))],
  },
  {
    name: 'a list whose only entry has nothing in it: the list goes with it',
    root: [paragraph(text('a')), bulletList(listItem())],
  },
  {
    name: 'a list entry that does not start with a paragraph',
    root: [
      bulletList(listItem(blockquote(paragraph(text('q')))), listItem(paragraph(text('kept')))),
    ],
  },
  {
    name: 'a task item that starts with a list',
    root: [
      paragraph(text('a')),
      {
        element: 'taskList',
        children: [
          {
            element: 'taskItem',
            attrs: { checked: true },
            children: [bulletList(listItem(paragraph()))],
          },
        ],
      },
    ],
  },
  {
    name: 'what fails deep down takes its emptied ancestors with it',
    root: [paragraph(text('a')), blockquote(bulletList(listItem(bulletList())))],
  },
  {
    name: 'a failing node between whole ones leaves them',
    root: [blockquote(paragraph(text('a')), bulletList(), paragraph(text('b')))],
  },
  {
    name: 'an element of a kind nobody knows',
    root: [paragraph(text('a')), { element: 'video', attrs: { src: 'x' } }],
  },
  {
    name: 'an element of a kind nobody knows in a line: it goes, the line stays',
    root: [paragraph(text('a'), { element: 'emoji', attrs: { name: 'x' } }, text('b'))],
  },
  {
    name: 'a text with a mark nobody knows goes whole, its neighbours stay',
    root: [
      paragraph(
        text('a'),
        { element: 'hardBreak' },
        { text: [{ insert: 'b' }, { insert: 'glow', attributes: { glow: {} } }] },
      ),
    ],
  },
  {
    name: 'a code block whose text has a mark',
    root: [paragraph(text('a')), { element: 'codeBlock', children: [text('b', { bold: {} })] }],
  },
  {
    name: 'a code block that holds an element',
    root: [
      paragraph(text('a')),
      { element: 'codeBlock', children: [text('b'), { element: 'hardBreak' }] },
    ],
  },
  {
    name: 'a paragraph that holds a block: the paragraph goes, not the block alone',
    root: [paragraph(text('a')), paragraph(text('b'), paragraph(text('c')))],
  },
  {
    name: 'a heading that holds a list',
    root: [
      paragraph(text('a')),
      { element: 'heading', attrs: { level: 2 }, children: [bulletList(listItem(paragraph()))] },
    ],
  },
  {
    name: 'leaves with something inside',
    root: [
      paragraph(text('a'), { element: 'hardBreak', children: [text('x')] }),
      { element: 'image', attrs: { fileId: 'f.png' }, children: [paragraph()] },
      { element: 'file', attrs: { fileId: 'f.pdf' }, children: [paragraph()] },
      { element: 'horizontalRule', children: [paragraph()] },
      { element: 'blockMath', attrs: { latex: 'x' }, children: [text('x')] },
      paragraph({ element: 'mention', attrs: { userId: ADA }, children: [text('x')] }),
      paragraph({ element: 'reference', attrs: { id: PLAN }, children: [text('x')] }),
      paragraph({ element: 'inlineMath', attrs: { latex: 'x' }, children: [text('x')] }, text('b')),
    ],
  },
  {
    name: 'a blockquote that holds a text',
    root: [paragraph(text('a')), blockquote(text('loose'))],
  },
  {
    name: 'a list that holds an entry of another kind of list',
    root: [
      paragraph(text('a')),
      bulletList({ element: 'taskItem', attrs: { checked: false }, children: [paragraph()] }),
      { element: 'taskList', children: [listItem(paragraph())] },
    ],
  },
  {
    name: 'a list that holds a paragraph beside an entry',
    root: [paragraph(text('a')), bulletList(listItem(paragraph(text('b'))), paragraph(text('c')))],
  },
  {
    name: 'a list entry inside a blockquote',
    root: [paragraph(text('a')), blockquote(listItem(paragraph(text('b'))))],
  },
  {
    name: 'a document inside a quote',
    root: [
      paragraph(text('a')),
      blockquote(paragraph(text('q')), { element: 'doc', children: [paragraph(text('b'))] }),
    ],
  },
  {
    name: 'a task item with nothing in it, beside one that is whole',
    root: [
      {
        element: 'taskList',
        children: [
          { element: 'taskItem', attrs: { checked: true } },
          { element: 'taskItem', attrs: { checked: false }, children: [paragraph(text('kept'))] },
        ],
      },
    ],
  },
  {
    name: 'a list entry that holds a text',
    root: [paragraph(text('a')), bulletList(listItem(text('loose')))],
  },
  {
    name: 'an ordered list that holds a paragraph',
    root: [
      paragraph(text('a')),
      { element: 'orderedList', attrs: { start: 1 }, children: [paragraph(text('b'))] },
    ],
  },
  {
    name: 'an element where a text belongs, in a code block inside a list',
    root: [
      bulletList(
        listItem(paragraph(text('kept')), {
          element: 'codeBlock',
          children: [{ element: 'mention', attrs: { userId: ADA } }],
        }),
      ),
    ],
  },
  { name: 'nothing is left', root: [bulletList(), { element: 'video' }, blockquote(blockquote())] },
];
