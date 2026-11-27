/**
 * A code block in the editor: coloured by decorations only, so that the
 * document, what is stored and the shared document stay what they were;
 * the same runs as where a body is only shown; the numbers of the lines; the
 * language changed from the keys; the copy button; and colours that hold
 * through a change from another reader and through an input method. The
 * highlighter answers in this process, as the worker would.
 */
import { Editor } from '@tiptap/core';
import { EditorState } from '@tiptap/pm/state';
import type { DecorationSet } from '@tiptap/pm/view';
import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BODY_FIELD } from '$lib/editor/body-editor.ts';
import BodyEditor from '$lib/editor/BodyEditor.svelte';
import { codeHighlight, codeHighlightKey } from '$lib/editor/code-highlight.ts';
import { answer } from '$lib/editor/code-highlight-worker.ts';
import { useHighlighter, type Highlighter } from '$lib/editor/code-highlighter.ts';
import { CODE_LINES_ATTRIBUTE } from '$lib/editor/code-lines.ts';
import { lineStarts } from '$lib/editor/code-paint.ts';
import { CODE_COPY_TEXTS } from '$lib/editor/code-copy-texts.ts';
import { drawCodeBlocks } from '$lib/editor/code-view.ts';
import { bodyExtensions } from '$lib/editor/extensions.ts';
import { renderBody } from '$lib/editor/render.ts';
import { toStoredDoc, type DocNode } from '$lib/editor/schema.ts';

import { code, doc, paragraph, text } from '../support/body-docs.ts';
import { clipboardOf, codeRuns, lineNumbers, noClipboard } from '../support/code-blocks.ts';

const SOURCE = 'const answer = 42; // the answer\nfunction ask() {\n  return "why";\n}';

let asked: string[] = [];
let editor: Editor | undefined;

beforeEach(() => {
  asked = [];
  const inProcess: Highlighter = {
    ready: () => Promise.resolve(true),
    highlight: (grammar, value) => {
      asked.push(value);
      return Promise.resolve(answer({ id: 0, grammar, text: value }).tokens);
    },
    stop: () => undefined,
  };
  useHighlighter(inProcess);
});
afterEach(() => {
  editor?.destroy();
  editor = undefined;
  useHighlighter(undefined);
  document.documentElement.removeAttribute(CODE_LINES_ATTRIBUTE);
  document.body.replaceChildren();
  noClipboard(window);
  vi.restoreAllMocks();
});

function create(content: DocNode, editable = true): Editor {
  const element = document.createElement('div');
  element.className = 'doc-body';
  document.body.append(element);
  editor = new Editor({
    element,
    injectCSS: false,
    editable,
    extensions: bodyExtensions(),
    content,
  });
  return editor;
}

const codeOf = (target: Editor, index = 0): HTMLElement | null =>
  target.view.dom.querySelectorAll<HTMLElement>('.doc-code pre > code')[index] ?? null;

const coloured = async (target: Editor): Promise<void> => {
  await vi.waitFor(() => {
    expect(codeOf(target)?.querySelector('.doc-code-keyword')).not.toBeNull();
  });
};

