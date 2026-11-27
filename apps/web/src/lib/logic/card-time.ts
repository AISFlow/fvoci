/**
 * When a card last changed. A card's `lastCommentAt` is when its newest
 * comment was posted, as the server says; 0 or absent, none or not known.
 */
import type { Card } from './wire.ts';

/** When a card last changed, a comment on it counting as a change: what "last updated time" shows. */
export function cardUpdatedAt(card: Card): number {
  return Math.max(card.updatedAt, card.lastCommentAt ?? 0);
}
