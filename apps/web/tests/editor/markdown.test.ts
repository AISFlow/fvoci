import { describe, expect, it } from 'vitest';

import { addNotes, markdownToBlocks, NOTE_KINDS } from '$lib/editor/markdown.ts';
import { validateDoc, type DocNode } from '$lib/editor/schema.ts';

const blocks = (markdown: string): DocNode[] => markdownToBlocks(markdown).content;
const notes = (markdown: string): Record<string, number> => markdownToBlocks(markdown).notes;
const valid = (content: DocNode[]): string[] => validateDoc({ type: 'doc', content });

describe('Markdown into document nodes', () => {
  it('reads paragraphs, with a single line break as a line break', () => {
    expect(blocks('first line\nsecond line\n\nnext paragraph')).toEqual([
      {
        type: 'paragraph',
        content: [
          { type: 'text', text: 'first line' },
          { type: 'hardBreak' },
          { type: 'text', text: 'second line' },
        ],
      },
      { type: 'paragraph', content: [{ type: 'text', text: 'next paragraph' }] },
    ]);
  });

  it('reads the inline marks, nested ones too', () => {
    expect(blocks('a **b _c_** ~~d~~ `e`')).toEqual([
      {
        type: 'paragraph',
        content: [
          { type: 'text', text: 'a ' },
          { type: 'text', text: 'b ', marks: [{ type: 'bold' }] },
          { type: 'text', text: 'c', marks: [{ type: 'bold' }, { type: 'italic' }] },
          { type: 'text', text: ' ' },
          { type: 'text', text: 'd', marks: [{ type: 'strike' }] },
          { type: 'text', text: ' ' },
          { type: 'text', text: 'e', marks: [{ type: 'code' }] },
        ],
      },
    ]);
  });

  it('reads links, written out and bare', () => {
    expect(blocks('[site](https://example.com/a) and https://example.com/b')).toEqual([
      {
        type: 'paragraph',
        content: [
          {
            type: 'text',
            text: 'site',
            marks: [{ type: 'link', attrs: { href: 'https://example.com/a' } }],
          },
          { type: 'text', text: ' and ' },
          {
            type: 'text',
            text: 'https://example.com/b',
            marks: [{ type: 'link', attrs: { href: 'https://example.com/b' } }],
          },
        ],
      },
    ]);
  });

  it('reads headings, lists, task lists, quotes, code and rules', () => {
    const content = blocks(
      [
        '## Title',
        '- a',
        '- b',
        '  - nested',
        '',
        '3. three',
        '4. four',
        '',
        '- [x] done',
        '- [ ] todo',
        '',
        '> quoted',
        '',
        '```js',
        'let x;',
        '```',
        '',
        '---',
      ].join('\n'),
    );
    expect(content.map((node) => node.type)).toEqual([
      'heading',
      'bulletList',
      'orderedList',
      'taskList',
      'blockquote',
      'codeBlock',
      'horizontalRule',
    ]);
    expect(content[0]).toEqual({
      type: 'heading',
      attrs: { level: 2 },
      content: [{ type: 'text', text: 'Title' }],
    });
    expect(content[1]?.content?.[1]?.content?.map((node) => node.type)).toEqual([
      'paragraph',
      'bulletList',
    ]);
    expect(content[2]?.attrs).toEqual({ start: 3 });
    expect(content[3]?.content?.map((item) => item.attrs)).toEqual([
      { checked: true },
      { checked: false },
    ]);
    expect(content[5]).toEqual({
      type: 'codeBlock',
      attrs: { language: 'js' },
      content: [{ type: 'text', text: 'let x;' }],
    });
    expect(valid(content)).toEqual([]);
  });

  it('returns nothing for empty text', () => {
    expect(markdownToBlocks('')).toEqual({ content: [], notes: {} });
    expect(blocks('   \n\n')).toEqual([]);
  });
});