describe('a code block in the editor', () => {
  it('is coloured, and the document and what is stored stay exactly what they were', async () => {
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    const content = doc(paragraph(text('before')), code('ts', SOURCE), code('python', 'x = 1'));
    const target = create(content);
    const before = JSON.stringify(target.getJSON());
    await coloured(target);
    expect(lineNumbers(codeOf(target))).toEqual(lineStarts(SOURCE));
    expect(JSON.stringify(target.getJSON())).toBe(before);
    expect(target.getJSON()).toEqual(content);
    expect(codeOf(target)?.textContent).toBe(SOURCE);
  });

  it('draws the same runs and numbers as a body that is only shown', async () => {
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    const body = doc(code('ts', SOURCE), code('rust', 'fn main() {\n    let x = 1;\n}\n'));
    const target = create(body);
    await coloured(target);
    await vi.waitFor(() => {
      expect(codeOf(target, 1)?.querySelector('.doc-code-keyword')).not.toBeNull();
    });
    const root = document.createElement('div');
    const drawn = renderBody(body);
    if (!drawn.ok) throw new Error('the body cannot be drawn');
    root.append(drawn.fragment);
    document.body.append(root);
    drawCodeBlocks(root, CODE_COPY_TEXTS);
    const shown = [...root.querySelectorAll('pre > code')];
    for (const [index, view] of shown.entries()) {
      expect(codeRuns(codeOf(target, index))).toEqual(codeRuns(view));
      expect(lineNumbers(codeOf(target, index))).toEqual(lineNumbers(view));
    }
  });

  it('stays plain in a language it does not know, and leaves a diagram a diagram', async () => {
    const target = create(doc(code('klingon', 'qaplah'), code('mermaid', 'graph TD; A-->B')));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(target.view.dom.querySelectorAll('.doc-code')).toHaveLength(1);
    expect(target.view.dom.querySelector('.doc-diagram-block')).not.toBeNull();
    expect(codeRuns(codeOf(target))).toEqual([['', 'qaplah']]);
    expect(asked).toEqual([]);
  });

  it('keeps its colours while it is typed in, and colours what was typed a moment later', async () => {
    const target = create(doc(code('ts', 'let a = 1;')));
    await coloured(target);
    target.commands.insertContentAt(1, 'const b = 2;\n');
    // At once: the old runs moved along, so nothing flashes plain.
    expect(codeRuns(codeOf(target))).toContainEqual(['doc-code-keyword', 'let']);
    await vi.waitFor(() => {
      expect(codeRuns(codeOf(target))).toContainEqual(['doc-code-keyword', 'const']);
    });
  });

  it('draws nothing new while an input method composes, and catches up once it ends', async () => {
    const target = create(doc(code('ts', 'let a = 1;')));
    await coloured(target);
    const askedBefore = asked.length;
    target.view.dom.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
    target.commands.insertContentAt(1, 'const ');
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(asked).toHaveLength(askedBefore);
    expect(codeRuns(codeOf(target))).toContainEqual(['doc-code-keyword', 'let']);
    target.view.dom.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
    target.commands.insertContentAt(7, '');
    target.view.dispatch(target.state.tr);
    await vi.waitFor(() => {
      expect(codeRuns(codeOf(target))).toContainEqual(['doc-code-keyword', 'const']);
    });
  });
});

