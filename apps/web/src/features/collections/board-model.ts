// Pure board rules for the grouped collection board: per-group page bodies,
// column card lists, drop targets and the move request. Source
// apps/web/src/features/collections/collection-panel.tsx (`move`) and
// collection-drag.tsx decide the same targets; paging is per group here.
import type {
  CollectionConfig,
  CollectionQueryBody,
  CollectionQueryItem,
} from "@/lib/queries/collections";

/** Items per column page (server default; its cap is 100). */
export const BOARD_PAGE_LIMIT = 50;

/** `dataTransfer` type carrying the dragged item id. */
export const BOARD_DRAG_TYPE = "application/x-fvoci-collection-item";

export type BoardGroup = {
  id: string | null;
  name: string;
  count: number;
  deleted: boolean;
};

export type BoardRow = Pick<
  CollectionQueryItem,
  "id" | "group" | "canEdit" | "taskId" | "statusId" | "values"
>;

/**
 * Body for one column page. `group: null` selects the unassigned column; the
 * key is always present so the server never falls back to every group.
 */
export function boardColumnBody(
  config: CollectionConfig,
  group: string | null,
  cursor?: string,
): CollectionQueryBody {
  return { config, group, limit: BOARD_PAGE_LIMIT, ...(cursor ? { cursor } : {}) };
}

/**
 * Cards of one column across its loaded pages: first occurrence wins and rows
 * whose group no longer matches (a stale page after a move) are dropped, so a
 * card never shows twice on the board.
 */
export function columnRows<T extends Pick<BoardRow, "id" | "group">>(
  pages: readonly { items: readonly T[] }[],
  groupId: string | null,
): T[] {
  const seen = new Set<string>();
  const rows: T[] = [];
  for (const page of pages) {
    for (const row of page.items) {
      if (row.group !== groupId || seen.has(row.id)) continue;
      seen.add(row.id);
      rows.push(row);
    }
  }
  return rows;
}

export type MoveRequest =
  | { kind: "status"; taskId: string; statusId: string; expectedStatusId: string }
  | { kind: "field"; fieldId: string; value: { options: [string] } | null };

/**
 * The write that moves `row` into `target`, or null when the move is not
 * allowed: read-only row, same group, archived (deleted) option, a status
 * board without a status, or an unknown group basis.
 */
export function moveRequest(
  groupBy: string | null,
  row: BoardRow,
  target: Pick<BoardGroup, "id" | "deleted">,
): MoveRequest | null {
  if (!groupBy || !row.canEdit || target.deleted || row.group === target.id) return null;
  if (groupBy === "status") {
    if (!target.id || !row.taskId || !row.statusId) return null;
    return {
      kind: "status",
      taskId: row.taskId,
      statusId: target.id,
      expectedStatusId: row.statusId,
    };
  }
  return { kind: "field", fieldId: groupBy, value: target.id ? { options: [target.id] } : null };
}

/** Groups offered by the per-card keyboard select, in board order. */
export function moveChoices(groupBy: string | null, groups: readonly BoardGroup[]) {
  return groups
    .filter((group) => groupBy !== "status" || group.id !== null)
    .map((group) => ({ id: group.id, name: group.name, disabled: group.deleted }));
}