describe('what a document cannot say is counted', () => {
  it('shows HTML as the characters it is, never as markup', () => {
    expect(blocks('a <b>x</b>')).toEqual([
      { type: 'paragraph', content: [{ type: 'text', text: 'a <b>x</b>' }] },
    ]);
    expect(notes('a <b>x</b>')).toEqual({ 'html-as-text': 2 });
    expect(blocks('<div onclick="x()">block</div>')[0]?.content?.[0]?.text).toBe(
      '<div onclick="x()">block</div>',
    );
  });

  it('lowers headings below level three', () => {
    expect(blocks('##### deep')[0]?.attrs).toEqual({ level: 3 });
    expect(notes('##### deep')).toEqual({ 'heading-level-clamped': 1 });
  });

  it('makes a table, its first row of headers and every row as wide', () => {
    const table = '| a | b |\n|---|---|\n| 1 | **2** |\n| 3 |';
    const cell = (type: string, content: DocNode[] = []): DocNode => ({
      type,
      content: [content.length > 0 ? { type: 'paragraph', content } : { type: 'paragraph' }],
    });
    const text = (value: string, marks?: DocNode['marks']): DocNode =>
      marks ? { type: 'text', text: value, marks } : { type: 'text', text: value };
    expect(blocks(table)).toEqual([
      {
        type: 'table',
        content: [
          {
            type: 'tableRow',
            content: [cell('tableHeader', [text('a')]), cell('tableHeader', [text('b')])],
          },
          {
            type: 'tableRow',
            content: [
              cell('tableCell', [text('1')]),
              cell('tableCell', [text('2', [{ type: 'bold' }])]),
            ],
          },
          { type: 'tableRow', content: [cell('tableCell', [text('3')]), cell('tableCell')] },
        ],
      },
    ]);
    expect(notes(table)).toEqual({});
    expect(valid(blocks(table))).toEqual([]);
    // How its columns were aligned is not kept.
    expect(notes('| a | b |\n|:--|--:|\n| 1 | 2 |')).toEqual({ 'table-alignment-dropped': 1 });
  });

  it('turns a table past 200 rows or 20 columns into a paragraph per row', () => {
    const wide = `|${' c |'.repeat(21)}\n|${'---|'.repeat(21)}\n|${' 1 |'.repeat(21)}`;
    const lines = blocks(wide);
    expect(lines).toHaveLength(2);
    expect(lines[0]).toEqual({
      type: 'paragraph',
      content: [{ type: 'text', text: Array.from({ length: 21 }, () => 'c').join(' | ') }],
    });
    expect(notes(wide)).toEqual({ table: 1 });
    const long = `| h |\n|---|\n${'| r |\n'.repeat(200)}`;
    expect(blocks(long).map((one) => one.type)).toEqual(
      Array.from({ length: 201 }, () => 'paragraph'),
    );
    expect(notes(long)).toEqual({ table: 1 });
    const fits = `| h |\n|---|\n${'| r |\n'.repeat(199)}`;
    expect(blocks(fits)[0]?.content).toHaveLength(200);
  });

  it('turns a table in a list entry or a quote into a paragraph per row, as a table stands only in the document', () => {
    const nested = [
      '1. step',
      '',
      '   | c | d |',
      '   |---|---|',
      '   | 3 | 4 |',
      '',
      '> | e |',
      '> |---|',
      '> | 5 |',
    ].join('\n');
    const line = (text: string): DocNode => ({
      type: 'paragraph',
      content: [{ type: 'text', text }],
    });
    expect(blocks(nested)).toEqual([
      {
        type: 'orderedList',
        attrs: { start: 1 },
        content: [{ type: 'listItem', content: [line('step'), line('c | d'), line('3 | 4')] }],
      },
      { type: 'blockquote', content: [line('e'), line('5')] },
    ]);
    expect(notes(nested)).toEqual({ table: 2 });
    expect(valid(blocks(nested))).toEqual([]);
    // Out of them again, a table is a table.
    expect(blocks(`> quoted\n\n| f |\n|---|\n| 6 |`).map((one) => one.type)).toEqual([
      'blockquote',
      'table',
    ]);
  });

  it('turns an image by address into a link', () => {
    expect(blocks('![a chart](https://example.com/c.png)')).toEqual([
      {
        type: 'paragraph',
        content: [
          {
            type: 'text',
            text: 'a chart',
            marks: [{ type: 'link', attrs: { href: 'https://example.com/c.png' } }],
          },
        ],
      },
    ]);
    expect(notes('![a chart](https://example.com/c.png)')).toEqual({ 'image-by-address': 1 });
  });

  it('keeps the text of a link whose target is not allowed', () => {
    expect(blocks('[click](javascript:alert(1))')).toEqual([
      { type: 'paragraph', content: [{ type: 'text', text: 'click' }] },
    ]);
    expect(notes('[click](javascript:alert(1))')).toEqual({ 'link-dropped': 1 });
    expect(notes('[a](https://example.com "tip")')).toEqual({ 'link-title-dropped': 1 });
  });

  it('lets inline code stand alone', () => {
    expect(blocks('**`x`**')).toEqual([
      { type: 'paragraph', content: [{ type: 'text', text: 'x', marks: [{ type: 'code' }] }] },
    ]);
    expect(notes('**`x`**')).toEqual({ 'marks-dropped-in-code': 1 });
  });

  it('writes the boxes out when a list mixes tasks and plain entries', () => {
    const content = blocks('- [x] task\n- plain');
    expect(content[0]?.type).toBe('bulletList');
    expect(content[0]?.content?.[0]?.content?.[0]?.content?.[0]?.text).toBe('[x] task');
    expect(notes('- [x] task\n- plain')).toEqual({ 'mixed-task-list': 1 });
    expect(notes('1. [ ] numbered task')).toEqual({ 'ordered-task-list': 1 });
  });

  it('always produces what the server accepts', () => {
    const awkward = [
      '# h\n\n- \n\n> \n\n```\n```\n\n1. a\n\n   para in item\n\n- [ ] \n',
      '<script>alert(1)</script>\n\n[x](data:text/html,x) ![y](javascript:1)',
      '* a\n\n  ```c++\n  code\n  ```\n\n| t |\n|---|\n',
      '```not a language!\nx\n```',
    ];
    for (const markdown of awkward) expect(valid(blocks(markdown))).toEqual([]);
  });

  it('only uses the documented kinds of note', () => {
    const total: Record<string, number> = {};
    addNotes(total, notes('<b>x</b> ##### h'));
    addNotes(total, notes('| a |\n|:-:|\n\n<i>y</i>'));
    addNotes(total, notes(`|${' c |'.repeat(21)}\n|${'---|'.repeat(21)}`));
    expect(total).toEqual({ 'html-as-text': 4, table: 1, 'table-alignment-dropped': 1 });
    for (const kind of Object.keys(total)) expect(NOTE_KINDS).toContain(kind);
  });
});

