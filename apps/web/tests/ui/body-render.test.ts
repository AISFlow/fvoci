/**
 * The read-only view of a body on its own: what it draws, what it refuses,
 * and that nothing a document says can become markup, a script or an
 * attribute that is not the view's own.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { setFileSource } from '$lib/editor/files.ts';
import { renderBody, type RenderedBody } from '$lib/editor/render.ts';
import {
  isAllowedHref,
  MAX_DEPTH,
  MAX_NODES,
  validateDoc,
  type DocNode,
} from '$lib/editor/schema.ts';

import { doc, image, link, mention, paragraph, task, tasks, text } from '../support/body-docs.ts';

/** Every element the view may create, and the attributes each may carry. */
const ALLOWED: Record<string, string[]> = {
  p: [],
  h1: [],
  h2: [],
  h3: [],
  ul: ['data-type'],
  ol: ['start'],
  li: ['data-checked'],
  label: [],
  input: ['type', 'aria-label', 'aria-readonly'],
  // A `<div>` is the content of a task, or the place of a block formula.
  div: ['data-type', 'data-latex', 'class'],
  blockquote: [],
  // How wide the room for the numbers of a code block's lines is: digits, set from its text's lines.
  pre: ['data-line-digits'],
  code: ['class'],
  hr: [],
  br: [],
  img: ['data-file-id', 'alt', 'loading', 'decoding', 'src'],
  // A mention, a reference, or the place of a formula in a line.
  span: [
    'data-user-id',
    'data-mention',
    'data-block-id',
    'data-reference',
    'class',
    'data-type',
    'data-latex',
  ],
  strong: [],
  em: [],
  s: [],
  a: ['href', 'target', 'rel'],
};

const host = document.createElement('div');
document.body.append(host);

afterEach(() => {
  host.replaceChildren();
  delete (window as { pwned?: unknown }).pwned;
});

/** Puts a drawn body into the page, as a caller would, and returns where it went. */
function place(rendered: RenderedBody): HTMLElement {
  if (!rendered.ok) throw new Error(`not drawn: ${rendered.problems.join(' / ')}`);
  host.replaceChildren(rendered.fragment);
  return host;
}

/** Fails when the drawn body holds anything the view has no business creating. */
function expectHarmless(root: ParentNode): void {
  for (const element of Array.from(root.querySelectorAll('*'))) {
    const tag = element.tagName.toLowerCase();
    const allowed = ALLOWED[tag];
    expect([tag, allowed !== undefined]).toEqual([tag, true]);
    for (const { name } of Array.from(element.attributes)) {
      expect([tag, name, allowed?.includes(name)]).toEqual([tag, name, true]);
    }
    if (element instanceof HTMLAnchorElement) {
      expect(isAllowedHref(element.getAttribute('href'))).toBe(true);
      // What the address resolves to in the page, not just how it is spelled.
      expect(['http:', 'https:', 'mailto:']).toContain(element.protocol);
      expect(element.getAttribute('target')).toBe('_blank');
      expect(element.getAttribute('rel')).toBe('noopener noreferrer nofollow');
    }
  }
  expect((window as { pwned?: unknown }).pwned).toBeUndefined();
}

const linked = (href: unknown): DocNode =>
  doc(
    paragraph({
      type: 'text',
      text: 'click me',
      marks: [{ type: 'link', attrs: { href } }],
    }),
  );

/** Something that claims to be a node and is not shaped like one. */
const loose = (node: Record<string, unknown>): DocNode => node as unknown as DocNode;

