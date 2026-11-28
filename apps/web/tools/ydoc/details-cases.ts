/**
 * The toggles, callouts and board views of `contracts/ydoc-vectors.json`,
 * written by hand (see `cases.ts` for the sections).
 *
 * A toggle that does not hold its two parts in their order, or what it
 * hides without a block, fails the editor's schema: every editor deletes
 * it whole, and so does the server. There is nothing of a toggle to mend,
 * as there is of a table: a toggle two people emptied together goes. A
 * callout of a tone not on the list fits the editor's schema, which checks
 * no value; only the server's rules refuse it. So does a board view that
 * names no view; one where only a block of a quote, a callout or a line
 * stands fails the schema, and what holds it goes. (An eleventh board view
 * is past a limit of the body, which a repair does not mend: the update that
 * makes it is refused.)
 */
import * as Y from 'yjs';

import type { RawCase } from './cases.ts';
import { rawType, type AttributeValue, type RawNode } from './shape.ts';
import type { MergedCase } from './table-cases.ts';

const text = (insert: string): RawNode => ({ text: [{ insert }] });
const line = (insert: string): RawNode => ({ element: 'paragraph', children: [text(insert)] });
const summary = (...children: RawNode[]): RawNode => ({ element: 'detailsSummary', children });
const content = (...children: RawNode[]): RawNode => ({ element: 'detailsContent', children });
const details = (...children: RawNode[]): RawNode => ({ element: 'details', children });
const toggle = (title: string, ...hidden: RawNode[]): RawNode =>
  details(summary(text(title)), content(...hidden));
const callout = (tone: AttributeValue | undefined, ...children: RawNode[]): RawNode =>
  tone === undefined
    ? { element: 'callout', children }
    : { element: 'callout', attrs: { tone }, children };
const quote = (...children: RawNode[]): RawNode => ({ element: 'blockquote', children });
const cell = (...children: RawNode[]): RawNode => ({ element: 'tableCell', children });
const row = (...children: RawNode[]): RawNode => ({ element: 'tableRow', children });
const view = (viewId: AttributeValue, ...children: RawNode[]): RawNode => ({
  element: 'boardView',
  attrs: { viewId },
  children,
});
/** The identifier of the `n`th view. */
const viewNumber = (n: number): string =>
  `01930000-0000-7000-8000-${(0xc001 + n).toString(16).padStart(12, '0')}`;

/** The element at a path of indexes below a fragment. */
function at(parent: Y.XmlFragment, ...indexes: number[]): Y.XmlElement {
  let node: Y.XmlFragment = parent;
  let element: Y.XmlElement | undefined;
  for (const index of indexes) {
    const child: unknown = node.get(index);
    if (!(child instanceof Y.XmlElement)) throw new Error(`no element at ${indexes.join('/')}`);
    element = child as Y.XmlElement;
    node = element;
  }
  if (!element) throw new Error('the root is no element');
  return element;
}

export const DETAILS_READINGS: RawCase[] = [
  {
    name: 'a callout without a tone is a note',
    root: [callout(undefined, line('n'))],
  },
  {
    name: 'a toggle that another client wrote open is read without it',
    root: [
      {
        element: 'details',
        attrs: { open: true },
        children: [summary(text('s')), content(line('hidden'))],
      },
    ],
  },
];

export const DETAILS_REFUSED: RawCase[] = [
  { name: 'a callout of a tone not on the list', root: [callout('purple', line('p')), line('a')] },
  { name: 'a callout whose tone is a number', root: [callout(2, line('p'))] },
  {
    name: 'a callout of a tone not on the list, in a toggle',
    root: [toggle('s', callout('INFO', line('p')), line('kept'))],
  },
  { name: 'a board view of a view that is no identifier', root: [line('a'), view('table-1')] },
  { name: 'a board view without its view', root: [{ element: 'boardView' }, line('a')] },
];

export const DETAILS_REPAIRS: RawCase[] = [
  { name: 'a toggle of a summary alone', root: [line('a'), details(summary(text('s')))] },
  {
    name: 'a toggle of its content before its summary',
    root: [line('a'), details(content(line('c')), summary(text('s')))],
  },
  {
    name: 'a toggle whose content holds no block',
    root: [line('a'), details(summary(text('s')), content())],
  },
  {
    name: 'a toggle whose summary holds a paragraph',
    root: [line('a'), details(summary(line('s')), content(line('c')))],
  },
  {
    name: 'a toggle in a quote: the quote goes',
    root: [line('a'), quote(toggle('s', line('c')))],
  },
  {
    name: 'a callout in a toggle in a callout: the outer callout goes',
    root: [line('a'), callout('info', toggle('s', callout('tip', line('c'))))],
  },
  {
    name: 'a callout that holds a table',
    root: [line('a'), callout('warn', { element: 'table', children: [row(cell(line('x')))] })],
  },
  { name: 'a callout with nothing in it', root: [line('a'), callout('tip')] },
  {
    name: 'a board view in a callout: the callout goes',
    root: [line('a'), callout('info', view(viewNumber(0)))],
  },
  {
    name: 'a board view in a quote: the quote goes',
    root: [line('a'), quote(view(viewNumber(0)))],
  },
  {
    name: 'a board view in a line: the line goes',
    root: [line('a'), { element: 'paragraph', children: [view(viewNumber(0))] }],
  },
  {
    name: 'a board view holding a line: it goes, and the board view in the toggle stays',
    root: [view(viewNumber(0), line('x')), toggle('s', view(viewNumber(1)))],
  },
  {
    name: 'a toggle whose second part is a paragraph, in a toggle: the inner one goes',
    root: [toggle('outer', line('kept'), details(summary(text('in')), line('loose')))],
  },
];

/** A toggle of two lines, with a paragraph before it. */
const twoLines = (): RawNode[] => [line('before'), toggle('s', line('one'), line('two'))];

/** What two edits leave that every editor deletes, and the server too. */
export const DETAILS_MERGED_REPAIRS: MergedCase[] = [
  {
    name: 'two people each delete one of the two lines a toggle hides: the toggle goes',
    base: twoLines(),
    first: (root) => {
      at(root, 1, 1).delete(0, 1);
    },
    second: (root) => {
      at(root, 1, 1).delete(1, 1);
    },
  },
];

/** What two edits leave that only the server's rules refuse: the server mends it. */
export const DETAILS_MERGED_REFUSED: MergedCase[] = [
  {
    name: 'in a toggle, one adds a column while the other adds a row',
    base: [
      line('before'),
      toggle('s', {
        element: 'table',
        children: [row(cell(line('a')), cell(line('b'))), row(cell(line('c')), cell(line('d')))],
      }),
    ],
    first: (root) => {
      for (const index of [0, 1])
        at(root, 1, 1, 0, index).insert(2, [rawType(cell(line(`new ${index}`)))]);
    },
    second: (root) => {
      at(root, 1, 1, 0).insert(2, [rawType(row(cell(line('e')), cell(line('f'))))]);
    },
  },
];
