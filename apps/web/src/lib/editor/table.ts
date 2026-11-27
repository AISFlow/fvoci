/**
 * The table of a body: rows of cells and headers, a cell spanning columns
 * and rows. The document holds each cell's two spans and nothing else of its
 * look (`schema.ts`): no widths, no alignment.
 *
 * A table stands only in the document itself, not among the blocks of a
 * quote, a list entry or a cell, so that no table is in another: a cell
 * holds blocks, and so does a list entry in a cell. Its grid is filled when
 * it is stored (`table-grid.ts`).
 *
 * The table plugin pads a table that does not fill its grid after a change.
 * Here it does so after the reader's own change only, and only while no
 * table of the document was left unfilled by a merge of two people's
 * changes: that one the server mends (the memory rooms too), and every
 * editor that padded it as well would add cells of its own. A change of the
 * reader's that would take a table past 200 rows or 20 columns is not made.
 */
import { Document } from '@tiptap/extension-document';
import { Table, TableCell, TableHeader, TableRow } from '@tiptap/extension-table';
import type { Node as DocumentNode } from '@tiptap/pm/model';
import { Plugin, type EditorState, type Transaction } from '@tiptap/pm/state';
import { TableMap, tableEditingKey } from '@tiptap/pm/tables';

import { TABLE_CELL_MIN_WIDTH } from './drawing.ts';
import { fromShared } from './shared-place.ts';
import { isPastLimits, tablePaste } from './table-paste.ts';
import { allowedSpan, MAX_TABLE_COLUMNS, MAX_TABLE_ROWS } from './table-grid.ts';

/** The group of what stands only in the document itself. */
const DOCUMENT_ONLY = 'documentBlock';

/** The document: blocks, and tables. */
export const BodyDocument = Document.extend({ content: `(block | ${DOCUMENT_ONLY})+` });

/** The tables of a document, which stand only in it. */
function tablesOf(doc: DocumentNode): DocumentNode[] {
  const tables: DocumentNode[] = [];
  doc.forEach((node) => {
    if (node.type.spec.tableRole === 'table') tables.push(node);
  });
  return tables;
}

/** Whether a document holds a table that does not fill its grid. */
const holdsUnfilled = (doc: DocumentNode): boolean =>
  tablesOf(doc).some((table) => TableMap.get(table).problems !== null);

/** The table plugin, padding tables after the reader's own changes only (see above). */
function paddingOwnChanges(plugin: Plugin<unknown>): Plugin<unknown> {
  const append = plugin.spec.appendTransaction;
  if (!append) return plugin;
  return new Plugin({
    ...plugin.spec,
    appendTransaction(transactions, oldState, state) {
      // Handed the state after the change as the one before, the plugin sees no table changed.
      const left =
        oldState.doc !== state.doc &&
        (transactions.some(fromShared) || holdsUnfilled(oldState.doc));
      return append.call(this, transactions, left ? state : oldState, state);
    },
  });
}

/** No change of the reader's takes a table past the limits (one past them already is the server's to refuse). */
const withinLimits = (): Plugin =>
  new Plugin({
    filterTransaction(tr: Transaction, state: EditorState) {
      if (!tr.docChanged || fromShared(tr)) return true;
      const past = (doc: DocumentNode): boolean => tablesOf(doc).some(isPastLimits);
      return !past(tr.doc) || past(state.doc);
    },
  });

/** Columns keep the width the page gives them: none is stored, and none is dragged. */
export const BodyTable = Table.extend({
  group: DOCUMENT_ONLY,
  addProseMirrorPlugins() {
    const plugins = (this.parent?.() ?? []).map((plugin) =>
      plugin.spec.key === tableEditingKey ? paddingOwnChanges(plugin) : plugin,
    );
    return [...plugins, withinLimits(), tablePaste()];
  },
}).configure({
  resizable: false,
  cellMinWidth: TABLE_CELL_MIN_WIDTH,
});

/** A span read from pasted HTML by its leading digits, as a browser reads it; else one. */
function spanOf(element: HTMLElement, name: string, limit: number): number {
  return allowedSpan(Number.parseInt(element.getAttribute(name) ?? '1', 10), limit) ?? 1;
}

/** The spans of a cell, the only attributes it keeps. */
function spans() {
  return {
    colspan: {
      default: 1,
      parseHTML: (element: HTMLElement) => spanOf(element, 'colspan', MAX_TABLE_COLUMNS),
    },
    rowspan: {
      default: 1,
      parseHTML: (element: HTMLElement) => spanOf(element, 'rowspan', MAX_TABLE_ROWS),
    },
  };
}

export const BodyTableCell = TableCell.extend({ addAttributes: spans });
export const BodyTableHeader = TableHeader.extend({ addAttributes: spans });
export { TableRow as BodyTableRow };
