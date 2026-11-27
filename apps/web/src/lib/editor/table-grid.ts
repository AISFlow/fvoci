/**
 * Where the cells of a table fall: its rows one under another, each cell in
 * the first column of its row that no cell above still covers, as the editor
 * lays a table out (`prosemirror-tables`). The server's rule is the same
 * (`crates/model/src/table_grid.rs`), and the shared vectors hold the two
 * together.
 *
 * A stored table fills its grid: no cell spans past the last row or over
 * another cell, and every row is as wide as the widest.
 */

/** The most rows a table has. */
export const MAX_TABLE_ROWS = 200;
/** The most columns a table has, as its cells' spans add up across a row. */
export const MAX_TABLE_COLUMNS = 20;

export interface GridNode {
  attrs?: Record<string, unknown>;
  content?: GridNode[];
}

/** What is wrong with the grid of a table, if anything. */
export type GridProblem = 'tooLarge' | 'noCell' | 'unfilled';

/** How far a cell reaches one way: as it fits, and whether that is other than it states. */
export interface Reach {
  span: number;
  changed: boolean;
}

/** Where a row's cells fit, in order, and how many empty cells its end lacks. */
export interface RowFit {
  cells: { columns: Reach; rows: Reach }[];
  missing: number;
}

/** A span that is allowed: a whole number from one up to the limit; undefined if not. */
export function allowedSpan(value: unknown, limit: number): number | undefined {
  return typeof value === 'number' && Number.isInteger(value) && value >= 1 && value <= limit
    ? value
    : undefined;
}

/** A span a cell states: one if it has none, undefined if it is not allowed. */
function statedSpan(cell: GridNode, name: string, limit: number): number | undefined {
  const value = cell.attrs?.[name];
  return value === undefined ? 1 : allowedSpan(value, limit);
}

const contentOf = (node: GridNode): GridNode[] => (Array.isArray(node.content) ? node.content : []);

/**
 * Where every cell of a table fits, row by row: its spans narrowed to what
 * fits, and the cells each row lacks to be as wide as the widest; 'tooLarge'
 * past a limit, which no narrowing brings under it. What a child is, is not
 * asked.
 */
export function gridFit(table: GridNode): RowFit[] | 'tooLarge' {
  const rows = contentOf(table);
  if (rows.length > MAX_TABLE_ROWS) return 'tooLarge';
  const covered: boolean[][] = rows.map(() => []);
  const isCovered = (row: number, column: number): boolean => covered[row]?.[column] === true;
  const fits: RowFit['cells'][] = [];
  for (const [row, node] of rows.entries()) {
    let column = 0;
    const cells: RowFit['cells'] = [];
    for (const cell of contentOf(node)) {
      while (isCovered(row, column)) column += 1;
      const statedColumns = statedSpan(cell, 'colspan', MAX_TABLE_COLUMNS);
      const statedRows = statedSpan(cell, 'rowspan', MAX_TABLE_ROWS);
      let columns = 0;
      while (columns < (statedColumns ?? 1) && !isCovered(row, column + columns)) columns += 1;
      // No cell above covers these columns in this row, so none covers them below.
      const down = Math.min(statedRows ?? 1, rows.length - row);
      if (column + columns > MAX_TABLE_COLUMNS) return 'tooLarge';
      for (const line of covered.slice(row, row + down)) {
        for (let at = column; at < column + columns; at += 1) line[at] = true;
      }
      cells.push({
        columns: { span: columns, changed: columns !== statedColumns },
        rows: { span: down, changed: down !== statedRows },
      });
      column += columns;
    }
    fits.push(cells);
  }
  const width = Math.max(0, ...covered.map((line) => line.length));
  // Counted, not walked: a row's list has holes where nothing covers it.
  return fits.map((cells, row) => ({
    cells,
    missing: width - (covered[row] ?? []).filter((at) => at).length,
  }));
}

/**
 * What is wrong with the grid of a table: past a limit, no cell at all, or a
 * span that does not fit as stated or a row narrower than the table; nothing
 * when it fills its grid.
 */
export function gridProblem(table: GridNode): GridProblem | undefined {
  const fit = gridFit(table);
  if (fit === 'tooLarge') return 'tooLarge';
  if (fit.every((row) => row.cells.length === 0)) return 'noCell';
  const filled = fit.every(
    (row) =>
      row.missing === 0 && row.cells.every((cell) => !cell.columns.changed && !cell.rows.changed),
  );
  return filled ? undefined : 'unfilled';
}
