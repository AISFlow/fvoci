/**
 * The search with a card's number: `#12` (or `#00012`) finds matching `#12`
 * cards across all readable boards of the workspace (`ContentSearch.find`),
 * and where a board is shown (also for bare `12`), puts that board's card
 * numbered 12 first (`References.numbered`) without listing it twice.
 * Elsewhere, or for another text, what `ContentSearch.find` answers alone.
 */
import { cardNumberOf, hashCardNumberOf } from '$lib/logic/card-numbers.ts';
import type { SearchAsk, SearchHit } from '$lib/state/content-search.ts';
import type { LinkedBlock } from '$lib/state/references.ts';

export interface NumberedSearch {
  /** The search of the words and `#N` card numbers (`ContentSearch.find`). */
  find: (ask: SearchAsk) => Promise<SearchHit[]>;
  /** The card of the board with the number, if the reader may see it. */
  numbered: (boardId: string, number: number) => Promise<LinkedBlock | undefined>;
  /** The board shown, as the search is asked; none on any other page. */
  boardShown: () => string | undefined;
}

/** What the search finds, with the card of the board shown that the words number placed first. */
export async function findNumbered(search: NumberedSearch, ask: SearchAsk): Promise<SearchHit[]> {
  const number = hashCardNumberOf(ask.query) ?? cardNumberOf(ask.query);
  const boardId = search.boardShown();
  if (
    number === undefined ||
    boardId === undefined ||
    (ask.kind !== undefined && ask.kind !== 'card')
  ) {
    return search.find(ask);
  }
  const [hits, card] = await Promise.all([
    search.find(ask),
    search.numbered(boardId, number).catch(() => undefined),
  ]);
  if (!card) {
    const isCurrentNumbered = (hit: SearchHit): boolean =>
      hit.containerId === boardId && hit.kind === 'card' && hit.number === number;
    const onBoard = hits.filter(isCurrentNumbered);
    if (onBoard.length === 0) return hits;
    const elsewhere = hits.filter((hit) => !isCurrentNumbered(hit));
    return [...onBoard, ...elsewhere];
  }
  const existing = hits.find((hit) => hit.id === card.id);
  const first: SearchHit = existing
    ? { ...existing, number }
    : {
        id: card.id,
        kind: 'card',
        containerId: card.containerId,
        containerKind: 'board',
        title: card.title,
        number,
        snippet: [],
      };
  const rest = hits.filter((hit) => hit.id !== card.id);
  return [first, ...rest].slice(0, ask.limit ?? rest.length + 1);
}
