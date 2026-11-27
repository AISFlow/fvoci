/**
 * The editor's extensions against the list of what a body may contain. Runs
 * a real editor in a simulated page: no layout, but the same schema, the
 * same parsing of HTML and the same serialisation as in a browser.
 */
import { Editor } from '@tiptap/core';
import { afterEach, describe, expect, it } from 'vitest';

import { bodyExtensions } from '$lib/editor/extensions.ts';
import { setFileSource } from '$lib/editor/files.ts';
import { markdownToBlocks } from '$lib/editor/markdown.ts';
import {
  MARK_SPECS,
  NODE_SPECS,
  toStoredDoc,
  validateDoc,
  type DocNode,
} from '$lib/editor/schema.ts';

let editor: Editor | undefined;

function create(content?: DocNode | string): Editor {
  editor = new Editor({
    element: document.createElement('div'),
    extensions: bodyExtensions({ mention: { search: () => [], labelOf: (id) => `name-of-${id}` } }),
    ...(content === undefined ? {} : { content }),
  });
  return editor;
}

const stored = (target: Editor): DocNode => toStoredDoc(target.getJSON() as DocNode);

afterEach(() => {
  editor?.destroy();
  editor = undefined;
});

describe('the editor and the list agree', () => {
  it('knows exactly the nodes and marks on the list', () => {
    const { schema } = create();
    expect(Object.keys(schema.nodes).sort()).toEqual(Object.keys(NODE_SPECS).sort());
    expect(Object.keys(schema.marks).sort()).toEqual(Object.keys(MARK_SPECS).sort());
  });

  it('gives every node and mark exactly the attributes on the list', () => {
    const { schema } = create();
    for (const [name, type] of Object.entries(schema.nodes)) {
      expect([name, Object.keys(type.spec.attrs ?? {}).sort()]).toEqual([
        name,
        Object.keys(NODE_SPECS[name]?.attrs ?? {}).sort(),
      ]);
    }
    for (const [name, type] of Object.entries(schema.marks)) {
      expect([name, Object.keys(type.spec.attrs ?? {}).sort()]).toEqual([
        name,
        Object.keys(MARK_SPECS[name] ?? {}).sort(),
      ]);
    }
  });

  it('allows the same nesting: what the validator accepts, the editor loads unchanged', () => {
    const markdown = [
      '# Title',
      'text with **bold**, _italic_, ~~strike~~, `code` and a [link](https://example.com).',
      '- one\n  - nested\n- two',
      '3. three\n4. four',
      '- [x] done\n- [ ] todo\n  - [ ] nested todo',
      '> quote\n>\n> - in quote',
      '```ts\nconst x = 1;\n```',
      '---',
    ].join('\n\n');
    const doc: DocNode = {
      type: 'doc',
      content: [
        ...markdownToBlocks(markdown).content,
        { type: 'image', attrs: { fileId: 'file-1', alt: 'chart' } },
      ],
    };
    expect(validateDoc(doc)).toEqual([]);
    const loaded = create(doc);
    // ProseMirror itself checks the content model here.
    loaded.state.doc.check();
    expect(stored(loaded)).toEqual(doc);
  });

  it('produces a valid document from an empty editor and after ordinary commands', () => {
    const target = create();
    expect(validateDoc(stored(target))).toEqual([]);
    target
      .chain()
      .insertContent('Hello')
      .toggleBold()
      .insertContent(' world')
      .setHardBreak()
      .toggleHeading({ level: 2 })
      .run();
    target
      .chain()
      .insertContent({ type: 'horizontalRule' })
      .toggleTaskList()
      .insertContent('task')
      .run();
    target.chain().toggleCodeBlock().insertContent('code').run();
    target.commands.setFileImage({ fileId: 'file-2' });
    target.commands.insertContent({
      type: 'mention',
      attrs: { userId: '01930000-0000-7000-8000-00000000c004' },
    });
    expect(validateDoc(stored(target))).toEqual([]);
  });
});

