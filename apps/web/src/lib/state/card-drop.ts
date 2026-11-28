/**
 * Cards dropped on a view, executed as one atomic `moveViewCards` server
 * transaction and one step of undo: updates the view's `cardOrder` and (when
 * dropped into another column) the grouping property on all moved cards.
 * Moved into the column of done, a card that repeats makes its next card:
 * the undo takes it back and lets the card repeat again.
 */
import { manualComparator } from '../logic/sort.ts';
import type { BoardView, Card } from '../logic/wire.ts';

import type { CardMovesInput } from './backend.ts';
import { withCardProperty, type CardDrop } from './board-parts.ts';
import { performBoardAction } from './board-sync.ts';
import type { BoardStore } from './board.svelte.ts';
import { droppedCardOrder, placeAmongAll } from './card-order.ts';
import { nextCardNotices, nextMadeBy, takeBackNext, type MadeNext } from './next-cards.svelte.ts';
import { put, take } from './write.ts';

interface PlannedCardDrop {
  viewId: string;
  targetIds: string[];
  groupBy: string | undefined;
  optionId: string | undefined;
  nextCardOrder: string[];
  prevCardOrder: string[];
  prevValues: Map<string, string>;
  needsOrderChange: boolean;
  needsGroupChange: boolean;
  forwardMove: CardMovesInput;
}

function planCardDrop(board: BoardStore, view: BoardView, drop: CardDrop): PlannedCardDrop | null {
  const groupBy = board.groupingOf(view)?.id;
  const optionId = drop.optionId;
  const valueOf = (card: Card): string => {
    const value = groupBy ? card.fields.properties[groupBy] : undefined;
    return typeof value === 'string' ? value : '';
  };

  const shown = [...board.realCards].sort(manualComparator(view.fields.cardOrder));
  const movedIds = shown.filter((c) => drop.cardIds.includes(c.id)).map((c) => c.id);
  const targetIds = movedIds.length > 0 ? movedIds : [...drop.cardIds];

  let afterCardId: string | null | undefined;
  let nextCardOrder = view.fields.cardOrder;
  if (drop.index !== undefined) {
    const column = shown
      .filter((card) => !targetIds.includes(card.id))
      .filter((card) => optionId === undefined || valueOf(card) === optionId)
      .map((card) => card.id);
    const insertIndex = drop.shown
      ? placeAmongAll(
          column,
          drop.shown.filter((id) => !targetIds.includes(id)),
          drop.index,
        )
      : drop.index;
    nextCardOrder = droppedCardOrder(
      shown.map((card) => card.id),
      column,
      targetIds,
      insertIndex,
    );
    const firstMovedAt = nextCardOrder.indexOf(targetIds[0] ?? '');
    afterCardId = firstMovedAt <= 0 ? '' : (nextCardOrder[firstMovedAt - 1] ?? '');
  }

  const prevCardOrder = [...view.fields.cardOrder];
  const prevValues = new Map<string, string>();
  if (groupBy && optionId !== undefined) {
    for (const id of targetIds) {
      const card = board.card(id);
      if (card) prevValues.set(id, valueOf(card));
    }
  }

  const needsOrderChange =
    afterCardId !== undefined &&
    (nextCardOrder.length !== prevCardOrder.length ||
      nextCardOrder.some((id, idx) => prevCardOrder[idx] !== id));
  const needsGroupChange =
    Boolean(groupBy && optionId !== undefined) &&
    targetIds.some((id) => {
      const card = board.card(id);
      return card !== undefined && valueOf(card) !== optionId;
    });
  if (!needsOrderChange && !needsGroupChange) return null;

  const forwardMove: CardMovesInput = {
    cardIds: targetIds,
    ...(afterCardId === undefined ? {} : { afterCardId }),
    ...(needsGroupChange && groupBy !== undefined
      ? { groupPropertyId: groupBy, groupValue: optionId === '' ? null : (optionId ?? null) }
      : {}),
  };

  return {
    viewId: view.id,
    targetIds,
    groupBy,
    optionId,
    nextCardOrder,
    prevCardOrder,
    prevValues,
    needsOrderChange,
    needsGroupChange,
    forwardMove,
  };
}

function applyOptimisticCardDrop(board: BoardStore, plan: PlannedCardDrop): void {
  if (plan.needsOrderChange) {
    const curView = board.view(plan.viewId);
    if (curView) {
      board.views = put(board.views, {
        ...curView,
        fields: { ...curView.fields, cardOrder: plan.nextCardOrder },
      });
    }
  }
  if (plan.needsGroupChange && plan.groupBy) {
    const val = plan.optionId === '' ? undefined : plan.optionId;
    for (const id of plan.targetIds) {
      const cur = board.card(id);
      if (cur)
        board.cards = put(board.cards, { ...cur, ...withCardProperty(cur, plan.groupBy, val) });
    }
  }
}

/** Takes back a drop: the order, the values, and the next cards it made (by moved card). */
async function undoCardDrop(
  board: BoardStore,
  plan: PlannedCardDrop,
  made: ReadonlyMap<string, MadeNext>,
): Promise<void> {
  if (plan.needsOrderChange) {
    await board.writeView(
      plan.viewId,
      { updatedFields: { cardOrder: plan.prevCardOrder } },
      { updatedFields: { cardOrder: plan.nextCardOrder } },
    );
  }
  if (plan.needsGroupChange && plan.groupBy) {
    for (const [id, prev] of plan.prevValues) {
      // Its next card goes first: the card repeats again only once it is gone.
      const planned = made.get(id);
      const next = planned && (await takeBackNext(board, planned.nextId)) ? planned : undefined;
      const updated = await board.workspace.backend.patchCard(
        board.boardId,
        id,
        {
          ...(prev === ''
            ? { deletedProperties: [plan.groupBy] }
            : { updatedProperties: { [plan.groupBy]: prev } }),
          ...(next ? { recurrence: next.recurrence } : {}),
        },
        { quiet: true },
      );
      board.receiveCard(updated);
    }
  }
}

export async function dropCardsOn(board: BoardStore, drop: CardDrop): Promise<void> {
  const view = board.view(drop.viewId);
  if (!view || drop.cardIds.length === 0) return;
  const plan = planCardDrop(board, view, drop);
  if (!plan) return;

  let said = false;
  /** The next cards the last redo made, by the card moved into the column of done. */
  const made = new Map<string, MadeNext>();
  const performing = performBoardAction(
    board,
    async () => {
      const prevCards = board.cards;
      const prevViews = board.views;
      applyOptimisticCardDrop(board, plan);
      try {
        const res = await board.workspace.backend.moveViewCards(
          board.boardId,
          plan.viewId,
          plan.forwardMove,
          said ? { quiet: true } : undefined,
        );
        said = true;
        board.views = take(board.views, res.view);
        made.clear();
        for (const updated of res.cards) {
          const next = nextMadeBy(
            prevCards.find((card) => card.id === updated.id),
            updated,
          );
          if (next) made.set(updated.id, next);
          board.receiveCard(updated);
        }
      } catch (error) {
        board.views = prevViews;
        board.cards = prevCards;
        throw error;
      }
    },
    () => undoCardDrop(board, plan, made),
    'moveCard',
  );
  const checkpoint = board.workspace.undo.currentCheckpoint;
  await performing;
  if (made.size > 0)
    void nextCardNotices(board.workspace).say(board, [...made.values()], checkpoint);
}
