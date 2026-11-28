/**
 * Server-authority operations for `MemoryBackend` (`state/memory/backend.ts`):
 * fractional-indexed view writes (`writeMemoryView`, `moveMemoryView`,
 * `duplicateMemoryView`), atomic `duplicateMemoryCard`, `moveMemoryViewCards`,
 * `batchDeleteMemoryCards` / `batchRestoreMemoryCards`, formal card comment
 * delete/restore, and the trash of views and attachments.
 */
import { valuesOfCopy } from '../../logic/dependencies.ts';
import { isPlace, movedTo, placeAtEnd } from '../../logic/list-places.ts';
import { manualComparator } from '../../logic/sort.ts';
import type { BoardView, Container } from '../../logic/wire.ts';
import type {
  BoardViewWriteResult,
  CardMovesInput,
  CardMovesOutput,
  CardSummary,
  Comment,
  CommentVersion,
  ViewDraft,
} from '../backend.ts';
import { droppedCardOrder } from '../card-order.ts';
import { orderViews, placesOf, viewCopy } from '../order.ts';
import { applyCardPatch } from '../patches.ts';

import { blockInBoard, createMemoryCard, withLastComment } from './block-ops.ts';
import { flushCard } from './bodies.ts';
import { noteCardChanged, noteCardsRestored } from './card-activity.ts';
import { makeNextCard, settleCardChange } from './card-recurrence.ts';
import { mayRemoveComments, roleOn, versionsOf } from './comments.ts';
import { memberRole } from './container-access.ts';
import {
  containerInWorld,
  newStoredBlock,
  tellBlocks,
  type MemoryContainerContext,
} from './container-ops.ts';
import { fail } from './refuse.ts';
import { readerLanguage } from './seed-templates.ts';
import { commentOf, copy, summaryOf, viewOf } from './shapes.ts';
import { deletion, mayManageCards, subtreeOf, takeBlockBack } from './trash.ts';
import type { StoredBlock, StoredCard, StoredView } from './world.ts';

export { memoryAccountToday, memoryWorkspaceToday } from './today-authority.ts';

function requireBoardEditor(ctx: MemoryContainerContext, boardId: string): Container {
  const board = containerInWorld(ctx.world, boardId, 'board');
  mayManageCards(ctx.world, ctx.userId, boardId, 'access denied to make board changes');
  return board;
}

function orderedStoredViews(ctx: MemoryContainerContext, boardId: string): BoardView[] {
  const views = ctx.world
    .blocksOf(boardId)
    .filter((block) => block.type === 'view')
    .map(viewOf);
  return orderViews(views, readerLanguage(ctx.world, ctx.userId));
}

function applyCompanionPlaces(
  ctx: MemoryContainerContext,
  keep: readonly { id: string; place: string }[],
): StoredView[] {
  const now = ctx.world.now();
  const companions: StoredView[] = [];
  for (const write of keep) {
    const stored = ctx.world.blocks.get(write.id) as StoredView | undefined;
    if (stored?.type !== 'view') continue;
    if (stored.fields.order === write.place) continue;
    stored.fields = { ...stored.fields, order: write.place };
    stored.updatedAt = now;
    stored.modifiedBy = ctx.userId;
    companions.push(stored);
  }
  return companions;
}