describe('toggles and callouts', () => {
  const text = (words: string, marks?: DocNode['marks']): DocNode =>
    marks ? { type: 'text', text: words, marks } : { type: 'text', text: words };
  const line = (...content: DocNode[]): DocNode =>
    content.length > 0 ? { type: 'paragraph', content } : { type: 'paragraph' };
  const toggle = (summary: DocNode[], ...hidden: DocNode[]): DocNode => ({
    type: 'details',
    content: [
      summary.length > 0
        ? { type: 'detailsSummary', content: summary }
        : { type: 'detailsSummary' },
      { type: 'detailsContent', content: hidden },
    ],
  });
  const callout = (tone: string, ...content: DocNode[]): DocNode => ({
    type: 'callout',
    attrs: { tone },
    content,
  });
  const cell = (type: string, words: string): DocNode => ({ type, content: [line(text(words))] });

  it('reads the Markdown the server writes them in', () => {
    // As crates/server/src/app/body_markdown/write.rs writes them.
    const written = [
      '<details>',
      '<summary>Why **this** &lt;now&gt; &amp; [then](https://example.com/?a=1&amp;b=2)?</summary>',
      '',
      'Because.',
      '',
      '> [!TIP]',
      '> inside',
      '',
      '<details>',
      '<summary></summary>',
      '',
      'deeper',
      '',
      '</details>',
      '',
      '</details>',
      '',
      '<details>',
      '<summary>nothing inside</summary>',
      '',
      '</details>',
      '',
      '> [!WARNING]',
      '> ```',
      '> rm -rf',
      '> ```',
      '',
      '> [!CAUTION]',
      '',
      '> \\[!NOTE\\]',
      '>',
      '> a quote',
    ].join('\n');
    const content = blocks(written);
    expect(content).toEqual([
      toggle(
        [
          text('Why '),
          text('this', [{ type: 'bold' }]),
          text(' <now> & '),
          text('then', [{ type: 'link', attrs: { href: 'https://example.com/?a=1&b=2' } }]),
          text('?'),
        ],
        line(text('Because.')),
        callout('tip', line(text('inside'))),
        toggle([], line(text('deeper'))),
      ),
      toggle([text('nothing inside')], line()),
      callout('warn', { type: 'codeBlock', content: [text('rm -rf')] }),
      callout('danger', line()),
      { type: 'blockquote', content: [line(text('[!NOTE]')), line(text('a quote'))] },
    ]);
    expect(valid(content)).toEqual([]);
    expect(notes(written)).toEqual({});
    // A `</summary>` after a backslash of its own is text of the summary.
    expect(
      blocks('<details>\n<summary>ends at \\</summary> \\\\</summary>\n\nnot yet\n\n</details>'),
    ).toEqual([toggle([text('ends at </summary> \\')], line(text('not yet')))]);
  });

  it('counts no tag of a toggle in the text of a summary', () => {
    // The server writes each of these; its reader reads them as these toggles.
    expect(
      blocks('<details>\n<summary>a \\</details> b</summary>\n\nhidden\n\n</details>\n\nafter'),
    ).toEqual([toggle([text('a </details> b')], line(text('hidden'))), line(text('after'))]);
    expect(
      blocks(
        '<details>\n<summary>a \\<details> b</summary>\n\nhidden\n\n</details>\n\nafter one\n\nafter two',
      ),
    ).toEqual([
      toggle([text('a <details> b')], line(text('hidden'))),
      line(text('after one')),
      line(text('after two')),
    ]);
    expect(
      blocks('<details>\n<summary>a `</details>` b</summary>\n\nhidden\n\n</details>\n\nafter'),
    ).toEqual([
      toggle(
        [text('a '), text('</details>', [{ type: 'code' }]), text(' b')],
        line(text('hidden')),
      ),
      line(text('after')),
    ]);
  });

  it('reads a toggle in 28 others as its lines, so that a body stays within its depth', () => {
    const nested = (count: number, inner: string): string =>
      count === 0
        ? inner
        : `<details>\n<summary>${String(count)}</summary>\n\n${nested(count - 1, inner)}\n\n</details>`;
    const deepest = (node: DocNode | undefined, depth = 0): number =>
      node?.type === 'details' ? deepest(node.content?.[1]?.content?.[0], depth + 1) : depth;
    const fits = blocks(nested(28, 'words'));
    expect(deepest(fits[0])).toBe(28);
    expect(valid(fits)).toEqual([]);
    const over = markdownToBlocks(nested(29, 'words'));
    expect(deepest(over.content[0])).toBe(28);
    expect(valid(over.content)).toEqual([]);
    expect(over.notes).toEqual({ toggle: 1 });
    // Far deeper reads quickly, and as deep.
    const started = performance.now();
    const far = blocks(nested(3000, 'words'));
    expect(performance.now() - started).toBeLessThan(5000);
    expect(deepest(far[0])).toBe(28);
    expect(valid(far)).toEqual([]);
  });

  it('reads alerts and toggles from elsewhere where they may stand, as the server does', () => {
    // The same Markdown as the server's test of it (body_markdown/tests.rs).
    const markdown = [
      '> [!IMPORTANT]\n> heads up',
      '> [!tip]',
      '<details open><summary>Q</summary>\n\nA\n</details>',
      '- item\n\n  <details>\n  <summary>S</summary>\n\n  hidden\n\n  </details>',
      '> [!NOTE]\n> > [!TIP]\n> > inner',
      '</details>',
      '<details>\n<summary>left open</summary>\n\n| a |\n| - |\n| 1 |',
    ].join('\n\n');
    expect(blocks(markdown)).toEqual([
      callout('info', line(text('heads up'))),
      callout('tip', line()),
      toggle([text('Q')], line(text('A'))),
      {
        type: 'bulletList',
        content: [
          {
            type: 'listItem',
            content: [line(text('item')), line(text('S')), line(text('hidden'))],
          },
        ],
      },
      callout('info', { type: 'blockquote', content: [line(text('inner'))] }),
      line(text('</details>')),
      toggle([text('left open')], {
        type: 'table',
        content: [
          { type: 'tableRow', content: [cell('tableHeader', 'a')] },
          { type: 'tableRow', content: [cell('tableCell', '1')] },
        ],
      }),
    ]);
    expect(notes(markdown)).toEqual({ toggle: 1, callout: 1, 'html-as-text': 1 });
  });

  it('reads a toggle or a table in a callout as its parts, as the server does', () => {
    const markdown =
      '> [!WARNING]\n> <details>\n> <summary>S</summary>\n>\n> x\n>\n> </details>\n>\n> | a |\n> | - |\n> | 1 |';
    expect(blocks(markdown)).toEqual([
      callout('warn', line(text('S')), line(text('x')), line(text('a')), line(text('1'))),
    ]);
    expect(notes(markdown)).toEqual({ toggle: 1, table: 1 });
  });
});
