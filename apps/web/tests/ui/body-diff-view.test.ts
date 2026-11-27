/**
 * Two bodies merged by a comparison, drawn for reading (render-diff.ts): what
 * marks a block added, removed or changed, the words heard before each mark,
 * the notes of what a block was, the stops of the way through the changes,
 * a table changed drawn whole as it was and as it is, and code whose lines
 * changed keeping its marks through its colours (code-view.ts). No stored
 * body can carry the marks.
 */
import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { answer } from '$lib/editor/code-highlight-worker.ts';
import { useHighlighter, type Highlighter } from '$lib/editor/code-highlighter.ts';
import { CODE_COPY_TEXTS } from '$lib/editor/code-copy-texts.ts';
import { drawCodeBlocks } from '$lib/editor/code-view.ts';
import { DIFF_STOP, drawDiffBlocks } from '$lib/editor/render-diff.ts';
import { renderBody } from '$lib/editor/render.ts';
import { DIAGRAM_LANGUAGE, type DocNode } from '$lib/editor/schema.ts';
import VersionDiff from '$lib/features/card/VersionDiff.svelte';
import { diffBodies } from '$lib/logic/body-diff.ts';

import {
  bullets,
  cell,
  code,
  doc,
  fileBlock,
  heading,
  image,
  item,
  math,
  mathBlock,
  mention,
  numbered,
  paragraph,
  reference,
  row,
  table,
  text,
} from '../support/body-docs.ts';
import { clipboardOf, codeRuns } from '../support/code-blocks.ts';

const inProcess: Highlighter = {
  ready: () => Promise.resolve(true),
  highlight: (grammar, value) => Promise.resolve(answer({ id: 0, grammar, text: value }).tokens),
  stop: () => undefined,
};

beforeEach(() => {
  useHighlighter(inProcess);
});
afterEach(() => {
  useHighlighter(undefined);
  document.body.replaceChildren();
});

/** What changed from `before` to `after`, drawn in the page as a `doc-body`. */
function drawn(before: DocNode, after: DocNode): HTMLElement {
  const root = document.createElement('div');
  root.className = 'doc-body';
  root.append(
    drawDiffBlocks(diffBodies(before, after, { diagramLanguage: DIAGRAM_LANGUAGE }).blocks),
  );
  document.body.append(root);
  return root;
}

const task = (checked: boolean, value: string): DocNode => ({
  type: 'taskItem',
  attrs: { checked },
  content: [paragraph(text(value))],
});

