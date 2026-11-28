import { describe, expect, it } from 'vitest';

import { countTasks, plainText } from '$lib/editor/body-text.ts';
import {
  BODY_VERSION,
  emptyDoc,
  isAllowedHref,
  isEmptyDoc,
  isFileId,
  MARK_SPECS,
  MAX_DEPTH,
  MAX_LATEX,
  MAX_NODES,
  NODE_SPECS,
  storedSize,
  toStoredDoc,
  validateDoc,
  type DocNode,
} from '$lib/editor/schema.ts';

const text = (value: string, ...marks: DocNode['marks'] & object): DocNode =>
  marks.length > 0 ? { type: 'text', text: value, marks } : { type: 'text', text: value };
const paragraph = (...content: DocNode[]): DocNode => ({ type: 'paragraph', content });
const doc = (...content: DocNode[]): DocNode => ({ type: 'doc', content });

describe('the list of what a body may contain', () => {
  it('has the twenty-eight nodes and five marks of version 7', () => {
    expect(BODY_VERSION).toBe(7);
    expect(Object.keys(NODE_SPECS).sort()).toEqual([
      'blockMath',
      'blockquote',
      'boardView',
      'bulletList',
      'callout',
      'codeBlock',
      'details',
      'detailsContent',
      'detailsSummary',
      'doc',
      'file',
      'hardBreak',
      'heading',
      'horizontalRule',
      'image',
      'inlineMath',
      'listItem',
      'mention',
      'orderedList',
      'paragraph',
      'reference',
      'table',
      'tableCell',
      'tableHeader',
      'tableRow',
      'taskItem',
      'taskList',
      'text',
    ]);
    expect(Object.keys(MARK_SPECS).sort()).toEqual(['bold', 'code', 'italic', 'link', 'strike']);
  });
});

