/**
 * What the kanban works out without a page: the names its columns go by
 * while dragging, where a drop lands, which cards a click selects, and what
 * a view's lists of columns become when one is hidden, shown or moved.
 */
import { moveItems, type ListOrder } from '$lib/logic/drop-target.ts';
import type { BoardGroup } from '$lib/logic/group.ts';

/** The one list of the sortable that moves columns: the row they stand in. */
export const ROW = 'board';

// The column of cards without a value has the empty identifier. A prefix
// keeps every name non-empty, and tells a column from a hidden one.
const COLUMN = 'c-';
const HIDDEN = 'h-';

/** The name of a visible column, as a list of cards and as an item of the row. */
export const columnIdOf = (optionId: string): string => COLUMN + optionId;
/** The name of a hidden column: a place to drop on. */
export const hiddenIdOf = (optionId: string): string => HIDDEN + optionId;
/** The option a column or a hidden column stands for. */
export const optionIdOf = (id: string): string => id.slice(COLUMN.length);

/** The visible columns as the lists of the sortable that moves cards. */
export function cardLists(groups: readonly BoardGroup[]): ListOrder[] {
  return groups.map((group) => ({
    id: columnIdOf(group.option.id),
    items: group.cards.map((card) => card.id),
  }));
}

/**
 * Where a sorted view would show a card in a column: after the cards of the
 * column that come before it in the view's order. `order` is every card of
 * the view in that order; the cards on their way (`carried`) are not counted.
 */
export function sortedIndex(
  order: readonly string[],
  column: readonly string[],
  cardId: string,
  carried: readonly string[],
): number {
  const position = new Map(order.map((id, at) => [id, at]));
  const own = position.get(cardId) ?? Number.POSITIVE_INFINITY;
  return column.filter(
    (id) => !carried.includes(id) && (position.get(id) ?? Number.POSITIVE_INFINITY) < own,
  ).length;
}

/**
 * The lists as a drop leaves them: the cards taken out of where they were
 * and put, in the order they had, at a place in a column. A column that is
 * not among the lists is a hidden one: the cards leave the board.
 */
export function listsAfterDrop(
  lists: readonly ListOrder[],
  cardIds: readonly string[],
  listId: string,
  index: number,
): ListOrder[] {
  if (lists.some((list) => list.id === listId)) return moveItems(lists, cardIds, { listId, index });
  return lists.map((list) => ({
    ...list,
    items: list.items.filter((id) => !cardIds.includes(id)),
  }));
}

/** What a press on a card asks for: the card opened, taken into the selection, or the range up to it. */
export type Activation = 'open' | 'toggle' | 'range';

/**
 * The "…" button of a card on a board or in a gallery: a plain element with
 * the ghost icon button's classes instead of a component wrapper per card.
 */
export const CARD_MENU_BUTTON =
  'inline-flex size-8 shrink-0 items-center justify-center rounded-lg border border-transparent text-muted-foreground transition-all outline-none select-none hover:bg-muted hover:text-foreground dark:hover:bg-muted/50 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 aria-expanded:bg-muted aria-expanded:text-foreground roomy:size-touch can-hover:opacity-0 can-hover:group-focus-within/card:opacity-100 can-hover:group-hover/card:opacity-100 can-hover:focus-visible:opacity-100 can-hover:aria-expanded:opacity-100';

/** A column a card can be moved to from its menu. */
export interface MoveTarget {
  optionId: string;
  name: string;
  /** The card is in it now. */
  current: boolean;
}

/** A selection with one card taken in, or taken out when it was in. */
export function toggled(selected: readonly string[], cardId: string): string[] {
  return selected.includes(cardId) ? selected.filter((id) => id !== cardId) : [...selected, cardId];
}

/**
 * A selection grown by a range: every card from `anchor` to `cardId` in the
 * order the board shows them (column by column, top to bottom). Without an
 * anchor on the board the card alone is added.
 */
