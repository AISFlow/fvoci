/**
 * View, card, comment, attachment, and block delete/restore operations for
 * `MemoryBackend` (`state/memory/backend.ts`), extracted to keep `MemoryBackend`
 * within 500 lines without dismantling it.
 */
import { cardPreviewOf, type CardPreview } from '../../editor/card-preview.ts';
import type { DocNode } from '../../editor/schema.ts';
import type { BlockPatch, BoardView } from '../../logic/wire.ts';
import type {
  Attachment,
  BodyWrite,
  CardDetail,
  CardDraft,
  CardPatch,
  CardSummary,
  Comment,
  UploadedFile,
  ViewDraft,
} from '../backend.ts';
import { applyCardPatch, applyViewPatch } from '../patches.ts';

import { checkedComment, checkedWhole, flushCard } from './bodies.ts';
import { writtenWhole } from './body-versions.ts';
import { checkCardParent, storeNewCard } from './card-hierarchy.ts';
import {
  lastCommentAt,
  mayChangeComment,
  mayRemoveComments,
  noteChange,
  roleOn,
  type Role,
} from './comments.ts';
import { memberRole } from './container-access.ts';
import {
  containerInWorld,
  newStoredBlock,
  tellBlocks,
  type MemoryContainerContext,
} from './container-ops.ts';
import { noteComment } from './notifications.ts';
import { checkRelations } from './references.ts';
import { fail } from './refuse.ts';
import { attachmentOf, commentOf, copy, detailOf, scramble, summaryOf, viewOf } from './shapes.ts';
import { deletion, mayManageCards, subtreeOf, takeBlockBack } from './trash.ts';
import type { StoredBlock, StoredCard, StoredView, World } from './world.ts';

export function blockInBoard(
  world: World,
  boardId: string,
  blockId: string,
  type?: StoredBlock['type'],
): StoredBlock {
  const block = world.blocks.get(blockId);
  const own =
    block?.containerId === boardId && !['page', 'entry', 'unit', 'chapter'].includes(block.type);
  if (!block || !own || (type && block.type !== type)) fail(404, `no such ${type ?? 'block'}`);
  return block;
}

function touchBlock(ctx: MemoryContainerContext, block: StoredBlock): void {
  block.updatedAt = ctx.world.now();
  block.modifiedBy = ctx.userId;
}

export function withLastComment<T extends CardSummary>(world: World, card: T): T {
  return { ...card, lastCommentAt: lastCommentAt(world.childrenOf(card.id)) };
}

export function createMemoryView(
  ctx: MemoryContainerContext,
  boardId: string,
  draft: ViewDraft,
): BoardView {
  containerInWorld(ctx.world, boardId, 'board');
  mayManageCards(ctx.world, ctx.userId, boardId, 'access denied to make board changes');
  const block = newStoredBlock(ctx, boardId, '', 'view');
  block.title = draft.title;
  block.fields = copy(draft.fields);
  ctx.world.blocks.set(block.id, block);
  tellBlocks(ctx.world, boardId, [block]);
  return viewOf(block);
}

export function patchMemoryView(
  ctx: MemoryContainerContext,
  boardId: string,
  viewId: string,
  sent: BlockPatch,
): { before: BoardView; after: BoardView } {
  mayManageCards(ctx.world, ctx.userId, boardId, 'access denied to make board changes');
  const stored = blockInBoard(ctx.world, boardId, viewId, 'view') as StoredView;
  if (sent.readAt !== undefined && stored.updatedAt !== sent.readAt) {
    fail(409, 'the block has changed since it was read');
  }
  const view: StoredView = { ...stored, ...applyViewPatch(stored, sent) };
  ctx.world.blocks.set(viewId, view);
  touchBlock(ctx, view);
  tellBlocks(ctx.world, boardId, [view]);
  return { before: viewOf(stored), after: viewOf(view) };
}

export function readMemoryCard(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
): CardDetail {
  memberRole(ctx.world, ctx.userId, boardId);
  const card = flushCard(ctx.world, blockInBoard(ctx.world, boardId, cardId, 'card') as StoredCard);
  return withLastComment(ctx.world, detailOf(card));
}

export function createMemoryCard(
  ctx: MemoryContainerContext,
  boardId: string,
  draft: CardDraft,
): CardSummary {
  const board = containerInWorld(ctx.world, boardId, 'board');
  mayManageCards(ctx.world, ctx.userId, boardId, 'access denied to create card');
  const block = newStoredBlock(ctx, boardId, draft.parentId ?? '', 'card');
  const stored = storeNewCard(ctx.world, board, block, draft);
  tellBlocks(ctx.world, boardId, [stored]);
  return withLastComment(ctx.world, summaryOf(stored));
}

export function patchMemoryCard(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
  patch: CardPatch,
): CardSummary {
  mayManageCards(ctx.world, ctx.userId, boardId, 'access denied to patch card');
  const before = blockInBoard(ctx.world, boardId, cardId, 'card') as StoredCard;
  const card = applyCardPatch(before, patch);
  checkCardParent(ctx.world, boardId, card, card.parentId, false, before);
  checkRelations(
    ctx.world,
    containerInWorld(ctx.world, boardId),
    card,
    before,
    'updatedProperties',
  );
  ctx.world.blocks.set(cardId, card);
  touchBlock(ctx, card);
  tellBlocks(ctx.world, boardId, [card]);
  return withLastComment(ctx.world, summaryOf(card));
}