describe('HTML from elsewhere', () => {
  const load = (html: string): DocNode => stored(create(html));
  const types = (node: DocNode): string[] => [node.type, ...(node.content ?? []).flatMap(types)];
  const marks = (node: DocNode): string[] => [
    ...(node.marks ?? []).map((mark) => mark.type),
    ...(node.content ?? []).flatMap(marks),
  ];
  const textOf = (node: DocNode): string =>
    (node.text ?? '') + (node.content ?? []).map(textOf).join('');

  it('keeps what is on the list', () => {
    const doc = load(
      '<h2>Title</h2><p><strong>b</strong> <em>i</em> <s>s</s> <code>c</code> <a href="https://example.com">l</a></p>' +
        '<ul><li>a</li></ul><ol start="5"><li>b</li></ol><blockquote><p>q</p></blockquote><pre><code class="language-js">x</code></pre><hr>',
    );
    expect(validateDoc(doc)).toEqual([]);
    expect(types(doc).filter((type) => type !== 'text' && type !== 'paragraph')).toEqual([
      'doc',
      'heading',
      'bulletList',
      'listItem',
      'orderedList',
      'listItem',
      'blockquote',
      'codeBlock',
      'horizontalRule',
    ]);
    expect(marks(doc).sort()).toEqual(['bold', 'code', 'italic', 'link', 'strike']);
    expect(doc.content?.find((node) => node.type === 'orderedList')?.attrs).toEqual({ start: 5 });
    expect(doc.content?.find((node) => node.type === 'codeBlock')?.attrs).toEqual({
      language: 'js',
    });
  });

  it('takes no mention that names no user: the server would refuse the body', () => {
    const doc = load(
      '<p>a <span data-mention data-user-id="u1">@Kim</span> b <span data-mention data-user-id="">@</span></p>',
    );
    expect(validateDoc(doc)).toEqual([]);
    expect(JSON.stringify(doc)).not.toContain('mention');
  });

  it('takes a reference by its identifier alone, and none that names nothing', () => {
    const id = '01930000-0000-7000-8000-00000000b001';
    const doc = load(
      `<p>a <span data-reference data-block-id="${id}">Plan</span> b <span data-reference data-block-id="">Hidden</span> c <span data-reference data-block-id="b1">x</span></p>`,
    );
    expect(doc.content?.[0]?.content?.filter((node) => node.type === 'reference')).toEqual([
      { type: 'reference', attrs: { id } },
    ]);
    // What stood inside the element, a title somebody else's screen drew, is not taken.
    expect(JSON.stringify(doc)).not.toContain('Plan');
    expect(validateDoc(doc)).toEqual([]);
  });

  it('keeps marks off line breaks and mentions inside marked text', () => {
    const doc = load(
      '<p><strong>a<br>b <span data-mention data-user-id="01930000-0000-7000-8000-00000000c001"></span> c</strong></p><p><a href="/x"><em>d<br></em>e</a></p>',
    );
    expect(validateDoc(doc)).toEqual([]);
    const unmarked = (node: DocNode): DocNode[] => [
      ...(node.type === 'hardBreak' || node.type === 'mention' ? [node] : []),
      ...(node.content ?? []).flatMap(unmarked),
    ];
    expect(unmarked(doc).map((node) => node.type)).toEqual(['hardBreak', 'mention', 'hardBreak']);
    for (const node of unmarked(doc)) expect(node.marks).toBeUndefined();
    // The text on both sides keeps its marks.
    expect(marks(doc).filter((mark) => mark === 'bold')).toHaveLength(3);
  });

  it('drops scripts, frames, styles and event handlers, and keeps the text', () => {
    const doc = load(
      '<p onclick="steal()" style="color:red">before<script>alert(1)</script><iframe src="https://evil.example"></iframe>' +
        '<span style="font-size:99px">styled</span><u>underlined</u></p><style>p{display:none}</style><object data="x"></object>',
    );
    expect(validateDoc(doc)).toEqual([]);
    expect(types(doc)).toEqual(['doc', 'paragraph', 'text']);
    expect(marks(doc)).toEqual([]);
    expect(textOf(doc)).toBe('beforestyledunderlined');
    expect(JSON.stringify(doc)).not.toMatch(/alert|steal|evil|color|font-size/);
  });

  it('drops an image that names an address instead of an uploaded file', () => {
    const doc = load('<p>a</p><img src="https://tracker.example/pixel.gif" alt="x"><p>b</p>');
    expect(types(doc)).not.toContain('image');
    expect(JSON.stringify(doc)).not.toContain('tracker');
    const own = load('<img data-file-id="file-9" alt="kept" src="https://ignored.example/x.png">');
    expect(own.content).toEqual([{ type: 'image', attrs: { fileId: 'file-9', alt: 'kept' } }]);
  });

  it('drops the target of a link that is not allowed and keeps its text', () => {
    const doc = load(
      '<p><a href="javascript:alert(1)">one</a> <a href="jav&#x61;script:alert(1)">two</a> ' +
        '<a href="data:text/html,x">three</a> <a href=" https://example.com/ok">four</a></p>',
    );
    expect(validateDoc(doc)).toEqual([]);
    expect(textOf(doc)).toBe('one two three four');
    const links = (doc.content?.[0]?.content ?? []).flatMap((node) => node.marks ?? []);
    expect(links).toEqual([{ type: 'link', attrs: { href: 'https://example.com/ok' } }]);
  });

  it('keeps a table as a table, its header row too, and the document valid', () => {
    const doc = load('<table><tr><th>h</th></tr><tr><td>cell <b>bold</b></td></tr></table>');
    expect(validateDoc(doc)).toEqual([]);
    expect(types(doc)).toEqual(expect.arrayContaining(['table', 'tableHeader', 'tableCell']));
    expect(textOf(doc)).toContain('cell bold');
  });
});

