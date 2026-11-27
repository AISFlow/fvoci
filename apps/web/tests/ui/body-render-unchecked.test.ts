/**
 * The read-only view with the validator out of the way.
 *
 * `renderBody` draws nothing the validator refuses, so in the product the
 * drawing never sees a bad document. Here the validator is replaced by one
 * that accepts everything, to see that the drawing holds on its own: an
 * address that is not allowed gives text and no link, and what is not on the
 * list is not drawn.
 */
import { Editor } from '@tiptap/core';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { bodyExtensions } from '$lib/editor/extensions.ts';
import { renderBody } from '$lib/editor/render.ts';
import type * as Schema from '$lib/editor/schema.ts';
import { validateDoc, type DocNode } from '$lib/editor/schema.ts';

import { doc, paragraph, text } from '../support/body-docs.ts';

vi.mock('$lib/editor/schema.ts', async (original) => ({
  ...(await original<typeof Schema>()),
  validateDoc: () => [],
}));

const loose = (node: Record<string, unknown>): DocNode => node as unknown as DocNode;
const linked = (href: unknown): DocNode =>
  doc(paragraph({ type: 'text', text: 'click me', marks: [{ type: 'link', attrs: { href } }] }));

function draw(body: unknown): HTMLElement {
  const rendered = renderBody(body);
  if (!rendered.ok) throw new Error('the validator is supposed to be out of the way');
  const host = document.createElement('div');
  host.append(rendered.fragment);
  return host;
}

let editor: Editor | undefined;
afterEach(() => {
  editor?.destroy();
  editor = undefined;
});