export function withRange(
  order: readonly string[],
  selected: readonly string[],
  anchor: string | undefined,
  cardId: string,
): string[] {
  const from = anchor === undefined ? -1 : order.indexOf(anchor);
  const to = order.indexOf(cardId);
  if (to === -1) return [...selected];
  const range = from === -1 ? [cardId] : order.slice(Math.min(from, to), Math.max(from, to) + 1);
  return [...selected, ...range.filter((id) => !selected.includes(id))];
}

/** What a view says about its columns. */
export type ColumnLists = {
  visibleGroups: string[];
  hiddenGroups: string[];
};

/**
 * The lists after a column was hidden. `shown` is the visible columns as
 * the board draws them: a view need not list an option to show it, and from
 * now on it lists them all.
 */
export function withColumnHidden(
  shown: readonly string[],
  hidden: readonly string[],
  optionId: string,
): ColumnLists {
  return {
    visibleGroups: shown.filter((id) => id !== optionId),
    hiddenGroups: [...hidden.filter((id) => id !== optionId), optionId],
  };
}

/** The lists after a hidden column was shown again: it comes last. */
export function withColumnShown(
  shown: readonly string[],
  hidden: readonly string[],
  optionId: string,
): ColumnLists {
  return {
    visibleGroups: [...shown.filter((id) => id !== optionId), optionId],
    hiddenGroups: hidden.filter((id) => id !== optionId),
  };
}

/** The visible columns with one moved by `step` places; it stops at the ends. */
export function withColumnStepped(
  shown: readonly string[],
  optionId: string,
  step: number,
): string[] {
  const from = shown.indexOf(optionId);
  if (from === -1) return [...shown];
  const to = Math.max(0, Math.min(shown.length - 1, from + step));
  const rest = shown.filter((id) => id !== optionId);
  rest.splice(to, 0, optionId);
  return rest;
}

/** One thing in the row of columns as it is drawn: a column, or the gap a dragged one would land in. */
export type ColumnEntry = { kind: 'column'; group: BoardGroup; at: number } | { kind: 'gap' };

/**
 * The columns as drawn while one of them is dragged to `index` (counted
 * without it). A pointer leaves the column where it is, for the page to
 * hide, and a gap shows where it would land. The keyboard has no overlay to
 * show the column in, so there the column itself stands at that place.
 * `at` is the place a column has when nothing is dragged.
 */
export function drawnColumns(
  groups: readonly BoardGroup[],
  drag: { mode: 'pointer' | 'keyboard'; itemId: string; index: number } | undefined,
): ColumnEntry[] {
  const all = groups.map((group, at): ColumnEntry & { kind: 'column' } => ({
    kind: 'column',
    group,
    at,
  }));
  if (!drag) return all;
  const dragged = (entry: ColumnEntry & { kind: 'column' }): boolean =>
    columnIdOf(entry.group.option.id) === drag.itemId;
  const others = all.filter((entry) => !dragged(entry));
  const index = Math.max(0, Math.min(drag.index, others.length));
  if (drag.mode === 'keyboard') {
    return [...others.slice(0, index), ...all.filter(dragged), ...others.slice(index)];
  }
  const entries: ColumnEntry[] = [];
  let passed = 0;
  for (const entry of all) {
    if (!dragged(entry)) {
      if (passed === index) entries.push({ kind: 'gap' });
      passed += 1;
    }
    entries.push(entry);
  }
  if (passed === index) entries.push({ kind: 'gap' });
  return entries;
}

/**
 * Which of the columns a board that shows one at a time is resting on: the
 * one whose resting place is nearest to where the board is scrolled to.
 * `rests` is the scroll offset at which each column sits in the middle.
 */
export function nearestRest(rests: readonly number[], scrollLeft: number): number {
  let nearest = 0;
  let least = Number.POSITIVE_INFINITY;
  rests.forEach((rest, at) => {
    const distance = Math.abs(rest - scrollLeft);
    if (distance < least) {
      least = distance;
      nearest = at;
    }
  });
  return nearest;
}
