/**
 * Live change application, background re-reading, and undo-wrapped mutations
 * for `BoardStore` (`board.svelte.ts`), extracted to keep `BoardStore` concise.
 */
import { descendantsOf } from '../logic/card-hierarchy.ts';
import type { BlockPatch, BoardView, Container } from '../logic/wire.ts';

import {
  isBackendError,
  isCardBlock,
  isViewBlock,
  type CardPatch,
  type CardSummary,
  type Change,
} from './backend.ts';
import { handleBoardRefreshFailure, readBoard } from './board-load.ts';
import type { BoardWrites, BlockWrites } from './board-writes.ts';
import type { BoardStore } from './board.svelte.ts';
import type { lastCommentsOf } from './last-comments.ts';
import type { MemberWrites } from './member-writes.ts';
import { authorIds, memberId } from './user-names.ts';
import { reread, take } from './write.ts';

export interface BoardSyncInternals {
  readonly store: BoardStore;
  readonly lastComments: ReturnType<typeof lastCommentsOf>;
  readonly boardWrites: BoardWrites;
  readonly cardWrites: BlockWrites<CardSummary, CardPatch>;
  readonly viewWrites: BlockWrites<BoardView, BlockPatch>;
  readonly people: MemberWrites;
  isInactive(): boolean;
  nextRefresh(): number;
  currentRefresh(): number;
  evictionGen(): number;
  takeBoard(board: Container): void;
  receiveCard(card: CardSummary): void;
  receiveView(view: BoardView): void;
  refreshPermissions(): Promise<void>;
  refresh(): Promise<void>;
}

export function applyBoardChange(internals: BoardSyncInternals, change: Change): void {
  const { store, lastComments, boardWrites, people } = internals;
  switch (change.kind) {
    case 'container':
      if (store.missing) {
        void store.load();
        break;
      }
      if (boardWrites.receive(change.container)) internals.takeBoard(change.container);
      break;
    case 'containerDeleted':
      store.workspace.evictContainer(store.boardId);
      if (store.board?.visibility === 'open' && !store.board.isTemplate && !store.isClosed()) {
        void store.load();
        break;
      }
      store.missing = true;
      store.status = 'failed';
      break;
    case 'members':
      people.told += 1;
      store.members = change.members;
      if (
        store.workspace.me &&
        !change.members.some((member) => member.userId === store.workspace.me?.id)
      ) {
        store.workspace.evictContainer(store.boardId);
      }
      void store.workspace.learnUsers(change.members.map(memberId), { members: true });
      void internals.refreshPermissions();
      break;
    case 'workspaceRole':
      if (change.userId === store.workspace.me?.id) {
        void internals.refreshPermissions();
      }
      break;
    case 'blocks':
      for (const block of change.blocks) {
        if (isCardBlock(block)) internals.receiveCard(block);
        else if (isViewBlock(block)) internals.receiveView(block);
        else {
          if (block.type === 'comment' && block.deletedAt === undefined) {
            lastComments.heard(block);
          }
          store.openCard?.receive(block);
        }
      }
      break;
    case 'blocksDeleted': {
      const gone = new Set(change.blockIds);
      for (const id of change.blockIds) {
        for (const child of descendantsOf(id, store.hierarchy.childrenMap)) gone.add(child.id);
      }
      const allGone = [...gone];
      lastComments.deleted(allGone);
      store.cards = store.cards.filter((card) => !gone.has(card.id));
      store.views = store.views.filter((view) => !gone.has(view.id));
      store.openCard?.forget(allGone);
      break;
    }
    case 'resync':
      void internals.refresh();
      break;
    default:
      break;
  }
}

