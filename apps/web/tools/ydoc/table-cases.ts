/**
 * The tables of `contracts/ydoc-vectors.json`, written by hand (see
 * `cases.ts` for the sections): shared content no editor writes but a merge
 * of two edits may leave.
 *
 * A table that does not fill its grid fits the editor's schema, which counts
 * no columns, so the editors keep it and only the server's rules refuse it:
 * the server mends it (`crates/collab/src/mend.rs`). `SHORTENED` is new to
 * tables: a cell that fails is deleted by every editor, which leaves its row
 * a cell short, so what an editor reads is no body; the server deletes the
 * cell too and puts an empty one in its place.
 */
import * as Y from 'yjs';

import type { RawCase } from './cases.ts';
import { rawType, type AttributeValue, type RawElement, type RawNode } from './shape.ts';

/** Two edits, each made to the base without the other, then merged. */
export interface MergedCase {
  name: string;
  base: RawNode[];
  first: (root: Y.XmlFragment) => void;
  second: (root: Y.XmlFragment) => void;
}

const text = (insert: string): RawNode => ({ text: [{ insert }] });
const paragraph = (...children: RawNode[]): RawNode => ({ element: 'paragraph', children });
const line = (insert: string): RawNode => paragraph(text(insert));
const table = (...children: RawNode[]): RawNode => ({ element: 'table', children });
const row = (...children: RawNode[]): RawNode => ({ element: 'tableRow', children });
const cellOf =
  (element: string) =>
  (children: RawNode[], attrs?: Record<string, AttributeValue>): RawNode =>
    attrs ? { element, attrs, children } : { element, children };
const cell = cellOf('tableCell');
const header = cellOf('tableHeader');
/** `count` cells, each one line of its column's number from `from`. */
const numbered = (from: number, count: number): RawNode[] =>
  Array.from({ length: count }, (_, index) => cell([line(String(from + index))]));

/** The element at a path of indexes below a fragment: the root is none, so one index at least. */
function at(parent: Y.XmlFragment, ...indexes: number[]): Y.XmlElement {
  let node: Y.XmlFragment = parent;
  let element: Y.XmlElement | undefined;
  for (const index of indexes) {
    const child = node.get(index);
    if (!(child instanceof Y.XmlElement)) throw new Error(`no element at ${indexes.join('/')}`);
    node = element = child;
  }
  if (!element) throw new Error('the root is no element');
  return element;
}

const setSpan = (element: Y.XmlElement, name: string, span: number): void => {
  (element as unknown as RawElement).setAttribute(name, span);
};

/** A table of two rows of two cells, with a paragraph before it. */
const twoByTwo = (): RawNode[] => [
  line('before'),
  table(row(cell([line('a')]), cell([line('b')])), row(cell([line('c')]), cell([line('d')]))),
];

export const TABLE_REFUSED: RawCase[] = [
  {
    name: 'a table whose second row is a cell short',
    root: [table(row(cell([line('a')]), cell([line('b')])), row(cell([line('c')])))],
  },
  {
    name: 'a row of headers a cell short',
    root: [table(row(header([line('a')])), row(cell([line('b')]), cell([line('c')])))],
  },
  {
    name: 'a table whose second row has no cell, which no cell above covers',
    root: [table(row(cell([line('a')]), cell([line('b')])), row())],
  },
  {
    name: 'a cell whose rowspan reaches past the last row',
    root: [
      table(row(cell([line('a')], { rowspan: 3 }), cell([line('b')])), row(cell([line('c')]))),
    ],
  },
  {
    name: 'a cell whose colspan runs into a cell that reaches down from above',
    root: [
      table(
        row(cell([line('a')]), cell([line('b')], { rowspan: 2 })),
        row(cell([line('c')], { colspan: 2 })),
      ),
    ],
  },
  {
    name: 'spans that are no whole number from one up',
    root: [
      table(
        row(cell([line('a')], { colspan: '2' }), cell([line('b')], { rowspan: 0 })),
        row(cell([line('c')], { colspan: 1.5 }), cell([line('d')])),
      ),
    ],
  },
  {
    name: 'a table whose only row has no cell',
    root: [line('a'), table(row())],
  },
  {
    name: 'a table of twenty-one columns',
    root: [table(row(...Array.from({ length: 21 }, (_, index) => cell([line(String(index))]))))],
  },
];

