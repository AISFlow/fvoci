/**
 * `batchPatchCards` of the memory backend, as the server answers it: the
 * same property values set and cleared on `1..=200` distinct cards of a
 * board, all or none. Every card is checked before any is changed: a card
 * of another board or a block that is no card is not there (404), a
 * template is refused (400), and so is a value a relation cannot hold. A
 * card that repeats and is made done makes its next card, told after it, and
 * every change is kept in the cards' activity, as a single patch does.
 */
import { CARD_BATCH_LIMIT, type CardPropertiesPatch } from '../authority-types.ts';
import type { CardSummary } from '../backend.ts';
import { applyCardPatch } from '../patches.ts';

import { blockInBoard, withLastComment } from './block-ops.ts';
import { noteCardChanged } from './card-activity.ts';
import { makeNextCard, settleCardChange } from './card-recurrence.ts';
import { containerInWorld, tellBlocks, type MemoryContainerContext } from './container-ops.ts';
import { checkRelations } from './references.ts';
import { fail } from './refuse.ts';
import { summaryOf } from './shapes.ts';
import { mayManageCards } from './trash.ts';
import type { StoredCard } from './world.ts';

export function batchPatchMemoryCards(
  ctx: MemoryContainerContext,
  boardId: string,
  cardIds: readonly string[],
  patch: CardPropertiesPatch,
): CardSummary[] {
  const board = containerInWorld(ctx.world, boardId, 'board');
  mayManageCards(ctx.world, ctx.userId, boardId, 'access denied to patch card');
  if (cardIds.length === 0 || cardIds.length > CARD_BATCH_LIMIT) {
    fail(400, `cardIds must name 1..=${CARD_BATCH_LIMIT} cards`, {
      field: 'cardIds',
      limit: CARD_BATCH_LIMIT,
    });
  }
  if (new Set(cardIds).size !== cardIds.length) {
    fail(400, 'duplicate cardIds', { field: 'cardIds' });
  }
  const values: CardPropertiesPatch = {
    updatedProperties: patch.updatedProperties ?? {},
    deletedProperties: patch.deletedProperties ?? [],
  };
  const settled = cardIds.map((cardId) => {
    const before = blockInBoard(ctx.world, boardId, cardId, 'card') as StoredCard;
    if (before.fields.isTemplate) {
      fail(400, 'a template is not changed with the cards', { field: 'cardIds' });
    }
    const card = applyCardPatch(before, values);
    checkRelations(ctx.world, board, card, before, 'updatedProperties');
    return { before, card, next: settleCardChange(ctx, board, before, card) };
  });
  const now = ctx.world.now();
  const made: StoredCard[] = [];
  for (const { before, card, next } of settled) {
    card.updatedAt = now;
    card.modifiedBy = ctx.userId;
    ctx.world.blocks.set(card.id, card);
    noteCardChanged(ctx.world, before, card, ctx.userId);
    if (next) made.push(makeNextCard(ctx, board, card, next));
  }
  const changed = settled.map(({ card }) => card);
  tellBlocks(ctx.world, boardId, [...changed, ...made]);
  return changed.map((card) => withLastComment(ctx.world, summaryOf(card)));
}
