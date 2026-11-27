/**
 * What every view of a board is given. The page that holds the view
 * (src/app/BoardPage.svelte) works out which cards it shows; the view draws
 * them and changes things through the board store.
 */
import type { Card, BoardView } from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

export interface ViewProps {
  board: BoardStore;
  view: BoardView;
  /** The cards the view shows: filtered, searched and sorted. */
  cards: readonly Card[];
  /** The address of a card, for a link to it. */
  cardHref: (cardId: string) => string;
  /** Opens a card over the view. */
  onOpenCard: (cardId: string) => void;
}
