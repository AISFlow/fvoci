/**
 * The tables of a body edited together on the backend in memory: what two
 * merged changes leave unfilled is mended as the server mends it (the
 * server's own repairs, `crates/collab/tests/data/made-here.json`, are the
 * measure), once, whether the two wrote in one world or in two tabs joined.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import type { DocNode } from '$lib/editor/schema.ts';
import { MAX_TABLE_COLUMNS, MAX_TABLE_ROWS } from '$lib/editor/table-grid.ts';
import { lostNote } from '$lib/features/card/body-notes.ts';
import { setLocale } from '$lib/i18n/paraglide/runtime.js';
import type { BodyDoc, BodyDocHears } from '$lib/state/backend.ts';
import { bodyOf, FIELD, roomsOf } from '$lib/state/memory/rooms.ts';
import { documentOf } from '$lib/state/memory/room-docs.ts';
import { mendTables } from '$lib/state/memory/table-mend.ts';
import type { StoredCard } from '$lib/state/memory/world.ts';

import { fromBase64, readerOf, YDOC_VECTORS_FILE } from '../../tools/ydoc/vectors.ts';

import { setup } from './support.ts';

interface Case {
  name: string;
  json: DocNode;
  update: string;
}
const vectors = JSON.parse(readFileSync(YDOC_VECTORS_FILE, 'utf8')) as Record<
  'refused' | 'shortened',
  Case[]
>;
const made = JSON.parse(
  readFileSync(
    path.resolve(import.meta.dirname, '../../../../crates/collab/tests/data/made-here.json'),
    'utf8',
  ),
) as { mended: { name: string; json: DocNode | null }[] };
const mended = new Map(made.mended.map((one) => [one.name, one.json]));

/** The cases of a table: all but one past a limit, which the server refuses too. */
const TABLES = [...vectors.refused, ...vectors.shortened].filter((one) =>
  JSON.stringify(one.json).includes('"type":"table"'),
);

const hears = (): BodyDocHears => ({ state: () => undefined, discarded: () => undefined });

const paragraph = (text: string): DocNode =>
  text === '' ? { type: 'paragraph' } : { type: 'paragraph', content: [{ type: 'text', text }] };
const tableOf = (rows: string[][]): DocNode => ({
  type: 'doc',
  content: [
    {
      type: 'table',
      content: rows.map((cells) => ({
        type: 'tableRow',
        content: cells.map((text) => ({ type: 'tableCell', content: [paragraph(text)] })),
      })),
    },
  ],
});

/** A cell of a shared document, holding a line of text. */
function cell(text: string): Y.XmlElement {
  const line = new Y.XmlElement('paragraph');
  line.insert(0, [new Y.XmlText(text)]);
  const made = new Y.XmlElement('tableCell');
  made.insert(0, [line]);
  return made;
}
const tableIn = (doc: Y.Doc): Y.XmlElement => doc.getXmlFragment(FIELD).get(0) as Y.XmlElement;
const rowsOf = (doc: Y.Doc): Y.XmlElement[] => tableIn(doc).toArray() as Y.XmlElement[];

/** A column added at the end of every row, as an editor adds one. */
function addColumn(doc: Y.Doc, texts: string[]): void {
  doc.transact(() => {
    for (const [index, row] of rowsOf(doc).entries())
      row.insert(row.length, [cell(texts[index] ?? '')]);
  });
}

/** A row of two cells added at the end of the table. */
function addRow(doc: Y.Doc, texts: string[]): void {
  const row = new Y.XmlElement('tableRow');
  row.insert(0, texts.map(cell));
  const table = tableIn(doc);
  table.insert(table.length, [row]);
}

/** A world whose card holds a table of two rows by two columns. */
function world(): ReturnType<typeof setup> & { boardId: string; cardId: string } {
  const made = setup();
  const card = [...made.world.blocks.values()].find((block) => block.type === 'card') as
    StoredCard | undefined;
  if (!card) throw new Error('the seed has no card');
  card.body = tableOf([
    ['a', 'b'],
    ['c', 'd'],
  ]);
  return { ...made, boardId: card.containerId, cardId: card.id };
}

