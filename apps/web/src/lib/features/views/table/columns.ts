/**
 * The columns of a table view: which properties, in what order, how wide.
 *
 * A view names its columns in `shownProperties` and their widths in
 * `columnWidths`; the title column is always first and has the identifier
 * `TITLE_COLUMN_ID` there. Everything here is arithmetic on those two
 * members. The numbers a person might want different are in
 * `TABLE_SETTINGS`, and only there.
 */
import {
  TITLE_COLUMN_ID,
  type BoardView,
  type PropertyTemplate,
  type SortOption,
} from '$lib/logic/wire.ts';

export const TABLE_SETTINGS = {
  /** Minimum column width in pixels. */
  minColumnWidth: 100,
  /** Nor wider than this. */
  maxColumnWidth: 800,
  /** A column the view gives no width. */
  defaultColumnWidth: 150,
  /** One press of an arrow key on the handle between two headings, in pixels. */
  keyStep: 16,
  /** One "Wider" or "Narrower" of a heading's menu, in pixels. */
  menuStep: 48,
} as const;

export interface TableColumn {
  id: string;
  template: PropertyTemplate;
  /** In pixels. */
  width: number;
}

/** A width a column may have: a whole number of pixels between the two limits. */
export function clampWidth(width: number, max: number = TABLE_SETTINGS.maxColumnWidth): number {
  const upper = Math.max(
    TABLE_SETTINGS.minColumnWidth,
    Math.min(max, TABLE_SETTINGS.maxColumnWidth),
  );
  return Math.round(Math.min(upper, Math.max(TABLE_SETTINGS.minColumnWidth, width)));
}

/** The width the view gives a column, or the default. */
export function storedWidth(widths: Readonly<Record<string, number>>, id: string): number {
  const stored = widths[id];
  return typeof stored === 'number' && stored > 0
    ? clampWidth(stored)
    : TABLE_SETTINGS.defaultColumnWidth;
}

/**
 * The width the reader chose for the title column, or undefined when they
 * never did: then the pane decides (the three steps are classes on the
 * table, not numbers here).
 */
export function storedTitleWidth(widths: Readonly<Record<string, number>>): number | undefined {
  const stored = widths[TITLE_COLUMN_ID];
  return typeof stored === 'number' && stored > 0 ? clampWidth(stored) : undefined;
}

/**
 * The property columns of a view, in its order. An identifier the board has
 * no property for (the property was deleted) gets no column.
 */
export function tableColumns(
  view: BoardView,
  templates: readonly PropertyTemplate[],
  /** Widths that are not stored yet: the column being dragged wider or narrower. */
  live: Readonly<Record<string, number>> = {},
): TableColumn[] {
  const seen = new Set<string>();
  const columns: TableColumn[] = [];
  for (const id of view.fields.shownProperties) {
    if (seen.has(id)) continue;
    seen.add(id);
    const template = templates.find((candidate) => candidate.id === id);
    if (!template) continue;
    columns.push({ id, template, width: live[id] ?? storedWidth(view.fields.columnWidths, id) });
  }
  return columns;
}

/**
 * The tracks of every row, as a value of `grid-template-columns`.
 *
 * The title column follows `--table-title` (the reader's width, or the
 * default of the pane) but never takes more than half of the pane: `cqw` is
 * a hundredth of the width of the container the table sits in. A last track
 * takes what is left, so that the lines of a table narrower than the pane
 * run to its edge.
 */
export function gridTemplate(widths: readonly number[]): string {
  const tracks = widths.map((width) => `${width}px`);
  return ['min(var(--table-title), 50cqw)', ...tracks, 'minmax(0, 1fr)'].join(' ');
}

/** `shownProperties` with the shown columns in a new order. Identifiers without a column keep the end. */
export function reorderedIds(
  visibleIds: readonly string[],
  shownOrder: readonly string[],
): string[] {
  const shown = new Set(shownOrder);
  return [...shownOrder, ...visibleIds.filter((id) => !shown.has(id))];
}

/**
 * `shownProperties` with one column moved a step to the left (-1) or to
 * the right (1) among the shown ones. Undefined when it is at that end.
 */
export function steppedIds(
  visibleIds: readonly string[],
  shownOrder: readonly string[],
  id: string,
  by: -1 | 1,
): string[] | undefined {
  const from = shownOrder.indexOf(id);
  const to = from + by;
  if (from === -1 || to < 0 || to >= shownOrder.length) return undefined;
  const next = [...shownOrder];
  next.splice(from, 1);
  next.splice(to, 0, id);
  return reorderedIds(visibleIds, next);
}

/** How the view is sorted by a column, when that column is its first sort. */
export function sortDirection(
  sort: readonly SortOption[],
  id: string,
): 'ascending' | 'descending' | undefined {
  const first = sort[0];
  if (first?.propertyId !== id) return undefined;
  return first.reversed ? 'descending' : 'ascending';
}
