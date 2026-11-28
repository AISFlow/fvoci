/**
 * The tables of a room's document mended as the server mends them
 * (`crates/collab/src/mend.rs`), for the rooms of the backend in memory
 * (rooms.ts): what two changes that are each valid can leave once merged. A
 * text with nothing in it among a row's cells goes, a cell that fails (left
 * empty, holding a table) is put back empty in its place, a span is narrowed
 * to what fits, and a row is given the empty cells it lacks at its end (one
 * without cells too, unless the cells above cover it); a table no row of which
 * holds a cell goes. Anything else that is wrong is left, for the room to
 * refuse. Tables are found where the document holds them: at its top and in
 * what a toggle hides, a toggle in a toggle too; a toggle left with nothing to
 * hide once its table goes goes too, whole, as the server's repair takes it.
 *
 * The rooms of the tabs joined together (`shareRooms`) each mend what they
 * hold. So that two that mend the same are one, as two makings of a document
 * are (room-docs.ts), each thing put is written as a client of its own, named
 * by where it goes and what it is: two tabs that put the same cell after the
 * same one write the same item, and two that put one after different ones
 * write two, never one item that is not the same in both. Deleting needs
 * none, as it is the same whoever does it.
 */
import * as Y from 'yjs';

import { BODY_FIELD } from '../../editor/body-editor.ts';
import { gridFit, type GridNode } from '../../editor/table-grid.ts';

import { bodySchema } from './room-docs.ts';

const CELLS = new Set(['tableCell', 'tableHeader']);

/** The name of an element of the document, from the item it is: the same in every tab. */
const nameOf = (type: { _item: Y.Item | null }): string =>
  type._item ? `${String(type._item.id.client)}.${String(type._item.id.clock)}` : 'start';

/**
 * A client named by a text: a whole number past 2³², which no editor's
 * client (picked at random below it) nor the maker of a document
 * (room-docs.ts) is, as the server's mending writes past it too.
 */
function clientOf(key: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < key.length; index += 1) {
    hash = Math.imul(hash ^ key.charCodeAt(index), 0x01000193) >>> 0;
  }
  return 2 ** 32 + hash;
}

/** Writes as the client `key` names, and as the document's own client again after. */
function writeAs(doc: Y.Doc, key: string, write: () => void): void {
  const own = doc.clientID;
  doc.clientID = clientOf(key);
  try {
    doc.transact(write);
  } finally {
    doc.clientID = own;
  }
}

const isElement = (node: unknown, names: Set<string>): node is Y.XmlElement =>
  node instanceof Y.XmlElement && names.has(node.nodeName);

/**
 * Whether an editor keeps a cell: its children, by their kinds, are what a
 * cell may hold. Only the kinds count, so the cell is not read whole.
 */
function cellStays(cell: Y.XmlElement): boolean {
  let match = bodySchema.nodes[cell.nodeName]?.contentMatch;
  for (const child of cell.toArray()) {
    const kind = child instanceof Y.XmlElement ? bodySchema.nodes[child.nodeName] : undefined;
    match = kind ? (match?.matchType(kind) ?? undefined) : undefined;
    if (!match) return false;
  }
  return match?.validEnd === true;
}

/** An empty cell of a kind, with the attributes given. */
function emptyCell(kind: string, attrs: Record<string, unknown> = {}): Y.XmlElement {
  const cell = new Y.XmlElement(kind);
  for (const [name, value] of Object.entries(attrs)) cell.setAttribute(name, value as string);
  cell.insert(0, [new Y.XmlElement('paragraph')]);
  return cell;
}

/**
 * Mends what is wrong with the rows and cells of a table, but its grid;
 * false when something else is wrong with it, which is not mended.
 */
function mendRows(doc: Y.Doc, table: Y.XmlElement): boolean {
  if (!table.toArray().every((row) => isElement(row, new Set(['tableRow'])))) return false;
  for (const row of table.toArray() as Y.XmlElement[]) {
    const children = row.toArray();
    // A text with nothing in it, which an editor passes over, goes first.
    for (let at = children.length - 1; at >= 0; at -= 1) {
      const child = children[at];
      if (child instanceof Y.XmlText && child.length === 0)
        doc.transact(() => {
          row.delete(at);
        });
    }
    const cells = row.toArray();
    if (!cells.every((cell) => isElement(cell, CELLS))) return false;
    for (const [at, cell] of cells.entries()) {
      if (cellStays(cell)) continue;
      const attrs = cell.getAttributes();
      const put = emptyCell(cell.nodeName, attrs);
      // After the cell before it as it is now: one put there already, if it failed too.
      const after = at > 0 ? nameOf(row.get(at - 1)) : 'start';
      const key = `put ${nameOf(row)} ${after} ${nameOf(cell)} ${JSON.stringify(attrs)}`;
      writeAs(doc, key, () => {
        row.delete(at);
        row.insert(at, [put]);
      });
    }
  }
  return true;
}