describe('the decorations of code blocks', () => {
  /** What a decoration set holds, in order, to be compared. */
  const listOf = (set: DecorationSet | undefined): string[] =>
    (set?.find() ?? [])
      .map((each) => `${each.from}-${each.to}-${JSON.stringify(each.spec)}`)
      .sort();

  it('stay what they would be if made anew, through edits anywhere and a change of language', async () => {
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    const long = Array.from(
      { length: 120 },
      (_, line) => `let value${line} = "${line}"; // ${line}`,
    );
    const target = create(
      doc(code('ts', long.join('\n')), paragraph(text('between')), code('python', 'x = 1\ny = 2')),
    );
    await coloured(target);
    let seed = 7;
    const random = (below: number): number => {
      seed = (seed * 1103515245 + 12345) % 2 ** 31;
      return seed % below;
    };
    for (let round = 0; round < 30; round += 1) {
      const { size } = target.state.doc.content;
      const at = 1 + random(size - 2);
      if (round === 15) {
        // A change of language: every run of the long block changes at once.
        target.view.dispatch(target.state.tr.setNodeAttribute(0, 'language', 'rust'));
      } else if (random(3) === 0 && at + 3 < size) {
        target.view.dispatch(target.state.tr.delete(at, at + 3));
      } else {
        target.view.dispatch(target.state.tr.insertText(random(2) ? '\n' : 'if (x) ', at));
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
      await vi.waitFor(() => {
        const fresh = EditorState.create({ doc: target.state.doc, plugins: [codeHighlight()] });
        expect(listOf(codeHighlightKey.getState(target.state)?.decorations)).toEqual(
          listOf(codeHighlightKey.getState(fresh)?.decorations),
        );
      });
    }
    // And the page shows what the set holds: a number at the start of every line.
    expect(lineNumbers(codeOf(target))).toEqual(lineStarts(codeOf(target)?.textContent ?? ''));
  });
});

describe('the numbers of the lines of a long block in the editor', () => {
  it('are one column beside the text, where a number at each line would cost too much', async () => {
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    const lines = Array.from({ length: 3000 }, (_, line) => `line ${String(line)}`);
    const target = create(doc(code(undefined, lines.join('\n')), code(undefined, 'short\nblock')));
    await vi.waitFor(() => {
      expect(target.view.dom.querySelector('.doc-code-column')).not.toBeNull();
    });
    const [long, short] = [...target.view.dom.querySelectorAll<HTMLElement>('.doc-code')];
    expect(long?.hasAttribute('data-long')).toBe(true);
    expect(long?.getAttribute('data-line-digits')).toBe('4');
    const column = long?.querySelector(':scope > .doc-code-column');
    const numbers = column?.getAttribute('data-numbers')?.split('\n') ?? [];
    expect(numbers).toHaveLength(3000);
    expect([numbers[0], numbers.at(-1)]).toEqual(['1', '3000']);
    expect(column?.getAttribute('aria-hidden')).toBe('true');
    expect(column?.textContent).toBe('');
    expect(short?.querySelector('.doc-code-column')?.hasAttribute('data-numbers')).toBe(false);
    expect(long?.querySelector('.doc-code-line')).toBeNull();
    // The text is the text; a short block keeps a number at each line.
    expect(long?.querySelector('pre > code')?.textContent).toBe(lines.join('\n'));
    expect(short?.hasAttribute('data-long')).toBe(false);
    expect(lineNumbers(short?.querySelector('pre > code'))).toEqual([0, 6]);
    // A line more is a column of one more.
    target.commands.insertContentAt(1, 'first\n');
    await vi.waitFor(() => {
      expect(
        target.view.dom
          .querySelector('.doc-code-column')
          ?.getAttribute('data-numbers')
          ?.split('\n'),
      ).toHaveLength(3001);
    });
    // Out of the text, whatever is drawn in it.
    expect(long?.querySelector('pre .doc-code-column')).toBeNull();
  });
});

describe('a move of the caret the editor has not read yet', () => {
  it('is kept when the decorations are drawn anew right behind it', async () => {
    const target = create(doc(code('ts', SOURCE)));
    await coloured(target);
    target.commands.setTextSelection(2);
    target.view.focus();
    expect(target.view.hasFocus()).toBe(true);
    expect(target.state.selection.head).toBe(2);
    // The browser moves the caret to the end of the block (End, an arrow), and tells only later.
    const walker = document.createTreeWalker(codeOf(target) ?? document.body, NodeFilter.SHOW_TEXT);
    let last: Node | null = null;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) last = node;
    document.getSelection()?.collapse(last, last?.textContent?.length ?? 0);
    // In that moment the numbers are turned on, and the decorations drawn anew.
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    await vi.waitFor(() => {
      expect(codeOf(target)?.querySelector('.doc-code-line')).not.toBeNull();
    });
    expect(target.state.selection.head).toBe(1 + SOURCE.length);
  });

  it('is kept when the colours come right behind it', async () => {
    let answerNow: (() => void) | undefined;
    useHighlighter({
      ready: () => Promise.resolve(true),
      highlight: (grammar, value) =>
        new Promise((resolve) => {
          answerNow = () => {
            resolve(answer({ id: 0, grammar, text: value }).tokens);
          };
        }),
      stop: () => undefined,
    });
    const target = create(doc(code('ts', SOURCE)));
    target.commands.setTextSelection(2);
    target.view.focus();
    expect(target.state.selection.head).toBe(2);
    await vi.waitFor(() => {
      expect(answerNow).toBeDefined();
    });
    // The browser moves the caret to the end of the block, and tells only after the colours,
    // which come in that moment: its word is held back until they are drawn.
    const late = (event: Event): void => {
      event.stopImmediatePropagation();
    };
    document.addEventListener('selectionchange', late, { capture: true });
    try {
      const walker = document.createTreeWalker(
        codeOf(target) ?? document.body,
        NodeFilter.SHOW_TEXT,
      );
      let last: Node | null = null;
      for (let node = walker.nextNode(); node; node = walker.nextNode()) last = node;
      document.getSelection()?.collapse(last, last?.textContent?.length ?? 0);
      answerNow?.();
      await coloured(target);
    } finally {
      // Not left for the tests after, whatever came of this one.
      document.removeEventListener('selectionchange', late, { capture: true });
    }
    document.dispatchEvent(new Event('selectionchange'));
    await vi.waitFor(() => {
      expect(target.state.selection.head).toBe(1 + SOURCE.length);
    });
  });
});