describe('drawing without the validator', () => {
  it('really has the validator out of the way', () => {
    expect(validateDoc({ type: 'nonsense' })).toEqual([]);
  });

  it.each([
    ['javascript:alert(1)'],
    ['JaVaScRiPt:alert(1)'],
    ['data:text/html,<script>alert(1)</script>'],
    ['vbscript:msgbox(1)'],
    ['java\tscript:alert(1)'],
    ['java\nscript:alert(1)'],
    ['\u0000javascript:alert(1)'],
    [' javascript:alert(1)'],
    ['//evil.example/path'],
    ['\\\\evil.example\\share'],
    ['/\\evil.example'],
    ['file:///etc/passwd'],
    [''],
    [`/${'a'.repeat(2048)}`],
    [7],
    [null],
    [undefined],
    [{ toString: () => 'https://example.com' }],
    [['https://example.com']],
  ])('draws the text of a link to %j and no link', (href) => {
    const body = draw(linked(href));
    expect(body.querySelector('a')).toBeNull();
    expect(body.innerHTML).toBe('<p>click me</p>');
  });

  it('keeps the other marks of a text whose link is dropped', () => {
    const body = draw(
      doc(
        paragraph({
          type: 'text',
          text: 'x',
          marks: [{ type: 'bold' }, { type: 'link', attrs: { href: 'javascript:alert(1)' } }],
        }),
      ),
    );
    expect(body.innerHTML).toBe('<p><strong>x</strong></p>');
  });

  it('draws only the address of a link, whatever else the mark carries', () => {
    const body = draw(
      doc(
        paragraph({
          type: 'text',
          text: 'x',
          marks: [
            {
              type: 'link',
              attrs: {
                href: '/ok',
                target: '_top',
                rel: 'opener',
                onclick: 'alert(1)',
                class: 'ProseMirror',
                style: 'position:fixed',
              },
            },
          ],
        }),
      ),
    );
    const anchor = body.querySelector('a');
    expect(anchor?.getAttributeNames().sort()).toEqual(['href', 'rel', 'target']);
    expect(anchor?.getAttribute('target')).toBe('_blank');
    expect(anchor?.getAttribute('rel')).toBe('noopener noreferrer nofollow');
  });

  it('does not draw nodes and marks that are not on the list', () => {
    const body = draw(
      doc(
        { type: 'script', content: [text('alert(1)')] },
        { type: 'iframe', attrs: { src: 'https://evil.example' } },
        { type: 'constructor' },
        paragraph(
          text('kept', 'underline', 'constructor', '__proto__'),
          { type: 'script', content: [text('alert(2)')] },
          { type: 'image', attrs: { fileId: 'inline' } },
        ),
        { type: 'table', content: [paragraph(text('cell'))] },
      ),
    );
    expect(body.innerHTML).toBe('<p>kept</p>');
  });

  it('draws the rows and cells of a table alone, with spans within the limits', () => {
    const body = draw(
      doc(
        loose({
          type: 'table',
          content: [
            paragraph(text('not a row')),
            {
              type: 'tableRow',
              content: [
                paragraph(text('not a cell')),
                {
                  type: 'tableCell',
                  attrs: { colspan: 1e9, rowspan: '2" onclick="alert(1)' },
                  content: [paragraph(text('x'))],
                },
              ],
            },
          ],
        }),
      ),
    );
    expect(body.querySelectorAll('col')).toHaveLength(1);
    expect(body.querySelector('td')?.getAttributeNames()).toEqual([]);
    expect(body.textContent).toBe('x');
  });

  it('draws no attribute a node is not meant to have', () => {
    const body = draw(
      doc(
        loose({
          type: 'paragraph',
          attrs: { onclick: 'alert(1)', style: 'x', class: 'y', id: 'z' },
        }),
        { type: 'heading', attrs: { level: 9, onclick: 'alert(1)' }, content: [text('h')] },
        { type: 'heading', attrs: { level: '2"><script>' }, content: [text('h')] },
        { type: 'orderedList', attrs: { start: '3" onclick="alert(1)' }, content: [] },
        { type: 'orderedList', attrs: { start: 2.5 }, content: [] },
        { type: 'taskList', content: [{ type: 'taskItem', attrs: { checked: 'yes' } }] },
        {
          type: 'image',
          attrs: { fileId: 7, alt: { a: 1 }, src: 'https://tracker.example/x.gif' },
        },
        { type: 'codeBlock', attrs: { language: 'ts onclick=alert(1)' }, content: [text('c')] },
        { type: 'codeBlock', attrs: { language: '"><script>' }, content: [text('c')] },
        { type: 'codeBlock', attrs: { language: 7 }, content: [text('c')] },
        paragraph({ type: 'mention', attrs: { userId: 7, class: 'x' } }),
      ),
    );
    expect(Array.from(body.children).map((element) => element.outerHTML)).toEqual([
      '<p><br></p>',
      '<p>h</p>',
      '<p>h</p>',
      '<ol></ol>',
      '<ol></ol>',
      '<ul data-type="taskList"><li data-checked="false"><label><input type="checkbox" aria-readonly="true"></label><div></div></li></ul>',
      '<img loading="lazy" decoding="async">',
      '<pre data-line-digits="1"><code>c</code></pre>',
      '<pre data-line-digits="1"><code>c</code></pre>',
      '<pre data-line-digits="1"><code>c</code></pre>',
      '<p><span data-user-id="" data-mention="" class="mention">@</span></p>',
    ]);
  });

  it('draws marks on a line break and a mention the way the editor does', () => {
    // The validator refuses marks on anything but text, yet the editor makes
    // them: a line break typed or pasted inside bold text is itself bold.
    // Should the list ever allow that, the view already draws it alike.
    const body: DocNode = doc(
      paragraph(
        text('a', 'bold'),
        { type: 'hardBreak', marks: [{ type: 'bold' }] },
        text('b', 'bold'),
        text(' '),
        text('x ', 'italic'),
        { type: 'mention', attrs: { userId: 'u1' }, marks: [{ type: 'italic' }] },
        text(' y', 'italic'),
      ),
    );
    editor = new Editor({
      element: document.createElement('div'),
      extensions: bodyExtensions(),
      content: body,
    });
    const fromEditor = editor.view.dom.querySelector('p');
    const fromView = draw(body).querySelector('p');
    expect(fromView?.innerHTML).toBe(
      '<strong>a<br>b</strong> <em>x <span data-user-id="u1" data-mention="" class="mention">@u1</span> y</em>',
    );
    expect(fromEditor?.innerHTML).toBe(fromView?.innerHTML);
  });
});
