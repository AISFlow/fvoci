/**
 * Builds the table's row lists with parent-child hierarchy grouping and
 * expand/collapse state, plus keyboard-drag preview ordering.
 */
import {
  hasVisibleHierarchy,
  hierarchicalRows,
  type HierarchicalRow,
} from '$lib/logic/card-hierarchy.ts';
import { moveItems, type ListOrder } from '$lib/logic/drop-target.ts';
import type { BoardGroup } from '$lib/logic/group.ts';
import type { Card } from '$lib/logic/wire.ts';

/** The rows between two headings: all of them, or those of one group. */
export interface RowList {
  /** Names the list for the row sorter. */
  id: string;
  group: BoardGroup | undefined;
  /** The cards drawn: none while the group is folded. */
  cards: readonly Card[];
  /** Hierarchical metadata matching `cards` one-for-one. */
  rows: readonly HierarchicalRow<Card>[];
  collapsed: boolean;
}

const ALL_ROWS = 'rows';

/** Turns either an ungrouped card array or grouped lists into hierarchical `RowList`s. */
export function buildTableRowLists(
  cards: readonly Card[],
  groups: readonly BoardGroup[] | undefined,
  collapsedGroups: readonly string[],
  collapsedParents: ReadonlySet<string>,
): RowList[] {
  if (!groups) {
    const rows = hierarchicalRows(cards, collapsedParents);
    return [
      {
        id: ALL_ROWS,
        group: undefined,
        cards: rows.map((row) => row.card),
        rows,
        collapsed: false,
      },
    ];
  }
  return groups.map((group) => {
    const collapsed = collapsedGroups.includes(group.option.id);
    const rows = collapsed ? [] : hierarchicalRows(group.cards, collapsedParents);
    return {
      id: `group:${group.option.id}`,
      group,
      cards: rows.map((row) => row.card),
      rows,
      collapsed,
    };
  });
}

/** Whether any list in `lists` shows parent-child nesting. */
export function listsHaveHierarchy(lists: readonly RowList[]): boolean {
  return lists.some((list) => hasVisibleHierarchy(list.rows));
}

/** Applies a keyboard row drag preview to `lists`, preserving each row's hierarchy metadata. */
export function previewMovedRowLists(
  lists: readonly RowList[],
  order: readonly ListOrder[],
  itemIds: readonly string[],
  gap: { listId: string; index: number },
): RowList[] {
  const moved = moveItems(order, itemIds, gap);
  const byId = new Map<string, HierarchicalRow<Card>>();
  for (const list of lists) {
    for (const row of list.rows) byId.set(row.card.id, row);
  }
  return lists.map((list, at) => {
    const rows = (moved[at]?.items ?? []).flatMap((id) => {
      const found = byId.get(id);
      return found ? [found] : [];
    });
    return { ...list, cards: rows.map((row) => row.card), rows };
  });
}