describe('the language of a code block', () => {
  const button = (target: Editor): HTMLButtonElement => {
    const found = target.view.dom.querySelector<HTMLButtonElement>('button.doc-code-language');
    if (!found) throw new Error('no language button');
    return found;
  };
  const field = (target: Editor): HTMLInputElement => {
    const found = target.view.dom.querySelector<HTMLInputElement>('.doc-code-search');
    if (!found) throw new Error('no field');
    return found;
  };
  const key = (element: Element, name: string): void => {
    element.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true }));
  };

  it('is named on a button that opens a list to find and take another from, by the keys', () => {
    const target = create(doc(code('ts', 'let a = 1;')));
    const opener = button(target);
    expect(opener.getAttribute('aria-label')).toContain('TypeScript');
    expect(opener.getAttribute('aria-haspopup')).toBe('listbox');
    opener.focus();
    key(opener, 'ArrowDown');
    expect(opener.getAttribute('aria-expanded')).toBe('true');
    const search = field(target);
    expect(document.activeElement).toBe(search);
    // The list starts on the block's language, "No language" first.
    const rows = (): HTMLElement[] => [
      ...target.view.dom.querySelectorAll<HTMLElement>('[role="option"]'),
    ];
    expect(rows()[0]?.getAttribute('data-language')).toBe('');
    const active = (): string | null | undefined =>
      rows()
        .find((row) => row.id === search.getAttribute('aria-activedescendant'))
        ?.getAttribute('data-language');
    expect(active()).toBe('typescript');
    search.value = 'py';
    search.dispatchEvent(new Event('input', { bubbles: true }));
    expect(rows().map((row) => row.getAttribute('data-language'))).toEqual(['python']);
    key(search, 'Enter');
    expect(target.getJSON()).toEqual(doc(code('python', 'let a = 1;')));
    expect(document.activeElement).toBe(button(target));
    expect(button(target).getAttribute('aria-expanded')).toBe('false');
  });

  it('takes "No language", and closes on Escape without a change', () => {
    const target = create(doc(code('ts', 'x')));
    button(target).click();
    key(field(target), 'Escape');
    expect(document.activeElement).toBe(button(target));
    expect(target.getJSON()).toEqual(doc(code('ts', 'x')));
    button(target).click();
    // The arrows go round: up from the first row is the last.
    const search = field(target);
    search.value = 'no';
    search.dispatchEvent(new Event('input', { bubbles: true }));
    const first = search.getAttribute('aria-activedescendant');
    key(search, 'ArrowUp');
    key(search, 'ArrowDown');
    expect(search.getAttribute('aria-activedescendant')).toBe(first);
    target.view.dom.querySelector<HTMLElement>('[role="option"][data-language=""]')?.click();
    // A block without a language: the attribute is empty, and nothing of it is stored.
    expect(target.state.doc.firstChild?.attrs).toEqual({ language: null });
    expect(toStoredDoc(target.getJSON() as DocNode)).toEqual(doc(code(undefined, 'x')));
  });

  it('cannot be changed in an editor that only reads, where the button only names it', () => {
    const target = create(doc(code('ts', 'x')), false);
    const opener = button(target);
    expect(opener.disabled).toBe(true);
    opener.click();
    expect(target.view.dom.querySelector('.doc-code-popup:not([hidden])')).toBeNull();
    target.setEditable(true);
    expect(button(target).disabled).toBe(false);
  });
});