describe('what is drawn', () => {
  it('draws an empty body as nothing, and says it is empty', () => {
    for (const nothing of [null, undefined, doc(paragraph())]) {
      const rendered = renderBody(nothing);
      expect(rendered).toMatchObject({ ok: true, empty: true });
      expect(rendered.ok && rendered.fragment.childNodes).toHaveLength(0);
    }
    // Two empty paragraphs are something: somebody made room on purpose.
    expect(renderBody(doc(paragraph(), paragraph()))).toMatchObject({ ok: true, empty: false });
  });

  it('draws into the document it is given', () => {
    const other = document.implementation.createHTMLDocument('other');
    const rendered = renderBody(doc(paragraph(text('x'))), { document: other });
    expect(rendered.ok && rendered.fragment.ownerDocument).toBe(other);
    expect(rendered.ok && rendered.fragment.firstChild?.ownerDocument).toBe(other);
    const empty = renderBody(null, { document: other });
    expect(empty.ok && empty.fragment.ownerDocument).toBe(other);
  });

  it('names a mentioned user through the caller and falls back to the identifier', () => {
    const body = place(
      renderBody(doc(paragraph(mention('u1'), mention('u2'), mention('u3'))), {
        resolveUser: (userId) => ({ u1: '김하늘', u3: '' })[userId],
      }),
    );
    const mentions = Array.from(body.querySelectorAll('span.mention'));
    expect(mentions.map((element) => element.textContent)).toEqual(['@김하늘', '@u2', '@u3']);
    expect(mentions.map((element) => element.getAttribute('data-user-id'))).toEqual([
      'u1',
      'u2',
      'u3',
    ]);
    // Without a resolver at all, as the editor does without a list of people.
    const bare = place(renderBody(doc(paragraph(mention('u1')))));
    expect(bare.textContent).toBe('@u1');
  });

  it('draws a mention that names nobody (as an anonymous reader gets it) through the caller, though it may not be written', () => {
    const anonymous = doc(paragraph(text('ask '), mention('')));
    expect(validateDoc(anonymous)).not.toEqual([]);
    expect(validateDoc(anonymous, { shown: true })).toEqual([]);
    const body = place(
      renderBody(anonymous, { resolveUser: (userId) => (userId ? undefined : 'Unknown user') }),
    );
    expect(body.querySelector('span.mention')?.textContent).toBe('@Unknown user');
  });

  it('draws an image from the address of its file, lazily', () => {
    const previous = setFileSource({
      url: (fileId) => (fileId === 'known' ? '/files/board-1/known' : undefined),
    });
    try {
      const body = place(renderBody(doc(image('known', '진행 그래프'), image('unknown'))));
      const [known, unknown] = Array.from(body.querySelectorAll('img'));
      expect(known?.getAttribute('src')).toBe('/files/board-1/known');
      expect(known?.getAttribute('alt')).toBe('진행 그래프');
      expect(known?.getAttribute('loading')).toBe('lazy');
      expect(known?.getAttribute('decoding')).toBe('async');
      expect(known?.getAttribute('data-file-id')).toBe('known');
      // No address: the image keeps its place and has no `src` and no `alt` it was not given.
      expect(unknown?.hasAttribute('src')).toBe(false);
      expect(unknown?.hasAttribute('alt')).toBe(false);
      expect(unknown?.getAttribute('data-file-id')).toBe('unknown');
      expect(unknown?.getAttribute('loading')).toBe('lazy');
    } finally {
      setFileSource(previous);
    }
  });

  it('shows the state of a task and does not let it change', () => {
    const body = place(
      renderBody(
        doc(
          tasks(
            task(true, paragraph(text('번역 검수'))),
            task(
              false,
              paragraph(text('for '), mention('u1'), text(' today')),
              paragraph(text('more')),
            ),
            task(false, paragraph()),
          ),
        ),
        { resolveUser: () => 'Ada' },
      ),
    );
    const boxes = Array.from(body.querySelectorAll('input'));
    expect(boxes.map((box) => box.checked)).toEqual([true, false, false]);
    expect(boxes.map((box) => box.type)).toEqual(['checkbox', 'checkbox', 'checkbox']);
    // Still a real checkbox that is not disabled: its state is read out, and it looks as in the editor.
    expect(boxes.map((box) => box.disabled)).toEqual([false, false, false]);
    expect(boxes.map((box) => box.getAttribute('aria-readonly'))).toEqual(['true', 'true', 'true']);
    // Named by the first paragraph of the task; a task without text has no name to give.
    expect(boxes.map((box) => box.getAttribute('aria-label'))).toEqual([
      '번역 검수',
      'for @Ada today',
      null,
    ]);
    expect(
      Array.from(body.querySelectorAll('li')).map((item) => item.getAttribute('data-checked')),
    ).toEqual(['true', 'false', 'false']);

    for (const box of boxes) {
      const before = box.checked;
      box.click();
      expect(box.checked).toBe(before);
      box.parentElement?.click(); // the label
      expect(box.checked).toBe(before);
      box.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
      expect(box.checked).toBe(before);
    }
  });

  it('lets the caller word the name of a task', () => {
    const body = place(
      renderBody(doc(tasks(task(true, paragraph(text('a'))), task(false, paragraph()))), {
        taskLabel: (label, checked) => `${checked ? '끝남' : '할 일'}: ${label || '(비어 있음)'}`,
      }),
    );
    expect(
      Array.from(body.querySelectorAll('input')).map((box) => box.getAttribute('aria-label')),
    ).toEqual(['끝남: a', '할 일: (비어 있음)']);
  });
});

