/**
 * The other end of a relation: the cards whose value of a relation property
 * names a card. A card's detail lists them under the property (as
 * ReverseRelations.svelte), so that a link made from one card can be seen,
 * and followed, from the card it names. Nothing is stored for it: it is read
 * from the cards of the board.
 */
import { relationEdges } from '$lib/logic/dependencies.ts';
import type { Card } from '$lib/logic/wire.ts';

/**
 * The cards among `cards` whose value of the relation `propertyId` names
 * `cardId`, in the order of `cards`. Templates are left out, and so is the
 * card itself.
 */
export function referrersOf<C extends Card>(
  cards: Iterable<C>,
  propertyId: string,
  cardId: string,
): C[] {
  const shown = [...cards].filter((card) => !card.fields.isTemplate);
  const byId = new Map(shown.map((card) => [card.id, card]));
  const from = new Set(
    relationEdges(shown, propertyId)
      .filter((edge) => edge.from === cardId)
      .map((edge) => edge.to),
  );
  return [...byId.values()].filter((card) => from.has(card.id));
}
