/**
 * A table of a body drawn for reading (render.ts), as the editor's view of
 * one draws it: in a frame, with a column for each that its first row spans,
 * each column at least so wide and the table at least as wide as they are
 * together. Only rows and cells, and spans within the limits, are drawn,
 * whatever was handed in; what a cell holds is drawn by the caller.
 */
import { TABLE_CELL_MIN_WIDTH } from './drawing.ts';
import type { DocNode } from './schema.ts';
import { allowedSpan, MAX_TABLE_COLUMNS, MAX_TABLE_ROWS } from './table-grid.ts';

const SPANS = [
  ['colspan', MAX_TABLE_COLUMNS],
  ['rowspan', MAX_TABLE_ROWS],
] as const;

const spanOf = (cell: DocNode, name: string, limit: number): number =>
  allowedSpan(cell.attrs?.[name], limit) ?? 1;

/** The table's frame, or nothing for a table without rows. */
export function drawTable(
  page: Document,
  table: DocNode,
  childrenOf: (node: DocNode) => DocNode[],
  drawCell: (tag: 'th' | 'td', cell: DocNode) => HTMLElement,
): HTMLElement | undefined {
  const cellsOf = (row: DocNode): DocNode[] =>
    childrenOf(row).filter((cell) => cell.type === 'tableCell' || cell.type === 'tableHeader');
  const rows = childrenOf(table).filter((row) => row.type === 'tableRow');
  if (rows.length === 0) return undefined;
  const first = rows[0] ? cellsOf(rows[0]) : [];
  const width = first.reduce((sum, cell) => sum + spanOf(cell, 'colspan', MAX_TABLE_COLUMNS), 0);
  const columns = page.createElement('colgroup');
  for (let column = 0; column < width; column += 1) {
    const col = page.createElement('col');
    col.style.setProperty('min-width', `${TABLE_CELL_MIN_WIDTH}px`);
    columns.append(col);
  }
  const body = page.createElement('tbody');
  for (const row of rows) {
    const line = page.createElement('tr');
    for (const cell of cellsOf(row)) {
      const drawn = drawCell(cell.type === 'tableHeader' ? 'th' : 'td', cell);
      for (const [name, limit] of SPANS) {
        const span = spanOf(cell, name, limit);
        if (span !== 1) drawn.setAttribute(name, String(span));
      }
      line.append(drawn);
    }
    body.append(line);
  }
  const drawnTable = page.createElement('table');
  drawnTable.style.setProperty('min-width', `${width * TABLE_CELL_MIN_WIDTH}px`);
  drawnTable.append(columns, body);
  const frame = page.createElement('div');
  frame.setAttribute('class', 'tableWrapper');
  frame.append(drawnTable);
  return frame;
}