describe('what is refused', () => {
  const refused: [string, unknown][] = [
    ['a string', '{"type":"doc"}'],
    ['a number', 7],
    ['a list', [doc(paragraph())]],
    ['an empty object', {}],
    ['a document without content', { type: 'doc' }],
    ['a node that is not the root', paragraph(text('x'))],
    ['an unknown node', doc({ type: 'script', content: [text('alert(1)')] })],
    ['an unknown node inside a known one', doc(paragraph({ type: 'iframe' }))],
    ['a table', doc({ type: 'table', content: [{ type: 'tableRow' }] })],
    ['an unknown mark', doc(paragraph(text('x', 'underline')))],
    ['a mark named like a built-in member', doc(paragraph(text('x', 'constructor')))],
    ['a mark named __proto__', doc(paragraph(text('x', '__proto__')))],
    ['a node named like a built-in member', doc({ type: 'toString' })],
    ['an unknown attribute on a node', doc({ type: 'paragraph', attrs: { onclick: 'alert(1)' } })],
    ['a style on a node', doc({ type: 'heading', attrs: { level: 1, style: 'position:fixed' } })],
    [
      'a link with an attribute beyond its address',
      doc(
        paragraph({
          type: 'text',
          text: 'x',
          marks: [{ type: 'link', attrs: { href: '/ok', onclick: 'alert(1)' } }],
        }),
      ),
    ],
    [
      'a link with a target of its own',
      doc(
        paragraph({
          type: 'text',
          text: 'x',
          marks: [{ type: 'link', attrs: { href: '/ok', target: '_top' } }],
        }),
      ),
    ],
    [
      'a member beyond the five of a node',
      doc(loose({ type: 'paragraph', innerHTML: '<b>x</b>' })),
    ],
    ['a heading of level 4', doc({ type: 'heading', attrs: { level: 4 } })],
    ['a heading inside a paragraph', doc(paragraph({ type: 'heading', attrs: { level: 1 } }))],
    ['an image inside a paragraph', doc(paragraph(image('f')))],
    ['an image without a file', doc({ type: 'image' })],
    [
      'an image with an address',
      doc({ type: 'image', attrs: { fileId: 'f', src: 'https://tracker.example/x.gif' } }),
    ],
    ['a task without its state', doc(tasks({ type: 'taskItem', content: [paragraph()] }))],
    [
      'a task whose state is a word',
      doc(tasks({ type: 'taskItem', attrs: { checked: 'true' }, content: [paragraph()] })),
    ],
    ['text with nothing in it', doc(paragraph({ type: 'text', text: '' }))],
    [
      'marks on something that is not text',
      doc(paragraph({ type: 'hardBreak', marks: [{ type: 'bold' }] })),
    ],
    // Shapes the validator trips over instead of naming: still a refusal, never an exception.
    ['marks that are a number', doc(paragraph(loose({ type: 'text', text: 'x', marks: 5 })))],
    ['a mark that is nothing', doc(paragraph(loose({ type: 'text', text: 'x', marks: [null] })))],
    ['content with a hole in it', doc(loose({ type: 'codeBlock', content: [null] }))],
    ['attributes that are a number', doc(loose({ type: 'heading', attrs: 7 }))],
    [
      'an attribute named like a built-in member',
      doc({ type: 'paragraph', attrs: { constructor: 1 } }),
    ],
  ];

  it.each(refused)('refuses %s and creates no node for it', (_name, body) => {
    const created = [
      vi.spyOn(document, 'createElement'),
      vi.spyOn(document, 'createTextNode'),
      vi.spyOn(document, 'createDocumentFragment'),
    ];
    try {
      const rendered = renderBody(body);
      expect(rendered.ok).toBe(false);
      expect(rendered.ok ? [] : rendered.problems).not.toHaveLength(0);
      expect(rendered).not.toHaveProperty('fragment');
      for (const spy of created) expect(spy).not.toHaveBeenCalled();
    } finally {
      for (const spy of created) spy.mockRestore();
    }
  });

  it('refuses a document that is too deep or too large, and draws one at the limit', () => {
    const nested = (levels: number): DocNode => {
      // `doc` is level 1 and the paragraph the last: `levels - 2` quotes between them.
      let inner: DocNode = paragraph();
      for (let level = 0; level < levels - 2; level += 1)
        inner = { type: 'blockquote', content: [inner] };
      return doc(inner);
    };
    const atLimit = renderBody(nested(MAX_DEPTH));
    expect(atLimit.ok).toBe(true);
    expect(atLimit.ok && atLimit.fragment.querySelectorAll('blockquote')).toHaveLength(
      MAX_DEPTH - 2,
    );
    expect(renderBody(nested(MAX_DEPTH + 1)).ok).toBe(false);
    expect(renderBody(nested(5000)).ok).toBe(false);

    const paragraphs = (nodes: number): DocNode =>
      doc(...Array.from({ length: nodes - 1 }, () => paragraph()));
    const full = renderBody(paragraphs(MAX_NODES));
    expect(full.ok && full.fragment.childNodes).toHaveLength(MAX_NODES - 1);
    expect(renderBody(paragraphs(MAX_NODES + 1)).ok).toBe(false);
  });

  it.each([
    ['a language with markup in it', '"><script>alert(1)</script>'],
    ['a language with a space, which would be a second class', 'ts ProseMirror-selectednode'],
    ['a language with an event handler', 'ts" onclick="alert(1)'],
    ['a language that is too long', 'a'.repeat(41)],
    ['an empty language', ''],
    ['a language that is a number', 7],
    ['a language that is an object', { toString: 'ts' }],
  ])('refuses a code block with %s', (_name, language) => {
    expect(renderBody(doc({ type: 'codeBlock', attrs: { language } })).ok).toBe(false);
  });

  it.each([
    ['a description that is a number', 7],
    ['a description that is an object', { alt: 'x' }],
    ['a description that is too long', 'a'.repeat(1001)],
  ])('refuses an image with %s', (_name, alt) => {
    expect(renderBody(doc({ type: 'image', attrs: { fileId: 'f', alt } })).ok).toBe(false);
  });
});