export function writeMemoryView(
  ctx: MemoryContainerContext,
  boardId: string,
  draft: ViewDraft,
): BoardViewWriteResult {
  requireBoardEditor(ctx, boardId);
  const shown = orderedStoredViews(ctx, boardId);
  let order = draft.fields.order;
  let companions: StoredView[] = [];
  if (!isPlace(order) || draft.after !== undefined) {
    if (draft.after === '') {
      const placed = placesOf(shown);
      const tempId = '__new__';
      const moved = movedTo(
        [...placed, { id: tempId, place: placeAtEnd(placed).place }],
        tempId,
        0,
      );
      order = moved.place;
      companions = applyCompanionPlaces(ctx, moved.keep);
    } else if (draft.after) {
      const idx = shown.findIndex((v) => v.id === draft.after);
      if (idx === -1) fail(400, 'no such after view', { field: 'after' });
      const placed = placesOf(shown);
      const tempId = '__new__';
      const moved = movedTo(
        [...placed, { id: tempId, place: placeAtEnd(placed).place }],
        tempId,
        idx + 1,
      );
      order = moved.place;
      companions = applyCompanionPlaces(ctx, moved.keep);
    } else {
      const atEnd = placeAtEnd(placesOf(shown));
      order = atEnd.place;
      companions = applyCompanionPlaces(ctx, atEnd.keep);
    }
  }
  const block = newStoredBlock(ctx, boardId, '', 'view');
  block.title = draft.title;
  block.fields = { ...copy(draft.fields), order };
  ctx.world.blocks.set(block.id, block);
  tellBlocks(ctx.world, boardId, [...companions, block]);
  return { view: viewOf(block), companionViews: companions.map(viewOf) };
}

export function moveMemoryView(
  ctx: MemoryContainerContext,
  boardId: string,
  viewId: string,
  after: string | null,
): BoardViewWriteResult {
  requireBoardEditor(ctx, boardId);
  const target = blockInBoard(ctx.world, boardId, viewId, 'view') as StoredView;
  const shown = orderedStoredViews(ctx, boardId);
  const without = shown.filter((v) => v.id !== viewId);
  let toIndex = 0;
  if (after !== null && after !== '') {
    const afterIdx = without.findIndex((v) => v.id === after);
    if (afterIdx === -1) fail(400, 'no such after view', { field: 'after' });
    toIndex = afterIdx + 1;
  }
  const placed = placesOf(shown);
  const { place, keep } = movedTo(placed, viewId, toIndex);
  const companions = applyCompanionPlaces(ctx, keep);
  target.fields = { ...target.fields, order: place };
  target.updatedAt = ctx.world.now();
  target.modifiedBy = ctx.userId;
  tellBlocks(ctx.world, boardId, [...companions, target]);
  return { view: viewOf(target), companionViews: companions.map(viewOf) };
}

export function duplicateMemoryView(
  ctx: MemoryContainerContext,
  boardId: string,
  viewId: string,
  title?: string,
  after?: string | null,
): BoardViewWriteResult {
  requireBoardEditor(ctx, boardId);
  const sourceBlock = blockInBoard(ctx.world, boardId, viewId, 'view');
  const source = viewOf(sourceBlock);
  const shown = orderedStoredViews(ctx, boardId);
  const copyTitle = title ?? `${source.title || 'View'} copy`;
  if (after !== undefined && after !== source.id) {
    return writeMemoryView(ctx, boardId, { title: copyTitle, fields: copy(source.fields), after });
  }
  const { fields, keep } = viewCopy(source, shown);
  const companions = applyCompanionPlaces(ctx, keep);
  const block = newStoredBlock(ctx, boardId, '', 'view');
  block.title = copyTitle;
  block.fields = copy(fields);
  ctx.world.blocks.set(block.id, block);
  tellBlocks(ctx.world, boardId, [...companions, block]);
  return { view: viewOf(block), companionViews: companions.map(viewOf) };
}

