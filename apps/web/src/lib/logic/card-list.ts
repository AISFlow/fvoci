/** From the cards of a board to the cards a view shows, in its order. */
import { applyFilterGroup, type FilterContext } from './filter.ts';
import { searchFilterCards } from './search.ts';
import { sortCards, type SortContext } from './sort.ts';
import type { Card, PropertyTemplate, ViewQuery } from './wire.ts';

export type ViewCardContext = Pick<
  SortContext,
  'usernameOf' | 'locale' | 'timeZone' | 'cardTitle'
> &
  Pick<FilterContext, 'meId' | 'now'>;

/**
 * Stage 1: cards of a view filtered by `view.filter` and sorted by `view.sort`
 * / `view.cardOrder`, independent of transient `searchText` (§1.2.4).
 */
export function filteredCardsOfView(
  cards: readonly Card[],
  templates: readonly PropertyTemplate[],
  view: ViewQuery,
  context: ViewCardContext,
): Card[] {
  let result = cards.filter((card) => !card.limited);
  result = applyFilterGroup(view.filter, templates, result, context);
  return sortCards(result, {
    ...context,
    templates,
    sort: view.sort,
    cardOrder: view.cardOrder,
  });
}

/** Stage 2: narrows already-filtered and sorted cards by `searchText` (§1.2.4). */
export function narrowCardsBySearch(
  filtered: readonly Card[],
  templates: readonly PropertyTemplate[],
  searchText: string,
): Card[] {
  return searchText ? searchFilterCards(filtered, templates, searchText) : [...filtered];
}

/**
 * The cards of a view: those the server does not withhold, that pass the
 * view's filter and the search text, sorted the way the view asks.
 */
export function cardsOfView(
  cards: readonly Card[],
  templates: readonly PropertyTemplate[],
  view: ViewQuery,
  searchText: string,
  context: ViewCardContext,
): Card[] {
  const filtered = filteredCardsOfView(cards, templates, view, context);
  return searchText ? searchFilterCards(filtered, templates, searchText) : filtered;
}

/** How many cards of a board the server withholds. */
export function hiddenCardCount(cards: readonly Card[]): number {
  return cards.filter((card) => card.limited).length;
}