describe('addresses of links', () => {
  it.each([
    ['javascript:alert(1)'],
    ['JavaScript:alert(1)'],
    ['JAVASCRIPT:alert(1)'],
    ['data:text/html,<script>alert(1)</script>'],
    ['DATA:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg=='],
    ['vbscript:msgbox(1)'],
    ['VBScript:msgbox(1)'],
    ['java\tscript:alert(1)'],
    ['java\nscript:alert(1)'],
    ['java\rscript:alert(1)'],
    ['java\u0000script:alert(1)'],
    ['\u0001javascript:alert(1)'],
    [' javascript:alert(1)'],
    ['\tjavascript:alert(1)'],
    ['javascript\t:alert(1)'],
    ['//evil.example/path'],
    ['/\t/evil.example/path'],
    [' //evil.example'],
    ['\\\\evil.example\\share'],
    ['/\\evil.example'],
    ['\\/evil.example'],
    ['/path\\..\\elsewhere'],
    ['file:///etc/passwd'],
    ['blob:https://example.com/1'],
    ['ftp://example.com/'],
    ['tel:+123'],
    ['intent://example#Intent;scheme=javascript;end'],
    [''],
    [' \t\n'],
    [`https://example.com/${'a'.repeat(2048)}`],
  ])('refuses the document with a link to %j', (href) => {
    const rendered = renderBody(linked(href));
    expect(rendered.ok).toBe(false);
    expect(rendered).not.toHaveProperty('fragment');
  });

  it.each([[7], [null], [{ href: 'https://example.com' }], [['https://example.com']], [true]])(
    'refuses the document with a link whose address is %j',
    (href) => {
      expect(renderBody(linked(href)).ok).toBe(false);
    },
  );

  it.each([
    // A character reference is only a spelling inside markup. Here the address
    // is never markup, so these stay the odd relative paths they are.
    ['jav&#x61;script:alert(1)'],
    ['&#106;avascript:alert(1)'],
    ['javascript&colon;alert(1)'],
    ['jav&#x09;ascript:alert(1)'],
    ['&#x6A;&#x61;&#x76;&#x61;script:alert(1)'],
    // Not a scheme: nothing before the colon that could be one.
    ['　javascript:alert(1)'],
    ['./javascript:alert(1)'],
    ['?next=javascript:alert(1)'],
    ['#javascript:alert(1)'],
  ])('draws a link to %j as a path inside the app, never as a script', (href) => {
    const body = place(renderBody(linked(href)));
    const anchor = body.querySelector('a');
    // The attribute holds the address letter for letter: nothing was decoded.
    expect(anchor?.getAttribute('href')).toBe(href);
    expect(anchor?.protocol).toBe(window.location.protocol);
    expect(anchor?.host).toBe(window.location.host);
    expect(anchor?.textContent).toBe('click me');
    expectHarmless(body);
  });

  it.each([
    ['https://example.com/a?b=1&c=2#d'],
    ['http://example.com'],
    ['HTTPS://EXAMPLE.COM/'],
    ['mailto:someone@example.com'],
    ['/board/1/card/2'],
    ['card/2'],
    ['?view=3'],
    ['#heading'],
    ['/path with spaces'],
  ])('draws a link to %j', (href) => {
    const body = place(renderBody(linked(href)));
    expect(body.querySelector('a')?.getAttribute('href')).toBe(href);
    expectHarmless(body);
  });

  it('keeps quotes and angle brackets in an address inside the attribute', () => {
    const href =
      'https://example.com/"onmouseover="window.pwned=1" x="<script>window.pwned=1</script>';
    const body = place(renderBody(linked(href)));
    const anchor = body.querySelector('a');
    expect(anchor?.getAttribute('href')).toBe(href);
    expect(anchor?.getAttributeNames().sort()).toEqual(['href', 'rel', 'target']);
    expect(body.querySelector('script')).toBeNull();
    expectHarmless(body);
  });
});