export function duplicateMemoryCard(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
  asTemplate?: boolean,
): CardSummary {
  const board = requireBoardEditor(ctx, boardId);
  const source = flushCard(
    ctx.world,
    blockInBoard(ctx.world, boardId, cardId, 'card') as StoredCard,
  );
  const onBoard = new Set<string>();
  for (const block of ctx.world.blocksOf(boardId)) {
    if (block.type === 'card') onBoard.add(block.id);
  }
  for (const deleted of ctx.world.deletedBlocks.values()) {
    if (deleted.containerId !== boardId) continue;
    for (const block of deleted.blocks) {
      if (block.type === 'card') onBoard.add(block.id);
    }
  }
  const created = createMemoryCard(ctx, boardId, {
    title: source.title,
    icon: source.fields.icon,
    properties: valuesOfCopy(summaryOf(source), board.cardProperties, onBoard),
    isTemplate: asTemplate ?? source.fields.isTemplate,
    body: copy(source.body),
    ...(source.fields.recurrence ? { recurrence: copy(source.fields.recurrence) } : {}),
    quiet: true,
  });
  for (const attachment of ctx.world.childrenOf(cardId)) {
    if (attachment.type !== 'attachment' || !attachment.file) continue;
    const clonedFileId = ctx.world.newId().replaceAll('-', '');
    const origFile = ctx.world.files.get(attachment.file.fileId);
    if (origFile) {
      ctx.world.files.set(clonedFileId, { ...origFile, fileId: clonedFileId, url: undefined });
    }
    const child = newStoredBlock(ctx, boardId, created.id, 'attachment');
    child.title = attachment.title;
    child.file = { ...attachment.file, fileId: clonedFileId };
    ctx.world.blocks.set(child.id, child);
    tellBlocks(ctx.world, boardId, [child]);
  }
  return created;
}

export function moveMemoryViewCards(
  ctx: MemoryContainerContext,
  boardId: string,
  viewId: string,
  move: CardMovesInput,
): CardMovesOutput {
  const board = requireBoardEditor(ctx, boardId);
  const view = blockInBoard(ctx.world, boardId, viewId, 'view') as StoredView;
  if (move.cardIds.length === 0 || move.cardIds.length > 200) {
    fail(400, 'cardIds must contain 1..=200 cards', { field: 'cardIds' });
  }
  if (new Set(move.cardIds).size !== move.cardIds.length) {
    fail(400, 'duplicate cardIds', { field: 'cardIds' });
  }
  const movedCards = move.cardIds.map((id) => {
    const block = ctx.world.blocks.get(id) as StoredCard | undefined;
    if (block?.type !== 'card' || block.containerId !== boardId || block.fields.isTemplate) {
      fail(400, 'card is not on this board', { field: 'cardIds' });
    }
    return block;
  });
  if (move.afterCardId !== undefined && move.afterCardId !== null && move.afterCardId !== '') {
    const after = ctx.world.blocks.get(move.afterCardId) as StoredCard | undefined;
    if (
      after?.type !== 'card' ||
      after.containerId !== boardId ||
      move.cardIds.includes(after.id)
    ) {
      fail(400, 'invalid afterCardId', { field: 'afterCardId' });
    }
  }
  const realCards = ctx.world
    .blocksOf(boardId)
    .filter((b): b is StoredCard => b.type === 'card' && !b.fields.isTemplate);
  const sorted = [...realCards].sort(manualComparator(view.fields.cardOrder));
  const remaining = sorted.filter((c) => !move.cardIds.includes(c.id)).map((c) => c.id);
  let insertAt = remaining.length;
  if (move.afterCardId === '') insertAt = 0;
  else if (move.afterCardId) insertAt = remaining.indexOf(move.afterCardId) + 1;
  const nextOrder = droppedCardOrder(
    sorted.map((c) => c.id),
    remaining,
    [...move.cardIds],
    insertAt,
  );
  view.fields = { ...view.fields, cardOrder: nextOrder };
  view.updatedAt = ctx.world.now();
  view.modifiedBy = ctx.userId;

  const made: StoredCard[] = [];
  const updatedCards = applyMovedGroupProperty(ctx, board, movedCards, move, made);
  tellBlocks(ctx.world, boardId, [view]);
  if (updatedCards.length > 0) tellBlocks(ctx.world, boardId, [...updatedCards, ...made]);
  return {
    view: viewOf(view),
    cards: updatedCards.map((c) => withLastComment(ctx.world, summaryOf(c))),
  };
}