describe('drawing', () => {
  it('draws an image from the address the resolver gives for its file', () => {
    const previous = setFileSource({ url: (fileId) => `/files/board-1/${fileId}` });
    const target = create({
      type: 'doc',
      content: [{ type: 'image', attrs: { fileId: 'file-1', alt: 'chart' } }],
    });
    const image = target.view.dom.querySelector('img');
    expect(image?.getAttribute('src')).toBe('/files/board-1/file-1');
    expect(image?.getAttribute('data-file-id')).toBe('file-1');
    expect(image?.getAttribute('alt')).toBe('chart');
    setFileSource(previous);
  });

  it('draws an image without an address when the file cannot be resolved', () => {
    const target = create({
      type: 'doc',
      content: [{ type: 'image', attrs: { fileId: 'unknown' } }],
    });
    expect(target.view.dom.querySelector('img')?.hasAttribute('src')).toBe(false);
  });

  it('draws a mention with the current name of the user', () => {
    const target = create({
      type: 'doc',
      content: [
        {
          type: 'paragraph',
          content: [{ type: 'mention', attrs: { userId: '01930000-0000-7000-8000-00000000c007' } }],
        },
      ],
    });
    const mention = target.view.dom.querySelector('[data-mention]');
    expect(mention?.textContent).toBe('@name-of-01930000-0000-7000-8000-00000000c007');
    expect(mention?.getAttribute('data-user-id')).toBe('01930000-0000-7000-8000-00000000c007');
    expect(target.getText()).toBe('@name-of-01930000-0000-7000-8000-00000000c007');
  });

  it('opens links in a new tab without handing over the opener, without storing that', () => {
    const target = create('<p><a href="https://example.com">x</a></p>');
    const link = target.view.dom.querySelector('a');
    expect(link?.getAttribute('target')).toBe('_blank');
    expect(link?.getAttribute('rel')).toContain('noopener');
    expect(JSON.stringify(stored(target))).not.toContain('_blank');
  });
});