export async function refreshBoard(internals: BoardSyncInternals): Promise<void> {
  const { store, lastComments, boardWrites, cardWrites, viewWrites, people } = internals;
  if (store.status !== 'ready' || internals.isInactive()) return;
  const reading = internals.nextRefresh();
  const heard = lastComments.begin();
  const evictionGen = internals.evictionGen();
  const cardsBefore = store.cards.map((card) => card.id);
  const viewsBefore = store.views.map((view) => view.id);
  const membersTold = people.told;
  try {
    const [board, views, cards, members, permissions] = await readBoard(
      store.workspace,
      store.boardId,
    );
    if (reading !== internals.currentRefresh() || internals.isInactive()) return;
    if (boardWrites.receive(board)) internals.takeBoard(board);
    [store.permissions, store.permissionsRole] = [permissions, store.role];
    store.views = reread(store.views, views, viewsBefore, (view) => viewWrites.receive(view));
    const shown = store.cards;
    const read = cards.map((card) => lastComments.settle(card, heard));
    store.cards = reread(shown, read, cardsBefore, (card) => cardWrites.receive(card));
    if (membersTold === people.told) {
      store.members = members;
      void store.workspace.learnUsers(members.map(memberId), { members: true });
    }
    if (internals.evictionGen() === evictionGen) store.evicted = false;
    const open = store.openCard;
    if (open) {
      const card = store.card(open.cardId);
      if (card) open.cardChanged(card);
      else if (cardsBefore.includes(open.cardId)) open.forget([open.cardId]);
      void open.refresh();
    }
  } catch (error) {
    await handleBoardRefreshFailure(
      store,
      error,
      () => reading === internals.currentRefresh() && !internals.isInactive(),
    );
  }
}

export function receiveBoardCard(
  store: BoardStore,
  lastComments: ReturnType<typeof lastCommentsOf>,
  cardWrites: { receive(card: CardSummary): boolean },
  incoming: CardSummary,
): void {
  const card = lastComments.merge(incoming, store.card(incoming.id));
  if (!cardWrites.receive(card)) return;
  void store.workspace.learnUsers(authorIds([card]));
  store.cards = take(store.cards, card);
  store.openCard?.cardChanged(card);
}

export async function performBoardAction<T>(
  store: BoardStore,
  redo: () => Promise<T>,
  undo: (value: T) => Promise<void>,
  description: string,
  groupId?: string,
  ready?: () => Promise<void> | undefined,
): Promise<T | undefined> {
  try {
    return await store.workspace.undo.perform(
      () => store.writeJoined(redo),
      undo,
      description,
      groupId,
      false,
      ready,
    );
  } catch (error) {
    if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
      store.workspace.evictContainer(store.boardId);
    }
    store.workspace.report('save', error);
    return undefined;
  }
}

export async function deleteBlockForUndo(store: BoardStore, blockId: string): Promise<void> {
  store.deleting(blockId);
  try {
    await store.workspace.backend.deleteBlock(store.boardId, blockId);
  } catch (error) {
    if (!isBackendError(error) || error.status !== 404) throw error;
  }
}

export async function deleteBoardCards(
  store: BoardStore,
  cardIds: readonly string[],
): Promise<void> {
  const uniqueIds = [...new Set(cardIds)];
  if (uniqueIds.length === 0) return;
  await performBoardAction(
    store,
    async () => {
      const gone = new Set<string>();
      for (const cardId of uniqueIds) {
        gone.add(cardId);
        for (const child of descendantsOf(cardId, store.hierarchy.childrenMap)) gone.add(child.id);
      }
      for (const id of gone) store.deleting(id);
      await store.workspace.backend.batchDeleteCards(store.boardId, uniqueIds);
      store.cards = store.cards.filter((card) => !gone.has(card.id));
      if (store.openCard && gone.has(store.openCard.cardId)) store.closeCard();
      return uniqueIds;
    },
    async (deletedIds) => {
      const restored = await store.workspace.backend.batchRestoreCards(store.boardId, deletedIds);
      let next = store.cards;
      for (const card of restored) next = take(next, card);
      store.cards = next;
    },
    'deleteCard',
  );
}