/** Gives the moved cards the group's value; the next cards of those it makes done go to `made`. */
function applyMovedGroupProperty(
  ctx: MemoryContainerContext,
  board: Container,
  movedCards: readonly StoredCard[],
  move: CardMovesInput,
  made: StoredCard[],
): StoredCard[] {
  if (!move.groupPropertyId) return [];
  const prop = board.cardProperties.find((p) => p.id === move.groupPropertyId);
  if (!prop) fail(400, 'unknown groupPropertyId', { field: 'groupPropertyId' });
  const groupVal = move.groupValue;
  const cleared =
    groupVal === undefined ||
    groupVal === null ||
    groupVal === '' ||
    (Array.isArray(groupVal) && groupVal.length === 0);
  const updatedCards: StoredCard[] = [];
  for (const card of movedCards) {
    const moved = applyCardPatch(
      card,
      cleared ? { deletedProperties: [prop.id] } : { updatedProperties: { [prop.id]: groupVal } },
    );
    const following = settleCardChange(ctx, board, card, moved);
    moved.updatedAt = ctx.world.now();
    moved.modifiedBy = ctx.userId;
    ctx.world.blocks.set(card.id, moved);
    noteCardChanged(ctx.world, card, moved, ctx.userId);
    updatedCards.push(moved);
    if (following) made.push(makeNextCard(ctx, board, moved, following));
  }
  return updatedCards;
}

export function batchDeleteMemoryCards(
  ctx: MemoryContainerContext,
  boardId: string,
  cardIds: readonly string[],
): void {
  requireBoardEditor(ctx, boardId);
  if (cardIds.length === 0 || cardIds.length > 200 || new Set(cardIds).size !== cardIds.length) {
    fail(400, 'invalid cardIds', { field: 'cardIds' });
  }
  const allGoneIds: string[] = [];
  for (const cardId of cardIds) {
    const block = ctx.world.blocks.get(cardId);
    if (!block) continue;
    if (block.containerId !== boardId || block.type !== 'card') fail(404, 'no such card');
    const gone = subtreeOf(ctx.world, block);
    ctx.world.deletedBlocks.set(cardId, { containerId: boardId, blocks: gone, ...deletion(ctx) });
    for (const each of gone) {
      ctx.world.blocks.delete(each.id);
      allGoneIds.push(each.id);
    }
  }
  if (allGoneIds.length > 0) {
    ctx.world.tellContainer(boardId, {
      kind: 'blocksDeleted',
      containerId: boardId,
      blockIds: allGoneIds,
    });
  }
}

export function batchRestoreMemoryCards(
  ctx: MemoryContainerContext,
  boardId: string,
  cardIds: readonly string[],
): CardSummary[] {
  requireBoardEditor(ctx, boardId);
  if (cardIds.length === 0 || cardIds.length > 200 || new Set(cardIds).size !== cardIds.length) {
    fail(400, 'invalid cardIds', { field: 'cardIds' });
  }
  // As the server: every card is in the trash, as deleted or with what it was under, before any
  // comes back; one that came back with its family is not brought back again.
  const inFamilies = new Set<string>();
  for (const cardId of cardIds) {
    const deleted = ctx.world.deletedBlocks.get(cardId);
    if (deleted?.containerId === boardId)
      for (const block of deleted.blocks) inFamilies.add(block.id);
  }
  if (cardIds.some((cardId) => !inFamilies.has(cardId))) fail(404, 'no such block in the trash');
  const restoredBlocks: StoredBlock[] = [];
  const back = new Set<string>();
  for (const cardId of cardIds) {
    if (back.has(cardId) || !ctx.world.deletedBlocks.has(cardId)) continue;
    const restored = takeBlockBack(
      ctx.world,
      ctx.userId,
      boardId,
      cardId,
      () => memberRole(ctx.world, ctx.userId, boardId),
      () => containerInWorld(ctx.world, boardId),
    );
    for (const block of restored) back.add(block.id);
    restoredBlocks.push(...restored);
  }
  noteCardsRestored(ctx.world, restoredBlocks, ctx.userId);
  if (restoredBlocks.length > 0) tellBlocks(ctx.world, boardId, restoredBlocks);
  return restoredBlocks
    .filter((b): b is StoredCard => b.type === 'card')
    .map((c) => withLastComment(ctx.world, summaryOf(c)));
}