async function joined(body: BodyDoc): Promise<Y.Doc> {
  await vi.waitFor(() => {
    expect(body.state.status).toBe('joined');
  });
  const { doc } = body.state;
  if (!doc) throw new Error('joined without a document');
  return doc;
}

const MERGED = tableOf([
  ['a', 'b', 'x'],
  ['c', 'd', 'y'],
  ['e', 'f', ''],
]);

describe('the tables of a room in memory', () => {
  it('are mended for every case of a table the server mends', () => {
    expect(TABLES.map((one) => one.name)).toContain('one adds a column while the other adds a row');
    for (const one of TABLES) {
      const doc = readerOf(fromBase64(one.update));
      const wrote = mendTables(doc);
      if (!mended.has(one.name)) {
        // Past a limit: no mending brings it under, and the room refuses it.
        expect(bodyOf(doc), one.name).toBeUndefined();
        continue;
      }
      expect(wrote, one.name).toBeDefined();
      expect(bodyOf(doc), one.name).toEqual(mended.get(one.name));
      // Mended once, there is nothing more to mend.
      expect(mendTables(doc), one.name).toBeUndefined();
    }
  });

  it('leaves a table that fills its grid as it is, a row the cells above cover wholly too', () => {
    // As it is stored: a row with nothing in it has no content, a span of one is not written.
    const filled: DocNode = {
      type: 'doc',
      content: [
        {
          type: 'table',
          content: [
            {
              type: 'tableRow',
              content: [
                { type: 'tableCell', attrs: { colspan: 2, rowspan: 2 }, content: [paragraph('a')] },
              ],
            },
            { type: 'tableRow' },
            ...(tableOf([['b', 'c']]).content?.[0]?.content ?? []),
          ],
        },
      ],
    };
    const doc = documentOf(filled);
    const before = Y.encodeStateAsUpdate(doc);
    expect(mendTables(doc)).toBeUndefined();
    expect(Y.encodeStateAsUpdate(doc)).toEqual(before);
    expect(bodyOf(doc)).toEqual(filled);
  });

  it('takes away a table in a toggle that holds no cell, and the toggles that then hide nothing', () => {
    const toggle = (summary: string, hidden: DocNode[]): DocNode => ({
      type: 'details',
      content: [
        { type: 'detailsSummary', content: [{ type: 'text', text: summary }] },
        { type: 'detailsContent', content: hidden },
      ],
    });
    const table: DocNode = {
      type: 'table',
      content: [{ type: 'tableRow', content: [{ type: 'tableCell', content: [paragraph('a')] }] }],
    };
    const emptied = (body: DocNode): Y.Doc => {
      const doc = documentOf(body);
      const found: Y.XmlElement[] = [];
      const walk = (node: Y.XmlFragment | Y.XmlElement): void => {
        for (const child of node.toArray())
          if (child instanceof Y.XmlElement) {
            if (child.nodeName === 'tableRow') found.push(child);
            else walk(child);
          }
      };
      walk(doc.getXmlFragment(FIELD));
      doc.transact(() => {
        for (const row of found) row.delete(0, row.length);
      });
      return doc;
    };
    // Its toggle hides only it, and that toggle is all its outer toggle hides: both go.
    const alone = emptied({
      type: 'doc',
      content: [paragraph('before'), toggle('outer', [toggle('inner', [table])])],
    });
    expect(mendTables(alone)).toBeDefined();
    expect(bodyOf(alone)).toEqual({ type: 'doc', content: [paragraph('before')] });
    // The outer toggle hides a line too: only the inner one goes.
    const beside = emptied({
      type: 'doc',
      content: [toggle('outer', [paragraph('kept'), toggle('inner', [table])])],
    });
    expect(mendTables(beside)).toBeDefined();
    expect(bodyOf(beside)).toEqual({
      type: 'doc',
      content: [toggle('outer', [paragraph('kept')])],
    });
  });

  it('puts a failing cell back after the cell before it, so that two tabs that saw different ones still agree', () => {
    // The cell after `a` failed; one tab has also had `n` put before it, the other not yet.
    const one = documentOf(tableOf([['a', 'f']]));
    const failing = rowsOf(one)[0]?.get(1) as Y.XmlElement;
    one.transact(() => {
      failing.delete(0, failing.length);
    });
    const other = new Y.Doc();
    Y.applyUpdate(other, Y.encodeStateAsUpdate(one));
    other.clientID = 0x7fff_fff0;
    rowsOf(other)[0]?.insert(1, [cell('n')]);
    expect(mendTables(one)).toBeDefined();
    expect(mendTables(other)).toBeDefined();
    Y.applyUpdate(one, Y.encodeStateAsUpdate(other, Y.encodeStateVector(one)));
    Y.applyUpdate(other, Y.encodeStateAsUpdate(one, Y.encodeStateVector(other)));
    // Both puts stand, each after the cell its tab saw: one table, the same in both.
    expect(bodyOf(one)).toBeDefined();
    expect(bodyOf(one)).toEqual(bodyOf(other));
  });

  it('takes a column and a row added at once by two readers, and mends the row once', async () => {
    const { world: stored, backend, other, boardId, cardId } = world();
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const theirs = other.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([joined(mine), joined(theirs)]);
    addColumn(a, ['x', 'y']);
    addRow(b, ['e', 'f']);
    await vi.waitFor(() => {
      expect(bodyOf(a)).toEqual(MERGED);
      expect(bodyOf(b)).toEqual(MERGED);
      expect(mine.state.pending || theirs.state.pending).toBe(false);
    });
    expect(mine.state.status).toBe('joined');
    expect(theirs.state.status).toBe('joined');
    roomsOf(stored).flush();
    expect(stored.blocks.get(cardId)?.body).toEqual(MERGED);
    mine.close();
    theirs.close();
  });

  it('takes the deletion of each column of the only table by two readers, which leaves no body', async () => {
    const { world: stored, backend, other, boardId, cardId } = world();
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const theirs = other.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([joined(mine), joined(theirs)]);
    const deleteColumn = (doc: Y.Doc, column: number): void => {
      doc.transact(() => {
        for (const row of rowsOf(doc)) row.delete(column);
      });
    };
    deleteColumn(a, 0);
    deleteColumn(b, 1);
    await vi.waitFor(() => {
      expect(a.getXmlFragment(FIELD).length).toBe(0);
      expect(b.getXmlFragment(FIELD).length).toBe(0);
      expect(mine.state.pending || theirs.state.pending).toBe(false);
    });
    expect(mine.state.status).toBe('joined');
    expect(theirs.state.status).toBe('joined');
    expect(bodyOf(a)).toBeNull();
    roomsOf(stored).flush();
    expect(stored.blocks.get(cardId)?.body).toBeNull();
    mine.close();
    theirs.close();
  });

  it('takes a change that empties the body, which needs no mending', async () => {
    const { world: stored, backend, boardId, cardId } = world();
    const mine = backend.openDoc('w', boardId, cardId, hears());
    const a = await joined(mine);
    // As an editor empties it: one empty paragraph.
    const fragment = a.getXmlFragment(FIELD);
    a.transact(() => {
      fragment.delete(0, fragment.length);
      fragment.insert(0, [new Y.XmlElement('paragraph')]);
    });
    await vi.waitFor(() => {
      expect(mine.state.pending).toBe(false);
    });
    expect(mine.state.status).toBe('joined');
    roomsOf(stored).flush();
    expect(stored.blocks.get(cardId)?.body).toBeNull();
    mine.close();
  });

  it('mends the same in the rooms of two tabs joined, so that the two mendings are one', async () => {
    const first = world();
    const second = world();
    const channel = `table-test-${crypto.randomUUID()}`;
    first.world.shareRooms = channel;
    second.world.shareRooms = channel;
    const { boardId, cardId } = first;
    const here = first.backend.openDoc('w', boardId, cardId, hears());
    const there = second.backend.openDoc('w', boardId, cardId, hears());
    const [a, b] = await Promise.all([joined(here), joined(there)]);
    // The two rooms know each other's documents before the two write.
    addRow(a, ['0', '0']);
    await vi.waitFor(() => {
      expect(rowsOf(b)).toHaveLength(3);
    });
    tableIn(a).delete(2);
    await vi.waitFor(() => {
      expect(rowsOf(b)).toHaveLength(2);
    });

    addColumn(a, ['x', 'y']);
    addRow(b, ['e', 'f']);
    await vi.waitFor(() => {
      expect(bodyOf(a)).toEqual(MERGED);
      expect(bodyOf(b)).toEqual(MERGED);
      expect(Y.encodeStateVector(a)).toEqual(Y.encodeStateVector(b));
    });
    for (const { world: stored } of [first, second]) {
      roomsOf(stored).flush();
      expect(stored.blocks.get(cardId)?.body).toEqual(MERGED);
    }
    here.close();
    there.close();
  });

  it('refuses a 201st row and a 21st column with table_too_large and leaves the table untouched', async () => {
    const cases = [
      {
        name: '201 rows',
        initial: tableOf(Array.from({ length: MAX_TABLE_ROWS }, () => ['a'])),
        exceed: (doc: Y.Doc) => {
          addRow(doc, ['extra']);
        },
      },
      {
        name: '21 columns',
        initial: tableOf([
          Array.from({ length: MAX_TABLE_COLUMNS }, () => 'a'),
          Array.from({ length: MAX_TABLE_COLUMNS }, () => 'b'),
        ]),
        exceed: (doc: Y.Doc) => {
          addColumn(doc, ['x', 'y']);
        },
      },
    ];
    for (const testCase of cases) {
      const { world: stored, backend, boardId, cardId } = world();
      const card = stored.blocks.get(cardId) as StoredCard;
      card.body = testCase.initial;
      const discarded: string[] = [];
      const mine = backend.openDoc('w', boardId, cardId, {
        state: () => undefined,
        discarded: (why) => {
          discarded.push(why);
        },
      });
      try {
        const doc = await joined(mine);
        testCase.exceed(doc);
        await vi.waitFor(() => {
          expect(discarded, testCase.name).toEqual(['table_too_large']);
          expect(mine.state.status, testCase.name).toBe('joined');
          expect(mine.state.pending, testCase.name).toBe(false);
        });
        expect(mine.state.doc ? bodyOf(mine.state.doc) : undefined, testCase.name).toEqual(
          testCase.initial,
        );
        roomsOf(stored).flush();
        expect(stored.blocks.get(cardId)?.body, testCase.name).toEqual(testCase.initial);

        // Room stays writable after the refusal: a subsequent valid edit is accepted.
        const recovered = mine.state.doc;
        expect(recovered, testCase.name).toBeDefined();
        recovered?.transact(() => {
          const line = new Y.XmlElement('paragraph');
          line.insert(0, [new Y.XmlText('after refusal')]);
          const fragment = recovered.getXmlFragment(FIELD);
          fragment.insert(fragment.length, [line]);
        });
        await vi.waitFor(() => {
          expect(mine.state.pending, testCase.name).toBe(false);
          expect(mine.state.doc ? bodyOf(mine.state.doc)?.content?.length : 0, testCase.name).toBe(
            2,
          );
        });
        expect(discarded, testCase.name).toEqual(['table_too_large']);
      } finally {
        mine.close();
      }
    }
  });

  it('maps table_too_large to dedicated en and ko limit notices', async () => {
    try {
      await setLocale('en', { reload: false });
      const en = lostNote('table_too_large');
      expect(en).toContain('200');
      expect(en).toContain('20');
      expect(en).not.toBe(lostNote('document_too_large'));

      await setLocale('ko', { reload: false });
      const ko = lostNote('table_too_large');
      expect(ko).toContain('200');
      expect(ko).toContain('20');
      expect(ko).not.toBe(lostNote('document_too_large'));
    } finally {
      await setLocale('en', { reload: false });
    }
  });
});