describe('text that looks like markup', () => {
  const HOSTILE = [
    '<script>window.pwned = true</script>',
    '<img src=x onerror="window.pwned = true">',
    '"><svg onload="window.pwned = true">',
    '</p></div><iframe src="javascript:window.parent.pwned = true"></iframe>',
    '<a href="javascript:window.pwned = true">x</a>',
    '&lt;script&gt;window.pwned = true&lt;/script&gt;',
    '<!-- --><style>*{display:none}</style>',
    '\u0000<b>null</b>',
  ];

  it.each(HOSTILE.map((value) => [value]))('draws %j as the letters it is', (value) => {
    const body = place(
      renderBody(
        doc(
          paragraph(text(value), text(value, 'bold', link('/x')), text(value, 'code')),
          { type: 'heading', attrs: { level: 2 }, content: [text(value)] },
          { type: 'codeBlock', attrs: { language: 'html' }, content: [text(value)] },
          tasks(task(false, paragraph(text(value)))),
        ),
      ),
    );
    expect(body.querySelector('p')?.textContent).toBe(value.repeat(3));
    expect(body.querySelector('h2')?.textContent).toBe(value);
    expect(body.querySelector('pre > code')?.textContent).toBe(value);
    // The name of the task's checkbox carries the text too, as an attribute value.
    expect(body.querySelector('input')?.getAttribute('aria-label')).toBe(value.trim());
    expect(body.querySelector('script, iframe, svg, style, b')).toBeNull();
    expectHarmless(body);
  });

  it.each(HOSTILE.map((value) => [value]))(
    'keeps %j inside the attributes it is given for',
    (value) => {
      const previous = setFileSource({ url: (fileId) => `/files/${encodeURIComponent(fileId)}` });
      try {
        const body = place(
          renderBody(doc(paragraph(mention(value)), image(value, value)), {
            resolveUser: () => value,
          }),
        );
        const span = body.querySelector('span');
        expect(span?.getAttribute('data-user-id')).toBe(value);
        expect(span?.textContent).toBe(`@${value}`);
        expect(span?.getAttributeNames().sort()).toEqual(['class', 'data-mention', 'data-user-id']);
        const picture = body.querySelector('img');
        expect(picture?.getAttribute('alt')).toBe(value);
        expect(picture?.getAttribute('data-file-id')).toBe(value);
        expect(picture?.getAttributeNames().sort()).toEqual([
          'alt',
          'data-file-id',
          'decoding',
          'loading',
          'src',
        ]);
        expect(body.querySelectorAll('img')).toHaveLength(1);
        expect(body.querySelector('script, iframe, svg, style, b')).toBeNull();
        expectHarmless(body);
      } finally {
        setFileSource(previous);
      }
    },
  );

  it('survives being written out as markup and read back', () => {
    // What a caller must never do with a body, done here to see that even
    // then the text comes back as text: the serialiser escapes what the view
    // put in as text nodes and attribute values.
    const value = '<img src=x onerror="window.pwned = true">"\'&';
    const body = place(
      renderBody(doc(paragraph(text(value, link(`/x?q="${value}`))), image('f', value))),
    );
    const reread = document.createElement('div');
    reread.innerHTML = body.innerHTML;
    host.append(reread);
    expect(reread.querySelector('p')?.textContent).toBe(value);
    expect(reread.querySelectorAll('img')).toHaveLength(1);
    expectHarmless(reread);
  });
});