describe('the marks of a comparison', () => {
  it('puts what was added and removed in <ins> and <del>, each with the words heard before it', () => {
    const root = drawn(doc(paragraph(text('the quick fox'))), doc(paragraph(text('the slow fox'))));
    const del = root.querySelector('p > del');
    const ins = root.querySelector('p > ins');
    expect([del?.textContent, ins?.textContent]).toEqual(['quick', 'slow']);
    expect(del?.getAttribute('data-change')).toBe('removed');
    expect(del?.getAttribute('data-diff-label')).toBeTruthy();
    expect(ins?.getAttribute('data-diff-label')).not.toBe(del?.getAttribute('data-diff-label'));
    // The paragraph is a stop of the way through the changes, and can take the focus.
    expect(root.querySelector('p')?.hasAttribute(DIFF_STOP)).toBe(true);
    expect(root.querySelector('p')?.getAttribute('tabindex')).toBe('-1');
  });

  it('marks a block added or removed whole, an image in an element that can carry the mark', () => {
    const root = drawn(
      doc(paragraph(text('kept')), image('f1')),
      doc(paragraph(text('kept')), paragraph(text('new'))),
    );
    const added = root.querySelector('p[data-diff="added"]');
    expect(added?.textContent).toBe('new');
    expect(added?.getAttribute('data-diff-label')).toBeTruthy();
    const removed = root.querySelector('[data-diff="removed"]');
    expect(removed?.tagName).toBe('DIV');
    expect(removed?.querySelector('img')?.getAttribute('data-file-id')).toBe('f1');
    expect(root.querySelectorAll(`[${DIFF_STOP}]`)).toHaveLength(2);
    // What is the same in both is drawn as it is, unmarked.
    expect(root.querySelector('p:not([data-diff])')?.textContent).toBe('kept');
  });

  it('says what a heading was and that a task was checked, the task keeping its box first', () => {
    const root = drawn(
      doc(heading(2, text('Plan')), { type: 'taskList', content: [task(false, 'call')] }),
      doc(heading(1, text('Plan')), { type: 'taskList', content: [task(true, 'call')] }),
    );
    const noted = root.querySelector('.doc-diff-noted');
    expect(noted?.querySelector('.doc-diff-note')?.textContent).toContain('→');
    expect(noted?.querySelector('h1')?.textContent).toBe('Plan');
    const checked = root.querySelector('li[data-checked="true"]');
    expect(checked?.firstElementChild?.tagName).toBe('LABEL');
    expect(checked?.lastElementChild?.firstElementChild?.className).toBe('doc-diff-note');
    expect(checked?.hasAttribute(DIFF_STOP)).toBe(true);
  });

  it('makes a task checked and reworded one stop, not one inside another', () => {
    const root = drawn(
      doc({ type: 'taskList', content: [task(false, 'call the bank')] }),
      doc({ type: 'taskList', content: [task(true, 'call the office')] }),
    );
    const stops = root.querySelectorAll(`[${DIFF_STOP}]`);
    expect([...stops].map((stop) => stop.tagName)).toEqual(['LI']);
    expect(root.querySelector('li del')?.textContent).toBe('bank');
  });

  it('keeps the stops of the items of a list made numbered, and goes into the list from its own', async () => {
    const before = doc(bullets(item(paragraph(text('milk'))), item(paragraph(text('bread')))));
    const after = doc(
      numbered(undefined, item(paragraph(text('milk'))), item(paragraph(text('jam')))),
    );
    const root = drawn(before, after);
    const stops = [...root.querySelectorAll<HTMLElement>(`[${DIFF_STOP}]`)];
    expect(stops.map((stop) => stop.tagName)).toEqual(['DIV', 'P']);
    expect(stops[1]?.textContent).toBe('breadjam');

    // The way through them, by the button: the list, the item in it, and round to the list.
    HTMLElement.prototype.scrollIntoView = () => undefined;
    const view = render(VersionDiff, { diff: diffBodies(before, after) });
    const next = view.getByTestId('body-diff-next');
    const shown = [...view.container.querySelectorAll<HTMLElement>(`[${DIFF_STOP}]`)];
    for (const expected of [0, 1, 0]) {
      next.click();
      await vi.waitFor(() => {
        expect(document.activeElement).toBe(shown[expected]);
      });
    }
    view.unmount();
  });

  it('puts a diagram added or removed in an element that carries the mark, where its picture goes too', () => {
    const root = drawn(
      doc(code(DIAGRAM_LANGUAGE, 'graph TD\nA-->B')),
      doc(code(DIAGRAM_LANGUAGE, 'graph TD\nA-->C')),
    );
    const marked = [...root.querySelectorAll('[data-diff]')];
    expect(marked.map((one) => [one.tagName, one.getAttribute('data-diff')])).toEqual([
      ['DIV', 'removed'],
      ['DIV', 'added'],
    ]);
    // The picture is put after the source, inside the element marked.
    for (const one of marked) expect(one.firstElementChild?.tagName).toBe('PRE');
    expect(root.querySelectorAll(`pre[${DIFF_STOP}]`)).toHaveLength(0);
  });

  it('keeps a list a list: the item changed is marked in it', () => {
    const root = drawn(
      doc(bullets(item(paragraph(text('milk'))), item(paragraph(text('bread'))))),
      doc(bullets(item(paragraph(text('milk'))), item(paragraph(text('jam'))))),
    );
    expect(root.querySelectorAll('ul > li')).toHaveLength(2);
    expect(root.querySelector('ul > li:nth-child(2) del')?.textContent).toBe('bread');
  });
});

describe('a table', () => {
  const grid = (texts: string[][]): DocNode =>
    table(...texts.map((cells) => row(...cells.map((one) => cell({}, paragraph(text(one)))))));

  it('is drawn as a table: one changed as the table it was, removed, and the one it is, added', () => {
    const before = grid([
      ['a', 'b'],
      ['c', 'd'],
    ]);
    const after = grid([
      ['a', 'b'],
      ['c', 'e'],
    ]);
    const last = (frame: Element | undefined): string | null | undefined =>
      frame?.querySelector('table tr:last-child td:last-child')?.textContent;
    const same = drawn(doc(paragraph(text('kept')), before), doc(paragraph(text('kept')), before));
    expect(same.querySelectorAll('[data-diff]')).toHaveLength(0);
    expect(last(same.querySelector('.tableWrapper') ?? undefined)).toBe('d');
    const root = drawn(doc(before), doc(after));
    const frames = [...root.querySelectorAll('.tableWrapper')];
    expect(frames.map((frame) => [frame.getAttribute('data-diff'), last(frame)])).toEqual([
      ['removed', 'd'],
      ['added', 'e'],
    ]);
    expect(root.querySelectorAll(`[${DIFF_STOP}]`)).toHaveLength(2);
  });
});