export const TABLE_REPAIRS: RawCase[] = [
  { name: 'a table without rows', root: [line('a'), table()] },
  { name: 'a cell straight in a table', root: [line('a'), table(cell([line('b')]))] },
  {
    name: 'a row in a quote',
    root: [line('a'), { element: 'blockquote', children: [row(cell([line('b')]))] }],
  },
  {
    name: 'a table in a quote',
    root: [line('a'), { element: 'blockquote', children: [table(row(cell([line('b')])))] }],
  },
  {
    name: 'a row that holds a paragraph beside its cell: the row goes, the table stays',
    root: [table(row(cell([line('a')])), row(cell([line('b')]), line('c')))],
  },
];

export const TABLE_SHORTENED: RawCase[] = [
  {
    name: 'a cell with nothing in it',
    root: [table(row(cell([]), cell([line('b')])), row(cell([line('c')]), cell([line('d')])))],
  },
  {
    name: 'a cell that holds a text with no paragraph',
    root: [
      table(
        row(cell([line('a')]), cell([text('loose')])),
        row(cell([line('c')]), cell([line('d')])),
      ),
    ],
  },
  {
    name: 'a cell that holds a table',
    root: [
      table(
        row(cell([line('a')]), cell([table(row(cell([line('inner')])))])),
        row(cell([line('c')]), cell([line('d')])),
      ),
    ],
  },
  {
    name: 'a cell with nothing in it, after a text with nothing in it',
    root: [
      table(
        row({ text: [] }, cell([line('a')]), cell([]), cell([line('c')])),
        row(cell([line('d')]), cell([line('e')]), cell([line('f')])),
      ),
    ],
  },
  {
    name: 'a cell with nothing in it, in a table of twenty columns',
    root: [table(row(cell([]), ...numbered(1, 19)), row(...numbered(0, 20)))],
  },
];

/** What two edits leave that only the server's rules refuse: the server mends it. */
export const TABLE_MERGED_REFUSED: MergedCase[] = [
  {
    name: 'one adds a column while the other adds a row',
    base: twoByTwo(),
    first: (root) => {
      for (const index of [0, 1])
        at(root, 1, index).insert(2, [rawType(cell([line(`new ${index}`)]))]);
    },
    second: (root) => {
      at(root, 1).insert(2, [rawType(row(cell([line('e')]), cell([line('f')])))]);
    },
  },
  {
    name: 'one deletes a column while the other adds a row',
    base: twoByTwo(),
    first: (root) => {
      for (const index of [0, 1]) at(root, 1, index).delete(1, 1);
    },
    second: (root) => {
      at(root, 1).insert(2, [rawType(row(cell([line('e')]), cell([line('f')])))]);
    },
  },
  {
    name: 'one joins the two cells of a column while the other deletes the lower row',
    base: twoByTwo(),
    first: (root) => {
      setSpan(at(root, 1, 0, 0), 'rowspan', 2);
      at(root, 1, 1).delete(0, 1);
    },
    second: (root) => {
      at(root, 1).delete(1, 1);
    },
  },
];

/** What two edits leave that every editor shortens. */
export const TABLE_MERGED_SHORTENED: MergedCase[] = [
  {
    name: 'two people each delete one of the two paragraphs of a cell',
    base: [
      table(
        row(cell([line('a'), line('more')]), cell([line('b')])),
        row(cell([line('c')]), cell([line('d')])),
      ),
    ],
    first: (root) => {
      at(root, 0, 0, 0).delete(0, 1);
    },
    second: (root) => {
      at(root, 0, 0, 0).delete(1, 1);
    },
  },
];
