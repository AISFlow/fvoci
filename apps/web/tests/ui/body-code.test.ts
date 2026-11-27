/**
 * The code blocks of a body that is only shown (code-view.ts): the frame with
 * the language's name and the copy button, the colours as the editor draws
 * them, the numbers of the lines while the reader has them on, and what is
 * put back when the body goes. The highlighter answers in this process, as
 * the worker would.
 */
import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import BodyView from '$lib/editor/BodyView.svelte';
import { answer } from '$lib/editor/code-highlight-worker.ts';
import { useHighlighter, type Highlighter } from '$lib/editor/code-highlighter.ts';
import { CODE_LINES_ATTRIBUTE } from '$lib/editor/code-lines.ts';
import { lineStarts } from '$lib/editor/code-paint.ts';
import { CODE_COPY_TEXTS } from '$lib/editor/code-copy-texts.ts';
import { drawCodeBlocks } from '$lib/editor/code-view.ts';
import { renderBody } from '$lib/editor/render.ts';

import { code, doc, paragraph, text } from '../support/body-docs.ts';
import { clipboardOf, codeRuns, lineNumbers } from '../support/code-blocks.ts';

const SOURCE = 'const answer = 42; // the answer\nfunction ask() {\n  return "why";\n}';

let asked: string[] = [];
const inProcess: Highlighter = {
  ready: () => Promise.resolve(true),
  highlight: (grammar, value) => {
    asked.push(value);
    return Promise.resolve(answer({ id: 0, grammar, text: value }).tokens);
  },
  stop: () => undefined,
};

beforeEach(() => {
  asked = [];
  useHighlighter(inProcess);
});
afterEach(() => {
  useHighlighter(undefined);
  document.documentElement.removeAttribute(CODE_LINES_ATTRIBUTE);
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

/** A body drawn as `BodyView` draws it, in the page. */
function shown(...content: Parameters<typeof doc>): HTMLElement {
  const root = document.createElement('div');
  root.className = 'doc-body';
  const drawn = renderBody(doc(...content));
  if (!drawn.ok) throw new Error('the body cannot be drawn');
  root.append(drawn.fragment);
  document.body.append(root);
  return root;
}

describe('a code block that is only shown', () => {
  it('is framed with its language and a copy button, and coloured once the tokens come', async () => {
    const root = shown(code('ts', SOURCE));
    drawCodeBlocks(root, CODE_COPY_TEXTS);
    const frame = root.querySelector('.doc-code');
    expect(frame?.querySelector('.doc-code-name')?.textContent).toBe('TypeScript');
    const copy = frame?.querySelector<HTMLButtonElement>('button.doc-code-copy');
    expect(copy?.getAttribute('aria-label')).toBeTruthy();
    // Nothing to change it with: only the name.
    expect(frame?.querySelector('button.doc-code-language')).toBeNull();
    const codeElement = frame?.querySelector('pre > code');
    expect(codeElement?.textContent).toBe(SOURCE);
    await vi.waitFor(() => {
      expect(codeRuns(codeElement)).toContainEqual(['doc-code-keyword', 'const']);
    });
    expect(codeRuns(codeElement)).toContainEqual(['doc-code-string', '"why"']);
    // The text is the same text, letter for letter.
    expect(codeElement?.textContent).toBe(SOURCE);
  });

  it('is coloured at once when its text was coloured before', async () => {
    const first = shown(code('ts', SOURCE));
    drawCodeBlocks(first, CODE_COPY_TEXTS);
    await vi.waitFor(() => {
      expect(first.querySelector('.doc-code-keyword')).not.toBeNull();
    });
    const again = shown(code('typescript', SOURCE));
    drawCodeBlocks(again, CODE_COPY_TEXTS);
    expect(again.querySelector('.doc-code-keyword')).not.toBeNull();
    expect(asked).toEqual([SOURCE]);
  });

  it('stays plain in a language it does not know, or none, and never asks', async () => {
    const root = shown(code('klingon', 'qaplah'), code(undefined, 'plain'));
    drawCodeBlocks(root, CODE_COPY_TEXTS);
    const names = [...root.querySelectorAll('.doc-code-name')].map((each) => each.textContent);
    expect(names).toEqual(['klingon', '']);
    await Promise.resolve();
    expect(root.querySelectorAll('pre span:not(.doc-code-line)')).toHaveLength(0);
    expect(asked).toEqual([]);
  });

  it('leaves a diagram alone', () => {
    const root = shown(code('mermaid', 'graph TD; A-->B'));
    const before = root.innerHTML;
    drawCodeBlocks(root, CODE_COPY_TEXTS);
    expect(root.querySelector('.doc-code')).toBeNull();
    expect(root.innerHTML).toBe(before);
  });

  it('numbers its lines while the reader has them on, out of the text and of what is read out', async () => {
    const root = shown(code('ts', SOURCE), code(undefined, 'one\n'));
    drawCodeBlocks(root, CODE_COPY_TEXTS);
    expect(root.querySelector('.doc-code-line')).toBeNull();
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    await vi.waitFor(() => {
      expect(root.querySelectorAll('.doc-code-line').length).toBeGreaterThan(0);
    });
    const [first, second] = [...root.querySelectorAll('pre > code')];
    // One at the start of each line; the empty last line after a line feed has its number too.
    expect(lineNumbers(first)).toEqual(lineStarts(SOURCE));
    expect(lineNumbers(second)).toEqual([0, 4]);
    for (const marker of root.querySelectorAll('.doc-code-line')) {
      expect(marker.getAttribute('aria-hidden')).toBe('true');
      expect(marker.textContent).toBe('');
    }
    expect(first?.textContent).toBe(SOURCE);
    document.documentElement.removeAttribute(CODE_LINES_ATTRIBUTE);
    await vi.waitFor(() => {
      expect(root.querySelector('.doc-code-line')).toBeNull();
    });
  });

  it('copies the whole text exactly, without the numbers, and says so', async () => {
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    const root = shown(code('ts', SOURCE));
    drawCodeBlocks(root, CODE_COPY_TEXTS);
    const written = clipboardOf(window);
    const button = root.querySelector<HTMLButtonElement>('button.doc-code-copy');
    button?.click();
    await vi.waitFor(() => {
      expect(button?.hasAttribute('data-copied')).toBe(true);
    });
    expect(written).toEqual([SOURCE]);
    expect(root.querySelector('[role="status"]')?.textContent).toBeTruthy();
  });

  it('puts back what it found when the body goes', () => {
    const root = shown(paragraph(text('before')), code('ts', SOURCE));
    const before = root.innerHTML;
    const undo = drawCodeBlocks(root, CODE_COPY_TEXTS);
    expect(root.innerHTML).not.toBe(before);
    undo();
    expect(root.innerHTML).toBe(before);
  });

  it('is drawn by BodyView, which fetches it only for a body with code', async () => {
    const { container } = render(BodyView, { doc: doc(code('ts', SOURCE)) });
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-code .doc-code-keyword')).not.toBeNull();
    });
    const plain = render(BodyView, { doc: doc(paragraph(text('no code'))) });
    expect(plain.container.querySelector('.doc-code')).toBeNull();
  });
});