describe('validating a document', () => {
  it('accepts a document that uses every node and mark', () => {
    const everything = doc(
      { type: 'heading', attrs: { level: 2 }, content: [text('Title')] },
      paragraph(
        text('plain '),
        text('bold', { type: 'bold' }),
        text('both', { type: 'italic' }, { type: 'strike' }),
        text('code', { type: 'code' }),
        text('link', { type: 'link', attrs: { href: 'https://example.com/a?b=1' } }),
        { type: 'hardBreak' },
        { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c001' } },
        { type: 'reference', attrs: { id: '01930000-0000-7000-8000-00000000b001' } },
      ),
      { type: 'bulletList', content: [{ type: 'listItem', content: [paragraph(text('a'))] }] },
      {
        type: 'orderedList',
        attrs: { start: 3 },
        content: [{ type: 'listItem', content: [paragraph(text('c'))] }],
      },
      {
        type: 'taskList',
        content: [
          {
            type: 'taskItem',
            attrs: { checked: true },
            content: [
              paragraph(text('done')),
              {
                type: 'taskList',
                content: [{ type: 'taskItem', attrs: { checked: false }, content: [paragraph()] }],
              },
            ],
          },
        ],
      },
      { type: 'blockquote', content: [paragraph(text('quoted'))] },
      { type: 'codeBlock', attrs: { language: 'rust' }, content: [text('fn main() {}')] },
      { type: 'codeBlock' },
      { type: 'horizontalRule' },
      { type: 'image', attrs: { fileId: 'f1', alt: 'a chart' } },
      { type: 'file', attrs: { fileId: 'f2.pdf' } },
    );
    expect(validateDoc(everything)).toEqual([]);
    expect(validateDoc(emptyDoc())).toEqual([]);
  });

  it.each<[string, unknown, string]>([
    ['an unknown node', doc({ type: 'video' }), 'unknown node type "video"'],
    [
      'an unknown mark',
      doc(paragraph(text('x', { type: 'underline' }))),
      'unknown mark "underline"',
    ],
    [
      'an unknown attribute',
      doc({ type: 'paragraph', attrs: { textAlign: 'center' } }),
      'unknown attribute "textAlign"',
    ],
    [
      'an unknown member',
      doc({ type: 'paragraph', style: 'x' } as DocNode),
      'unknown member "style"',
    ],
    [
      'a heading level out of range',
      doc({ type: 'heading', attrs: { level: 4 } }),
      '"level" has an invalid value',
    ],
    ['a missing required attribute', doc({ type: 'image' }), '"fileId" is missing'],
    [
      'an image by address',
      doc({ type: 'image', attrs: { fileId: 'f', src: 'https://x/y.png' } }),
      'unknown attribute "src"',
    ],
    [
      'a link with a script',
      doc(paragraph(text('x', { type: 'link', attrs: { href: 'javascript:alert(1)' } }))),
      '"href" has an invalid value',
    ],
    [
      'a link with extra attributes',
      doc(paragraph(text('x', { type: 'link', attrs: { href: '/a', target: '_blank' } }))),
      'unknown attribute "target"',
    ],
    ['empty text', doc(paragraph({ type: 'text', text: '' })), 'text is empty'],
    [
      'a block inside a paragraph',
      doc(paragraph({ type: 'horizontalRule' })),
      'not allowed inside',
    ],
    ['text directly in the document', doc(text('x')), 'not allowed inside'],
    ['a list without entries', doc({ type: 'bulletList' }), 'needs at least 1 child'],
    [
      'a list entry that does not start with a paragraph',
      doc({
        type: 'bulletList',
        content: [{ type: 'listItem', content: [{ type: 'horizontalRule' }] }],
      }),
      'has to start with a paragraph',
    ],
    [
      'marks in a code block',
      doc({ type: 'codeBlock', content: [text('x', { type: 'bold' })] }),
      'no marks',
    ],
    [
      'a mark given twice',
      doc(paragraph(text('x', { type: 'bold' }, { type: 'bold' }))),
      'given twice',
    ],
    ['something that is not a document', { type: 'paragraph' }, 'not allowed inside the root'],
    ['something that is not a node', 'text', 'not a node'],
  ])('refuses %s', (_name, value, problem) => {
    expect(validateDoc(value).join('\n')).toContain(problem);
  });

  it('refuses a document nested deeper than the limit', () => {
    let nested: DocNode = paragraph(text('deep'));
    for (let level = 0; level < MAX_DEPTH; level += 1)
      nested = { type: 'blockquote', content: [nested] };
    expect(validateDoc(doc(nested)).join('\n')).toContain('deeper than');
  });

  it('refuses a document with more nodes than the limit', () => {
    // Each paragraph is two nodes with its text.
    const many = doc(...Array.from({ length: MAX_NODES / 2 }, () => paragraph(text('x'))));
    expect(validateDoc(many).join('\n')).toContain(`more than ${MAX_NODES} nodes`);
  });
});

describe('link targets', () => {
  it.each([
    'https://example.com',
    'http://example.com/a b',
    'HTTPS://EXAMPLE.COM',
    'mailto:someone@example.com',
    '/board/abc',
    'relative/path?x=1#y',
    '#section',
    '../up',
  ])('allows %s', (href) => {
    expect(isAllowedHref(href)).toBe(true);
  });

  it.each([
    'javascript:alert(1)',
    'JavaScript:alert(1)',
    ' javascript:alert(1)',
    'java\tscript:alert(1)',
    'java\nscript:alert(1)',
    'java\rscript:alert(1)',
    '\u0000javascript:alert(1)',
    '\u0001javascript:alert(1)',
    '\u007fjavascript:alert(1)',
    'data:text/html,<script>alert(1)</script>',
    'vbscript:msgbox(1)',
    'file:///etc/passwd',
    'ftp://example.com',
    '//example.com/x',
    '\\\\example.com\\x',
    '/\\example.com',
    '',
    '   ',
  ])('refuses %j', (href) => {
    expect(isAllowedHref(href)).toBe(false);
  });

  it('refuses path traversal and unsafe characters in fileId while accepting server and seed ids', () => {
    for (const ok of [
      'f1',
      'f2.pdf',
      'viewer-sample.ko.docx',
      '01930000-0000-7000-8000-00000000b001',
    ]) {
      expect(isFileId(ok)).toBe(true);
    }
    for (const bad of [
      '',
      '.',
      '..',
      '../secret',
      'a..b',
      'a/b',
      'a\\b',
      'a b',
      'a?b=1',
      'x'.repeat(101),
    ]) {
      expect(isFileId(bad)).toBe(false);
    }
  });

  it('refuses what is not text, and text that is too long', () => {
    expect(isAllowedHref(undefined)).toBe(false);
    expect(isAllowedHref(42)).toBe(false);
    expect(isAllowedHref(`https://example.com/${'a'.repeat(2048)}`)).toBe(false);
  });
});

describe('the stored form', () => {
  it('leaves out attributes without a value and empty attribute lists', () => {
    const fromEditor: DocNode = doc(
      { type: 'codeBlock', attrs: { language: null }, content: [text('x')] },
      { type: 'image', attrs: { fileId: 'f1', alt: null } },
      {
        type: 'orderedList',
        attrs: { start: 1 },
        content: [{ type: 'listItem', content: [paragraph()] }],
      },
    );
    expect(toStoredDoc(fromEditor)).toEqual(
      doc(
        { type: 'codeBlock', content: [text('x')] },
        { type: 'image', attrs: { fileId: 'f1' } },
        {
          type: 'orderedList',
          content: [{ type: 'listItem', content: [paragraph()] }],
        },
      ),
    );
    expect(validateDoc(fromEditor)).not.toEqual([]);
    expect(validateDoc(toStoredDoc(fromEditor))).toEqual([]);
  });

  it('leaves out the start of an ordered list at 1, which a list without one starts at, and keeps any other', () => {
    const list = (attrs?: DocNode['attrs']): DocNode => ({
      type: 'orderedList',
      ...(attrs ? { attrs } : {}),
      content: [{ type: 'listItem', content: [paragraph()] }],
    });
    expect(toStoredDoc(doc(list({ start: 1 })))).toEqual(doc(list()));
    expect(toStoredDoc(doc(list({ start: 0 })))).toEqual(doc(list({ start: 0 })));
    expect(toStoredDoc(doc(list({ start: 3 })))).toEqual(doc(list({ start: 3 })));
    // Stored twice, the same.
    expect(toStoredDoc(toStoredDoc(doc(list({ start: 1 }))))).toEqual(doc(list()));
  });

  it('measures a document in bytes of UTF-8', () => {
    expect(storedSize(doc(paragraph(text('가'))))).toBe(
      '{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":""}]}]}'
        .length + 3,
    );
  });
});

describe('reading a document without an editor', () => {
  const sample = doc(
    { type: 'heading', attrs: { level: 1 }, content: [text('Plan')] },
    paragraph(
      text('ask '),
      { type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c001' } },
      { type: 'reference', attrs: { id: '01930000-0000-7000-8000-00000000b001' } },
      { type: 'hardBreak' },
      text('today'),
    ),
    {
      type: 'taskList',
      content: [
        {
          type: 'taskItem',
          attrs: { checked: true },
          content: [
            paragraph(text('one')),
            {
              type: 'taskList',
              content: [
                {
                  type: 'taskItem',
                  attrs: { checked: false },
                  content: [paragraph(text('inner'))],
                },
              ],
            },
          ],
        },
        { type: 'taskItem', attrs: { checked: true }, content: [paragraph()] },
      ],
    },
    { type: 'image', attrs: { fileId: 'f' } },
  );

  it('counts tasks at every depth', () => {
    expect(countTasks(sample)).toEqual({ total: 3, checked: 2 });
    expect(countTasks(emptyDoc())).toEqual({ total: 0, checked: 0 });
  });

  it('gives the words, one line per paragraph, with mentions by name and nothing for a reference', () => {
    expect(
      plainText(sample, (id) => (id === '01930000-0000-7000-8000-00000000c001' ? 'Ada' : id)),
    ).toBe('Plan\nask @Ada\ntoday\none\ninner');
    expect(plainText(emptyDoc())).toBe('');
  });
});

describe('a value that is not shaped like a document', () => {
  // The validator is the gate for what arrives from elsewhere: it answers, it never throws.
  const text = (extra: Record<string, unknown>): unknown => ({
    type: 'doc',
    content: [{ type: 'paragraph', content: [{ type: 'text', text: 'x', ...extra }] }],
  });

  it.each([
    ['marks that are a number', text({ marks: 5 })],
    ['marks that are text', text({ marks: 'bold' })],
    ['a mark that is null', text({ marks: [null] })],
    ['a mark that is text', text({ marks: ['bold'] })],
    ['a mark without a type', text({ marks: [{}] })],
    ['a mark with a member that is not on the list', text({ marks: [{ type: 'bold', x: 1 }] })],
    ['a mark with attributes that are a number', text({ marks: [{ type: 'link', attrs: 7 }] })],
    ['attributes that are a number', { type: 'doc', content: [{ type: 'paragraph', attrs: 7 }] }],
    ['attributes that are a list', { type: 'doc', content: [{ type: 'paragraph', attrs: [] }] }],
    [
      'an attribute named like a member every object has',
      { type: 'doc', content: [{ type: 'heading', attrs: { level: 1, constructor: 1 } }] },
    ],
    ['a child that is null', { type: 'doc', content: [null] }],
    ['a child that is text', { type: 'doc', content: ['paragraph'] }],
    [
      'a null child of a code block',
      { type: 'doc', content: [{ type: 'codeBlock', content: [null] }] },
    ],
    [
      'a null first child of a list item',
      {
        type: 'doc',
        content: [{ type: 'bulletList', content: [{ type: 'listItem', content: [null] }] }],
      },
    ],
    ['content that is an object', { type: 'doc', content: {} }],
    ['a number', 5],
    ['a list', []],
    ['nothing', null],
  ])('is refused without an exception: %s', (_name, value) => {
    let problems: string[] = [];
    expect(() => {
      problems = validateDoc(value);
    }).not.toThrow();
    expect(problems.length).toBeGreaterThan(0);
  });
});

describe('formulas', () => {
  const inline = (latex: unknown): DocNode => ({ type: 'inlineMath', attrs: { latex } });
  const block = (latex: unknown): DocNode => ({ type: 'blockMath', attrs: { latex } });

  it('accepts a formula in a line and one on a line of its own, wherever inline and block content go', () => {
    expect(
      validateDoc(
        doc(
          paragraph(text('a '), inline('x^2'), text(' b')),
          { type: 'heading', attrs: { level: 1 }, content: [inline('E = mc^2')] },
          block('\\int_0^1 x \\, dx'),
          { type: 'blockquote', content: [block('a')] },
          {
            type: 'bulletList',
            content: [{ type: 'listItem', content: [paragraph(inline('b')), block('c')] }],
          },
          {
            type: 'taskList',
            content: [
              {
                type: 'taskItem',
                attrs: { checked: false },
                content: [paragraph(inline('d')), block('e')],
              },
            ],
          },
        ),
      ),
    ).toEqual([]);
  });

  it('takes a source of one character up to the limit, and no other', () => {
    expect(MAX_LATEX).toBe(4000);
    expect(validateDoc(doc(block('x')))).toEqual([]);
    expect(validateDoc(doc(block('x'.repeat(MAX_LATEX))))).toEqual([]);
    expect(validateDoc(doc(paragraph(inline('x'.repeat(MAX_LATEX)))))).toEqual([]);
    for (const latex of ['', 'x'.repeat(MAX_LATEX + 1), 7, null, ['x'], { latex: 'x' }, true]) {
      expect([latex, validateDoc(doc(block(latex))).length > 0]).toEqual([latex, true]);
      expect([latex, validateDoc(doc(paragraph(inline(latex)))).length > 0]).toEqual([latex, true]);
    }
    // The limit counts UTF-16 code units, as every limit on text here does.
    expect(validateDoc(doc(block('😀'.repeat(MAX_LATEX / 2))))).toEqual([]);
    expect(validateDoc(doc(block(`${'😀'.repeat(MAX_LATEX / 2)}x`))).length).toBeGreaterThan(0);
  });

  it.each<[string, DocNode, string]>([
    ['a formula without a source', doc({ type: 'blockMath' }), '"latex" is missing'],
    [
      'an inline formula without a source',
      doc(paragraph({ type: 'inlineMath' })),
      '"latex" is missing',
    ],
    [
      'another attribute',
      doc({ type: 'blockMath', attrs: { latex: 'x', display: true } }),
      'unknown attribute "display"',
    ],
    [
      'a drawn formula stored next to the source',
      doc(paragraph({ type: 'inlineMath', attrs: { latex: 'x', html: '<span>x</span>' } })),
      'unknown attribute "html"',
    ],
    ['an inline formula as a block', doc(inline('x')), 'not allowed inside'],
    ['a block formula in a line', doc(paragraph(block('x'))), 'not allowed inside'],
    [
      'a formula in a code block',
      doc({ type: 'codeBlock', content: [inline('x')] }),
      'not allowed inside',
    ],
    [
      'a block formula first in a list entry',
      doc({ type: 'bulletList', content: [{ type: 'listItem', content: [block('x')] }] }),
      'has to start with a paragraph',
    ],
    [
      'content inside a formula',
      doc({ type: 'blockMath', attrs: { latex: 'x' }, content: [text('y')] }),
      'cannot have content',
    ],
    [
      'marks on a formula',
      doc(paragraph({ type: 'inlineMath', attrs: { latex: 'x' }, marks: [{ type: 'bold' }] })),
      'only text nodes have marks',
    ],
    [
      'text on a formula',
      doc(paragraph({ type: 'inlineMath', attrs: { latex: 'x' }, text: 'x' })),
      'only text nodes have text',
    ],
  ])('refuses %s', (_name, body, problem) => {
    expect(validateDoc(body).join(' / ')).toContain(problem);
  });

  it('puts the source of formulas and of diagrams into the plain text', () => {
    const body = doc(
      paragraph(text('energy '), inline('E = mc^2'), text(' and')),
      block('\\frac{1}{2}'),
      {
        type: 'codeBlock',
        attrs: { language: 'mermaid' },
        content: [text('flowchart LR\n  A --> B')],
      },
      { type: 'blockquote', content: [block('q')] },
    );
    expect(plainText(body)).toBe('energy E = mc^2 and\n\\frac{1}{2}\nflowchart LR\n  A --> B\nq');
  });

  it('keeps a formula as it is in the stored form, and does not count it as an empty body', () => {
    const body = doc(
      paragraph({ type: 'inlineMath', attrs: { latex: 'x' }, marks: [{ type: 'bold' }] }),
    );
    expect(toStoredDoc(body)).toEqual(doc(paragraph(inline('x'))));
    expect(isEmptyDoc(doc(block('x')))).toBe(false);
    expect(isEmptyDoc(doc(paragraph(inline('x'))))).toBe(false);
    expect(isEmptyDoc(emptyDoc())).toBe(true);
    expect(isEmptyDoc(doc(paragraph(), paragraph()))).toBe(false);
  });
});