describe('the copy button of a code block', () => {
  const copyButton = (target: Editor): HTMLButtonElement => {
    const found = target.view.dom.querySelector<HTMLButtonElement>('button.doc-code-copy');
    if (!found) throw new Error('no copy button');
    return found;
  };

  it('copies the whole text exactly, without the numbers of the lines, and says so', async () => {
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    const written = clipboardOf(window);
    const target = create(doc(code('ts', SOURCE)));
    copyButton(target).click();
    await vi.waitFor(() => {
      expect(copyButton(target).hasAttribute('data-copied')).toBe(true);
    });
    expect(written).toEqual([SOURCE]);
    const status = target.view.dom.querySelector('.doc-code-status');
    expect(status?.getAttribute('role')).toBe('status');
    expect(status?.textContent).toBeTruthy();
    expect(target.getJSON()).toEqual(doc(code('ts', SOURCE)));
  });

  it("uses the browser's copy where there is no clipboard, and says how to copy when that fails too", async () => {
    noClipboard(window);
    const target = create(doc(paragraph(text('first')), code('ts', SOURCE)));
    const copy = vi.fn(() => true);
    // The browser's copy of the selection, which the button falls back on.
    Object.defineProperty(document, 'execCommand', { configurable: true, value: copy });
    copyButton(target).click();
    await vi.waitFor(() => {
      expect(copyButton(target).hasAttribute('data-copied')).toBe(true);
    });
    expect(copy).toHaveBeenCalledWith('copy');
    // The text was selected for it.
    const { from, to } = target.state.selection;
    expect(target.state.doc.textBetween(from, to)).toBe(SOURCE);

    copy.mockReturnValue(false);
    target.commands.setTextSelection(1);
    copyButton(target).click();
    await vi.waitFor(() => {
      expect(copyButton(target).hasAttribute('data-copied')).toBe(false);
      expect(target.view.dom.querySelector('.doc-code-status')?.textContent).toMatch(/C/);
    });
    const left = target.state.selection;
    expect(target.state.doc.textBetween(left.from, left.to)).toBe(SOURCE);
  });
});

describe('a code block on a shared document', () => {
  /** A shared document with one code block, as another reader's editor writes it. */
  function sharedWithCode(language: string, value: string): Y.Doc {
    const shared = new Y.Doc();
    const block = new Y.XmlElement('codeBlock');
    block.setAttribute('language', language);
    block.insert(0, [new Y.XmlText(value)]);
    shared.getXmlFragment(BODY_FIELD).insert(0, [block]);
    return shared;
  }
  const editorIn = (container: HTMLElement): Editor => {
    const root = container.querySelector<HTMLElement & { editor?: Editor }>('.ProseMirror');
    if (!root?.editor) throw new Error('no editor in the component');
    return root.editor;
  };

  it('writes nothing to the shared document by being coloured or numbered', async () => {
    const shared = sharedWithCode('ts', SOURCE);
    const before = Y.encodeStateAsUpdate(shared);
    const { container } = render(BodyEditor, { doc: null, collab: shared });
    const target = editorIn(container);
    await coloured(target);
    document.documentElement.setAttribute(CODE_LINES_ATTRIBUTE, '');
    await vi.waitFor(() => {
      expect(lineNumbers(codeOf(target))).toEqual(lineStarts(SOURCE));
    });
    expect(Y.encodeStateAsUpdate(shared)).toEqual(before);
  });

  it("keeps its colours through another reader's change and colours that change a moment later", async () => {
    const shared = sharedWithCode('ts', 'let a = 1;');
    const { container } = render(BodyEditor, { doc: null, collab: shared });
    const target = editorIn(container);
    await coloured(target);
    const other = new Y.Doc();
    Y.applyUpdate(other, Y.encodeStateAsUpdate(shared));
    const since = Y.encodeStateVector(other);
    const block = other.getXmlFragment(BODY_FIELD).get(0) as Y.XmlElement;
    (block.get(0) as Y.XmlText).insert(0, 'const b = 2;\n');
    Y.applyUpdate(shared, Y.encodeStateAsUpdate(other, since), 'room');
    expect(codeOf(target)?.textContent).toBe('const b = 2;\nlet a = 1;');
    expect(codeRuns(codeOf(target))).toContainEqual(['doc-code-keyword', 'let']);
    await vi.waitFor(() => {
      expect(codeRuns(codeOf(target))).toContainEqual(['doc-code-keyword', 'const']);
    });
  });
});