export function deleteMemoryComment(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
  commentId: string,
): void {
  const role = roleOn(ctx.world.members.get(boardId) ?? [], ctx.userId);
  if (role === '' || role === 'viewer') fail(403, 'access denied');
  const block = blockInBoard(ctx.world, boardId, commentId, 'comment');
  if (block.parentId !== cardId) fail(404, 'no such comment');
  // What is below it goes with it, and is the deleter's to delete too.
  const gone = subtreeOf(ctx.world, block);
  const entry = { containerId: boardId, blocks: gone, ...deletion(ctx) };
  if (!mayRemoveComments(role, ctx.userId, entry)) {
    fail(403, "access denied to delete other users' comments");
  }
  for (const each of gone) ctx.world.blocks.delete(each.id);
  ctx.world.deletedBlocks.set(commentId, entry);
  ctx.world.tellContainer(boardId, {
    kind: 'blocksDeleted',
    containerId: boardId,
    blockIds: gone.map((each) => each.id),
  });
}

export function restoreMemoryComment(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
  commentId: string,
): Comment {
  const restored = takeBlockBack(
    ctx.world,
    ctx.userId,
    boardId,
    commentId,
    () => memberRole(ctx.world, ctx.userId, boardId),
    () => containerInWorld(ctx.world, boardId),
  );
  const comment = restored.find((b) => b.id === commentId && b.parentId === cardId);
  if (!comment) fail(404, 'no such comment');
  tellBlocks(ctx.world, boardId, restored);
  return commentOf(comment);
}

/** The earlier texts of a comment of the board, for a member of it. */
export function memoryCommentVersions(
  ctx: MemoryContainerContext,
  boardId: string,
  commentId: string,
): CommentVersion[] {
  memberRole(ctx.world, ctx.userId, boardId);
  return versionsOf(ctx.world, boardId, commentId);
}

/**
 * Moves a view of the board, or an attachment of the card `cardId`, to the
 * trash with what is below it, as `deleteBoardView` and `deleteCardAttachment`
 * do: a block of another type, board or card is not there.
 */
export function deleteMemoryBlockOf(
  ctx: MemoryContainerContext,
  boardId: string,
  blockId: string,
  type: 'view' | 'attachment',
  cardId?: string,
): void {
  requireBoardEditor(ctx, boardId);
  if (cardId !== undefined) blockInBoard(ctx.world, boardId, cardId, 'card');
  const block = blockInBoard(ctx.world, boardId, blockId, type);
  if (cardId !== undefined && block.parentId !== cardId) fail(404, `no such ${type}`);
  const gone = subtreeOf(ctx.world, block);
  for (const each of gone) ctx.world.blocks.delete(each.id);
  ctx.world.deletedBlocks.set(blockId, { containerId: boardId, blocks: gone, ...deletion(ctx) });
  ctx.world.tellContainer(boardId, {
    kind: 'blocksDeleted',
    containerId: boardId,
    blockIds: gone.map((each) => each.id),
  });
}

/**
 * Takes a view of the board, or an attachment of the card `cardId`, back out
 * of the trash where it went on its own, as `restoreBoardView` and
 * `restoreCardAttachment` do: an attachment gone with its card is not there.
 */
export function restoreMemoryBlockOf(
  ctx: MemoryContainerContext,
  boardId: string,
  blockId: string,
  type: 'view' | 'attachment',
  cardId?: string,
): void {
  requireBoardEditor(ctx, boardId);
  if (cardId !== undefined) blockInBoard(ctx.world, boardId, cardId, 'card');
  const top = ctx.world.deletedBlocks.get(blockId)?.blocks[0];
  if (
    top?.id !== blockId ||
    top.type !== type ||
    (cardId !== undefined && top.parentId !== cardId)
  ) {
    fail(404, `no such ${type}`);
  }
  const restored = takeBlockBack(
    ctx.world,
    ctx.userId,
    boardId,
    blockId,
    () => memberRole(ctx.world, ctx.userId, boardId),
    () => containerInWorld(ctx.world, boardId),
  );
  tellBlocks(ctx.world, boardId, restored);
}