/** Whether no row of a table holds a cell, or it has no row: the grid has no column, and it goes. */
const holdsNoCell = (table: Y.XmlElement): boolean =>
  table.toArray().every((row) => row instanceof Y.XmlElement && row.length === 0);

/** Narrows the spans of a table's cells to what fits, and fills its rows' ends; false past a limit. */
function mendGrid(doc: Y.Doc, table: Y.XmlElement): boolean {
  const rows = table.toArray() as Y.XmlElement[];
  const cellsOf = (row: Y.XmlElement): Y.XmlElement[] => row.toArray() as Y.XmlElement[];
  const grid: GridNode = {
    content: rows.map((row) => ({
      content: cellsOf(row).map((cell) => ({ attrs: cell.getAttributes() })),
    })),
  };
  const fit = gridFit(grid);
  if (fit === 'tooLarge') return false;
  for (const [index, { cells, missing }] of fit.entries()) {
    const row = rows[index];
    if (!row) continue;
    const elements = cellsOf(row);
    for (const [at, { columns, rows: down }] of cells.entries()) {
      const cell = elements[at];
      if (!cell) continue;
      for (const [name, reach] of [
        ['colspan', columns],
        ['rowspan', down],
      ] as const) {
        if (!reach.changed) continue;
        writeAs(doc, `span ${nameOf(cell)} ${name} ${String(reach.span)}`, () => {
          cell.setAttribute(name, reach.span as unknown as string);
        });
      }
    }
    // A row of headers is given headers, a row of cells cells.
    const kind = elements.at(-1)?.nodeName ?? 'tableCell';
    for (let added = 0; added < missing; added += 1) {
      const after = row.toArray().at(-1);
      writeAs(doc, `pad ${nameOf(row)} ${after ? nameOf(after) : 'start'} ${kind}`, () => {
        row.insert(row.length, [emptyCell(kind)]);
      });
    }
  }
  return true;
}

/** Whether a table needs mending: a text or a failing cell in a row, no cell at all, or an unfilled grid. */
function needsMending(table: Y.XmlElement): boolean {
  if (holdsNoCell(table)) return true;
  const grid: GridNode = { content: [] };
  for (const row of table.toArray()) {
    if (!(row instanceof Y.XmlElement)) return true;
    const cells = row.toArray();
    if (cells.some((cell) => !isElement(cell, CELLS) || !cellStays(cell))) return true;
    grid.content?.push({
      content: (cells as Y.XmlElement[]).map((cell) => ({ attrs: cell.getAttributes() })),
    });
  }
  const fit = gridFit(grid);
  return (
    fit !== 'tooLarge' &&
    fit.some(
      (row) =>
        row.missing > 0 || row.cells.some((cell) => cell.columns.changed || cell.rows.changed),
    )
  );
}

type Parent = Y.XmlFragment | Y.XmlElement;

const TABLE = new Set(['table']);
const TOGGLE = new Set(['details']);
const HIDDEN = new Set(['detailsContent']);

/** The tables under `parent`, each with what holds it: at its top and in what its toggles hide. */
function tablesOf(parent: Parent): [Parent, Y.XmlElement][] {
  const found: [Parent, Y.XmlElement][] = [];
  for (const child of parent.toArray()) {
    if (isElement(child, TABLE)) found.push([parent, child]);
    else if (isElement(child, TOGGLE))
      for (const part of child.toArray())
        if (isElement(part, HIDDEN)) found.push(...tablesOf(part));
  }
  return found;
}

/** Deletes `child` from `parent`, and a toggle that then hides nothing, up to the one that hides something. */
function deleteFrom(doc: Y.Doc, parent: Parent, child: Y.XmlElement): void {
  let holder = parent;
  let gone: Y.XmlElement = child;
  for (;;) {
    const toggle =
      holder instanceof Y.XmlElement && isElement(holder, HIDDEN) ? holder.parent : null;
    if (!(toggle instanceof Y.XmlElement) || holder.length > 1 || !toggle.parent) break;
    gone = toggle as Y.XmlElement;
    holder = toggle.parent as Parent;
  }
  const at = holder.toArray().indexOf(gone);
  doc.transact(() => {
    holder.delete(at);
  });
}

/**
 * Mends every table of a room's document that two merged changes left
 * wrong; the update it wrote, if it wrote anything.
 */
export function mendTables(doc: Y.Doc): Uint8Array | undefined {
  const updates: Uint8Array[] = [];
  const keep = (update: Uint8Array): void => {
    updates.push(update);
  };
  doc.on('update', keep);
  try {
    for (const [parent, table] of tablesOf(doc.getXmlFragment(BODY_FIELD))) {
      if (!needsMending(table)) continue;
      if (!mendRows(doc, table)) continue;
      if (holdsNoCell(table)) {
        deleteFrom(doc, parent, table);
        continue;
      }
      mendGrid(doc, table);
    }
  } finally {
    doc.off('update', keep);
  }
  return updates.length > 0 ? Y.mergeUpdates(updates) : undefined;
}