describe('a code block whose lines changed', () => {
  it('keeps its marks through its colours, copies the lines it has now, and numbers its lines', async () => {
    const root = drawn(
      doc(code('ts', 'const a = 1;\nconst b = 2;')),
      doc(code('ts', 'const a = 1;\nconst b = 3;')),
    );
    expect(root.querySelector('pre')?.getAttribute('data-diff')).toBe('changed');
    drawCodeBlocks(root, CODE_COPY_TEXTS);
    const written = clipboardOf(window);
    const codeElement = root.querySelector('.doc-code pre > code');
    await vi.waitFor(() => {
      expect(codeRuns(codeElement)).toContainEqual(['doc-code-keyword', 'const']);
    });
    // The old last line ends where the new one starts: a line between them, in neither.
    expect(codeElement?.querySelector('del')?.textContent).toBe('const b = 2;');
    expect(codeElement?.querySelector('[data-change="break"]')?.textContent).toBe('\n');
    expect(codeElement?.querySelector('ins')?.textContent).toBe('const b = 3;');
    expect(codeElement?.querySelector('ins .doc-code-keyword')?.textContent).toBe('const');
    expect(codeElement?.querySelector('ins')?.getAttribute('data-diff-label')).toBeTruthy();
    root.querySelector<HTMLButtonElement>('button.doc-code-copy')?.click();
    await vi.waitFor(() => {
      expect(written).toEqual(['const a = 1;\nconst b = 3;']);
    });
  });
});

describe('a stored body', () => {
  it('cannot carry the marks of a comparison: the validator refuses it', () => {
    const marked = doc(paragraph(text('x', { type: 'diff', attrs: { change: 'added' } })));
    expect(renderBody(marked).ok).toBe(false);
  });
});

describe('what a comparison says of itself', () => {
  it('says a list numbered from elsewhere, from what number to what, as a stop', () => {
    const before = doc(numbered(undefined, item(paragraph(text('one')))));
    const after = doc(numbered(5, item(paragraph(text('one')))));
    const root = drawn(before, after);
    const noted = root.querySelector('.doc-diff-noted');
    expect(noted?.querySelector('.doc-diff-note')?.textContent).toBe('First number: 1 → 5');
    expect(noted?.hasAttribute(DIFF_STOP)).toBe(true);
    expect(noted?.querySelector('ol')?.getAttribute('start')).toBe('5');
  });

  it('counts one block reshaped as one, and more as more', () => {
    const one = render(VersionDiff, {
      diff: diffBodies(doc(heading(1, text('a'))), doc(heading(2, text('a')))),
    });
    expect(one.getByTestId('body-diff-counts').textContent).toContain('1 block reshaped');
    one.unmount();
    const two = render(VersionDiff, {
      diff: diffBodies(
        doc(heading(1, text('a')), heading(1, text('b'))),
        doc(heading(2, text('a')), heading(2, text('b'))),
      ),
    });
    expect(two.getByTestId('body-diff-counts').textContent).toContain('2 blocks reshaped');
    two.unmount();
  });

  it('folds again what was opened, and opens one stretch without the others', async () => {
    const same = (count: number, from: number): DocNode[] =>
      Array.from({ length: count }, (_, index) => paragraph(text(`same ${String(from + index)}`)));
    const before = doc(
      paragraph(text('first')),
      ...same(5, 0),
      paragraph(text('middle')),
      ...same(5, 10),
    );
    const after = doc(
      paragraph(text('First')),
      ...same(5, 0),
      paragraph(text('Middle')),
      ...same(5, 10),
    );
    const view = render(VersionDiff, { diff: diffBodies(before, after) });
    const folds = (): HTMLElement[] => view.queryAllByTestId('body-diff-fold');
    expect(folds()).toHaveLength(2);
    folds()[1]?.click();
    await vi.waitFor(() => {
      expect(folds()).toHaveLength(1);
    });
    expect(view.container.textContent).toContain('same 14');
    expect(view.container.textContent).not.toContain('same 2');
    // What was opened can be folded again, and the focus goes to what takes its place.
    const refold = view.getByTestId('body-diff-refold');
    refold.click();
    await vi.waitFor(() => {
      expect(folds()).toHaveLength(2);
    });
    expect(view.container.textContent).not.toContain('same 14');
    expect(document.activeElement).toBe(folds()[1]);
    view.unmount();
  });
});

describe('things that are not text, marked', () => {
  it('draws a mention, a formula and a reference added inside the mark, a file and a formula block whole', () => {
    const root = drawn(
      doc(paragraph(text('see')), fileBlock('f1')),
      doc(
        paragraph(text('see '), mention('u1'), text(' '), math('x^2'), text(' '), reference('c9')),
        mathBlock('y^2'),
      ),
    );
    expect(root.querySelector('p ins [data-mention]')?.getAttribute('data-user-id')).toBe('u1');
    expect(root.querySelector('p ins [data-reference]')).not.toBeNull();
    expect(root.querySelector('p ins [data-latex], p ins .doc-math')).not.toBeNull();
    const removed = root.querySelector('[data-diff="removed"]');
    expect(removed?.getAttribute('data-diff-label')).toBe('Removed block');
    // The file block is itself the element marked: it holds what is drawn into it later.
    expect(removed?.outerHTML).toContain('"f1"');
    expect(root.querySelector('[data-diff="added"]')?.getAttribute('data-diff-label')).toBe(
      'Added block',
    );
  });

  it("says a code block's language changed, in the names of the languages", () => {
    const root = drawn(doc(code('ts', 'x')), doc(code('js', 'x')));
    expect(root.querySelector('.doc-diff-note')?.textContent).toBe(
      'Language: TypeScript → JavaScript',
    );
  });
});