export function writeMemoryCardBody(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
  write: BodyWrite,
): CardDetail {
  const card = blockInBoard(ctx.world, boardId, cardId, 'card') as StoredCard;
  const body = checkedWhole(ctx.world, boardId, cardId, write.body);
  if (card.updatedAt !== write.readAt) fail(409, 'the card changed since it was read');
  writtenWhole(ctx.world, card, body, ctx.userId, () => {
    card.body = body;
    touchBlock(ctx, card);
  });
  ctx.world.bodyWritten?.(cardId);
  tellBlocks(ctx.world, boardId, [card]);
  return withLastComment(ctx.world, detailOf(card));
}

export function listMemoryComments(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
): Comment[] {
  memberRole(ctx.world, ctx.userId, boardId);
  blockInBoard(ctx.world, boardId, cardId, 'card');
  return scramble(ctx.world.childrenOf(cardId).filter((block) => block.type === 'comment')).map(
    commentOf,
  );
}

export function createMemoryComment(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
  body: DocNode,
): Comment {
  blockInBoard(ctx.world, boardId, cardId, 'card');
  const checked = checkedComment(ctx.world, boardId, body);
  if (checked === null) fail(400, 'a comment needs a text', { field: 'body' });
  const block = newStoredBlock(ctx, boardId, cardId, 'comment');
  block.body = checked;
  ctx.world.blocks.set(block.id, block);
  tellBlocks(ctx.world, boardId, [block]);
  noteComment(ctx.world, block);
  return commentOf(block);
}

export function patchMemoryComment(
  ctx: MemoryContainerContext,
  boardId: string,
  commentId: string,
  write: BodyWrite,
): Comment {
  const role = memberRole(ctx.world, ctx.userId, boardId);
  if (role === 'viewer') fail(403, 'access denied to make board changes');
  const block = blockInBoard(ctx.world, boardId, commentId, 'comment');
  if (!mayChangeComment(role, block.createdBy === ctx.userId)) {
    fail(403, "access denied to change other users' comments");
  }
  const checked = checkedComment(ctx.world, boardId, write.body);
  if (checked === null) fail(400, 'a comment needs a text', { field: 'body' });
  if (block.updatedAt !== write.readAt) {
    fail(409, 'the comment changed since it was read');
  }
  const before = block.body ?? null;
  block.body = checked;
  touchBlock(ctx, block);
  noteChange(ctx.world, block, before, ctx.userId);
  tellBlocks(ctx.world, boardId, [block]);
  return commentOf(block);
}

export function listMemoryAttachments(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
): Attachment[] {
  memberRole(ctx.world, ctx.userId, boardId);
  blockInBoard(ctx.world, boardId, cardId, 'card');
  return scramble(ctx.world.childrenOf(cardId).filter((block) => block.type === 'attachment')).map(
    attachmentOf,
  );
}

export function createMemoryAttachment(
  ctx: MemoryContainerContext,
  boardId: string,
  cardId: string,
  file: UploadedFile,
): Attachment {
  blockInBoard(ctx.world, boardId, cardId, 'card');
  const stored = ctx.world.files.get(file.fileId);
  if (stored?.containerId !== boardId) {
    fail(400, 'no such file on this board', { field: 'fileId' });
  }
  const block = newStoredBlock(ctx, boardId, cardId, 'attachment');
  block.title = file.name;
  block.file = {
    fileId: stored.fileId,
    name: file.name,
    size: stored.size,
    contentType: stored.contentType,
  };
  ctx.world.blocks.set(block.id, block);
  tellBlocks(ctx.world, boardId, [block]);
  return attachmentOf(block);
}

export function deleteMemoryBlock(
  ctx: MemoryContainerContext,
  boardId: string,
  blockId: string,
): void {
  const block = blockInBoard(ctx.world, boardId, blockId);
  const gone = subtreeOf(ctx.world, block);
  const entry = { containerId: boardId, blocks: gone, ...deletion(ctx) };
  const role: Role = roleOn(ctx.world.members.get(boardId) ?? [], ctx.userId);
  if (block.type === 'comment' && !mayRemoveComments(role, ctx.userId, entry)) {
    fail(403, "access denied to delete other users' comments");
  }
  for (const each of gone) ctx.world.blocks.delete(each.id);
  ctx.world.deletedBlocks.set(blockId, entry);
  ctx.world.tellContainer(boardId, {
    kind: 'blocksDeleted',
    containerId: boardId,
    blockIds: gone.map((each) => each.id),
  });
}

export function restoreMemoryBlock(
  ctx: MemoryContainerContext,
  boardId: string,
  blockId: string,
): void {
  const restored = takeBlockBack(
    ctx.world,
    ctx.userId,
    boardId,
    blockId,
    () => memberRole(ctx.world, ctx.userId, boardId),
    () => {
      containerInWorld(ctx.world, boardId);
    },
  );
  tellBlocks(ctx.world, boardId, restored);
}

export function listMemoryViews(ctx: MemoryContainerContext, boardId: string): BoardView[] {
  memberRole(ctx.world, ctx.userId, boardId);
  return scramble(ctx.world.blocksOf(boardId).filter((b) => b.type === 'view')).map(viewOf);
}

export function listMemoryCards(ctx: MemoryContainerContext, boardId: string): CardSummary[] {
  memberRole(ctx.world, ctx.userId, boardId);
  const cards = ctx.world.blocksOf(boardId).filter((b) => b.type === 'card');
  return scramble(cards).map((c) => withLastComment(ctx.world, summaryOf(c as StoredCard)));
}

export function listMemoryCardPreviews(
  ctx: MemoryContainerContext,
  boardId: string,
): Map<string, CardPreview> {
  memberRole(ctx.world, ctx.userId, boardId);
  const cards = ctx.world.blocksOf(boardId).filter((b) => b.type === 'card');
  return new Map(cards.map((c) => [c.id, cardPreviewOf(c.body)]));
}
