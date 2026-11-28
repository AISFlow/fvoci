/**
 * What the slash menu offers for tables (slash-menu.ts), so that a table is
 * made and changed with the keyboard alone: a table on a line of the
 * document itself, and in a table a row added below and a column to the
 * right of the reader's cell, that row or column deleted, the first row
 * turned into headers or back, and the table deleted.
 *
 * A table stands only in the document itself, so it is not offered in a
 * list entry, a quote or a cell. What would take a table past 200 rows or 20
 * columns is not offered either, nor what cannot be done (deleting the only
 * row).
 */
import { findParentNode, type Editor } from '@tiptap/core';
import type { Node as DocumentNode } from '@tiptap/pm/model';
import { TableMap } from '@tiptap/pm/tables';

import { m } from '$lib/i18n/paraglide/messages.js';

import { takesDocumentBlocks } from './details.ts';
import type { SlashEntry } from './slash-menu.ts';
import { MAX_TABLE_COLUMNS, MAX_TABLE_ROWS } from './table-grid.ts';

/** How large a table is made: a row of headers over two rows, three columns. */
const MADE = { rows: 3, cols: 3, withHeaderRow: true };

const tableAround = findParentNode((node) => node.type.spec.tableRole === 'table');

/** Whether every cell of a table's first row is a header. */
function headed(table: DocumentNode): boolean {
  let all = table.firstChild !== null;
  table.firstChild?.forEach((cell) => {
    if (cell.type.name !== 'tableHeader') all = false;
  });
  return all;
}

/** The entries for tables where the reader is now. */
export function tableEntries(editor: Editor): SlashEntry[] {
  const { selection } = editor.state;
  const around = tableAround(selection);
  if (!around) {
    // A table stands only where a toggle does: on a line of the document or of a toggle's content.
    const { $from } = selection;
    if ($from.depth < 1 || !takesDocumentBlocks($from.node($from.depth - 1))) return [];
    return [
      {
        value: 'table',
        label: m.editor_slash_table(),
        words: ['table', 'grid', '표', '테이블'],
        run: (current) => current.chain().focus().insertTable(MADE).run(),
      },
    ];
  }
  const map = TableMap.get(around.node);
  const can = editor.can();
  const entries: (SlashEntry | false)[] = [
    map.height < MAX_TABLE_ROWS &&
      can.addRowAfter() && {
        value: 'table-row-add',
        label: m.editor_table_row_add(),
        words: ['row', 'add', '행', '줄', '넣기'],
        run: (current) => current.chain().focus().addRowAfter().run(),
      },
    map.width < MAX_TABLE_COLUMNS &&
      can.addColumnAfter() && {
        value: 'table-column-add',
        label: m.editor_table_column_add(),
        words: ['column', 'add', '열', '칸', '넣기'],
        run: (current) => current.chain().focus().addColumnAfter().run(),
      },
    can.deleteRow() && {
      value: 'table-row-delete',
      label: m.editor_table_row_delete(),
      words: ['row', 'delete', 'remove', '행', '줄', '지우기', '삭제'],
      run: (current) => current.chain().focus().deleteRow().run(),
    },
    can.deleteColumn() && {
      value: 'table-column-delete',
      label: m.editor_table_column_delete(),
      words: ['column', 'delete', 'remove', '열', '칸', '지우기', '삭제'],
      run: (current) => current.chain().focus().deleteColumn().run(),
    },
    // The first row's cells are made again as the other kind: two readers who
    // turn it at once leave the cells of both (e2e-live/body-tables.spec.ts), and
    // what a third writes in it meanwhile goes with the cells it was in. Headers
    // as a mark of the row, not a kind of cell, would need body version 6.
    can.toggleHeaderRow() && {
      value: 'table-header-row',
      label: headed(around.node) ? m.editor_table_header_off() : m.editor_table_header_on(),
      words: ['header', 'heading', 'row', '머리', '제목', '행'],
      run: (current) => current.chain().focus().toggleHeaderRow().run(),
    },
    {
      value: 'table-delete',
      label: m.editor_table_delete(),
      words: ['table', 'delete', 'remove', '표', '지우기', '삭제'],
      run: (current) => current.chain().focus().deleteTable().run(),
    },
  ];
  return entries.filter((entry) => entry !== false);
}
