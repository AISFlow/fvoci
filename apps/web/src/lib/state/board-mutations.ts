/**
 * Undo-wrapped card, view, and board mutation helpers for `BoardStore`
 * (`board.svelte.ts`), extracted to keep `BoardStore` well within 500 lines.
 */
import type { BoardView, Card, Container } from '../logic/wire.ts';

import type { CardDraft, CardSummary, ViewDraft } from './backend.ts';
import { deleteCardForUndo, deleteViewForUndo, performBoardAction } from './board-sync.ts';
import type { BoardWrites, BlockWrites } from './board-writes.ts';
import type { BoardStore } from './board.svelte.ts';
import {
  cardPatches,
  containerPatches,
  isEmptyBlockPatch,
  isEmptyCardPatch,
  isEmptyContainerPatch,
  viewPatches,
} from './patches.ts';
import { nextCardNotices, nextMadeBy, takeBackNext, type MadeNext } from './next-cards.svelte.ts';
import { authorIds } from './user-names.ts';
import { reread, take } from './write.ts';

export async function changeBoardCard(
  store: BoardStore,
  cardWrites: BlockWrites<CardSummary, ReturnType<typeof cardPatches>[0]>,
  cardId: string,
  change: (card: CardSummary) => Card,
  description = 'changeCard',
  groupId?: string,
): Promise<void> {
  const current = store.card(cardId);
  if (!current) return;
  const [forward, backward] = cardPatches(change(current), current);
  if (isEmptyCardPatch(forward)) return;
  let said = false;
  /** The next card the last redo made (completing a card that repeats): its undo takes it back. */
  let made: MadeNext | undefined;
  const performing = performBoardAction(
    store,
    async () => {
      await cardWrites.write(cardId, forward, backward, said);
      made = nextMadeBy(current, store.card(cardId));
      said = true;
    },
    async () => {
      // Its next card goes first: the card repeats again only once it is gone.
      const next = made && (await takeBackNext(store, made.nextId)) ? made : undefined;
      const repeat = next ? { recurrence: next.recurrence } : {};
      await cardWrites.write(cardId, { ...backward, ...repeat }, forward, true);
    },
    description,
    groupId,
    cardWrites.keep(cardId, backward),
  );
  // The step is in place as soon as it is performed.
  const checkpoint = store.workspace.undo.currentCheckpoint;
  await performing;
  if (made) void nextCardNotices(store.workspace).say(store, [made], checkpoint);
}

export function createBoardCard(
  store: BoardStore,
  draft: CardDraft,
  groupId?: string,
): Promise<CardSummary | undefined> {
  let made = false;
  return performBoardAction(
    store,
    async () => {
      const card = await store.workspace.backend.createCard(
        store.boardId,
        made ? { ...draft, quiet: true } : draft,
      );
      made = true;
      void store.workspace.learnUsers(authorIds([card]));
      store.cards = take(store.cards, card);
      return card;
    },
    async (card) => {
      await deleteCardForUndo(store, card.id);
      store.cards = store.cards.filter((existing) => existing.id !== card.id);
    },
    'createCard',
    groupId,
  );
}

export async function duplicateBoardCard(
  store: BoardStore,
  cardId: string,
  asTemplate?: boolean,
): Promise<CardSummary | undefined> {
  const isUndoBusy = (): boolean => store.workspace.undo.busy;
  const wasBusy = isUndoBusy();
  const copy = await performBoardAction(
    store,
    async () => {
      const card = await store.workspace.backend.duplicateCard(store.boardId, cardId, asTemplate);
      void store.workspace.learnUsers(authorIds([card]));
      store.cards = take(store.cards, card);
      return card;
    },
    async (card) => {
      await deleteCardForUndo(store, card.id);
      store.cards = store.cards.filter((existing) => existing.id !== card.id);
    },
    'duplicateCard',
  );
  return !wasBusy && isUndoBusy() ? undefined : copy;
}

export async function changeBoardView(
  store: BoardStore,
  viewWrites: BlockWrites<BoardView, ReturnType<typeof viewPatches>[0]>,
  viewId: string,
  change: (view: BoardView) => BoardView,
  description = 'changeView',
  groupId?: string,
): Promise<void> {
  const current = store.view(viewId);
  if (!current) return;
  const [forward, backward] = viewPatches(change(current), current);
  if (isEmptyBlockPatch(forward)) return;
  await performBoardAction(
    store,
    () => viewWrites.write(viewId, forward, backward),
    () => viewWrites.write(viewId, backward, forward),
    description,
    groupId,
    viewWrites.keep(viewId, backward),
  );
}

export async function rereadBoardViews(
  store: BoardStore,
  viewWrites: BlockWrites<BoardView, ReturnType<typeof viewPatches>[0]>,
  isInactive: () => boolean,
  waitForId?: string,
): Promise<void> {
  if (waitForId !== undefined) await viewWrites.settled(waitForId);
  const before = store.views.map((view) => view.id);
  const views = await store.workspace.backend.views(store.boardId);
  if (isInactive()) return;
  store.views = reread(store.views, views, before, (view) => viewWrites.receive(view));
}

export function createBoardView(
  store: BoardStore,
  draft: ViewDraft,
): Promise<BoardView | undefined> {
  return performBoardAction(
    store,
    async () => {
      const view = await store.workspace.backend.createView(store.boardId, draft);
      store.views = take(store.views, view);
      return view;
    },
    async (view) => {
      await deleteViewForUndo(store, view.id);
      store.views = store.views.filter((existing) => existing.id !== view.id);
    },
    'createView',
  );
}

export async function deleteBoardView(store: BoardStore, viewId: string): Promise<void> {
  const gone = store.view(viewId);
  if (!gone) return;
  await performBoardAction(
    store,
    async () => {
      store.deleting(viewId);
      await store.workspace.backend.deleteView(store.boardId, viewId);
      store.views = store.views.filter((view) => view.id !== viewId);
    },
    async () => {
      await store.workspace.backend.restoreView(store.boardId, viewId);
      store.views = take(store.views, gone);
    },
    'deleteView',
  );
}

export async function changeBoardContainer(
  store: BoardStore,
  boardWrites: BoardWrites,
  change: (board: Container) => Container,
  description = 'changeBoard',
  groupId?: string,
): Promise<void> {
  if (!store.board) return;
  const [forward, backward] = containerPatches(change(store.board), store.board);
  if (isEmptyContainerPatch(forward)) return;
  await performBoardAction(
    store,
    () => boardWrites.write(forward, backward),
    () => boardWrites.write(backward, forward),
    description,
    groupId,
    boardWrites.keep(backward),
  );
}
