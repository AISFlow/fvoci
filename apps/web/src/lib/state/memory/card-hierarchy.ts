/**
 * Card hierarchy rules in the memory backend, matching the server:
 * - Ordinary cards on a board may sit up to `MAX_CARD_DEPTH` (3) levels deep.
 * - A parent must be a live, non-template card on the same board, never itself or a descendant.
 * - Template cards cannot have a parent or child cards.
 * - Restoring a child card whose parent is gone re-roots it to the top level (`""`).
 */
import type { Container } from '../../logic/wire.ts';
import { BackendError, refusedMember, type CardDraft } from '../backend.ts';

import { settleNewCard } from './card-recurrence.ts';
import { checkedBody } from './bodies.ts';
import { numberBlock } from './card-numbers.ts';
import { checkRelations } from './references.ts';
import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import type { StoredBlock, StoredCard, World } from './world.ts';

/** Maximum card nesting depth: top-level = 1, child = 2, grandchild = 3. */
export const MAX_CARD_DEPTH = 3;

function isTemplateCard(block: StoredBlock): boolean {
  return (block.fields as { isTemplate?: unknown }).isTemplate === true;
}

function failCardTooDeep(): never {
  const detail = `the card would be deeper on its board than ${MAX_CARD_DEPTH}`;
  throw new BackendError(400, {
    type: '/problems/card-too-deep',
    title: 'Card too deep',
    status: 400,
    detail,
    reason: 'card_too_deep',
    limit: MAX_CARD_DEPTH,
    errors: [refusedMember('parentId', detail, 'card_too_deep')],
  });
}

function cardDepth(world: World, boardId: string, cardId: string): number {
  let depth = 0;
  let current = cardId;
  const seen = new Set<string>();
  while (current !== '' && depth <= MAX_CARD_DEPTH && !seen.has(current)) {
    seen.add(current);
    const block = world.blocks.get(current);
    if (block?.containerId !== boardId || block.type !== 'card') break;
    depth += 1;
    current = block.parentId;
  }
  return depth;
}

function cardSubtreeHeight(world: World, cardId: string): number {
  let height = 1;
  const seen = new Set<string>([cardId]);
  let frontier = [cardId];
  while (frontier.length > 0 && height <= MAX_CARD_DEPTH) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const child of world.childrenOf(id)) {
        if (child.type === 'card' && !seen.has(child.id)) {
          seen.add(child.id);
          next.push(child.id);
        }
      }
    }
    if (next.length === 0) break;
    height += 1;
    frontier = next;
  }
  return height;
}

function hasDescendant(world: World, cardId: string, candidateId: string): boolean {
  const seen = new Set<string>([cardId]);
  const stack = [cardId];
  while (stack.length > 0) {
    const id = stack.pop();
    if (id === undefined) break;
    for (const child of world.childrenOf(id)) {
      if (child.id === candidateId) return true;
      if (!seen.has(child.id)) {
        seen.add(child.id);
        stack.push(child.id);
      }
    }
  }
  return false;
}

function deletedCardHeight(blocks: readonly StoredBlock[], rootId: string): number {
  const byParent = new Map<string, string[]>();
  for (const block of blocks) {
    if (block.type === 'card' && block.parentId !== '') {
      const list = byParent.get(block.parentId) ?? [];
      list.push(block.id);
      byParent.set(block.parentId, list);
    }
  }
  let height = 1;
  const seen = new Set<string>([rootId]);
  let frontier = [rootId];
  while (frontier.length > 0) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const child of byParent.get(id) ?? []) {
        if (!seen.has(child)) {
          seen.add(child);
          next.push(child);
        }
      }
    }
    if (next.length === 0) break;
    height += 1;
    frontier = next;
  }
  return height;
}

/** Populates, validates, numbers, and stores a new card block on `board`. */
export function storeNewCard(
  world: World,
  board: Container,
  block: StoredBlock,
  draft: CardDraft,
): StoredCard {
  block.title = draft.title ?? '';
  block.fields = {
    icon: draft.icon ?? '',
    isTemplate: draft.isTemplate ?? false,
    properties: copy(draft.properties ?? {}),
    ...(draft.recurrence ? { recurrence: copy(draft.recurrence) } : {}),
  };
  block.body = checkedBody(world, board.id, draft.body ?? null);
  checkCardParent(world, board.id, block, block.parentId, true);
  checkRelations(world, board, block, undefined, 'properties');
  settleNewCard(board, block as StoredCard, draft.quiet ?? false);
  numberBlock(world, block);
  world.blocks.set(block.id, block);
  return block as StoredCard;
}

/** Validates a card's parent on create or patch, matching server store rules. */
export function checkCardParent(
  world: World,
  boardId: string,
  card: StoredBlock,
  parentId: string,
  isNew: boolean,
  before?: StoredBlock,
): void {
  if (isTemplateCard(card)) {
    if (parentId !== '')
      fail(400, 'a card template cannot have a parent card', { field: 'parentId' });
    if (!isNew && world.childrenOf(card.id).some((child) => child.type === 'card')) {
      fail(400, 'a card with sub-cards cannot become a template', { field: 'fields' });
    }
    return;
  }
  if (parentId === '') return;
  if (parentId === card.id || (!isNew && hasDescendant(world, card.id, parentId))) {
    fail(400, 'the parent is the block itself or below it', { field: 'parentId' });
  }
  const parent = world.blocks.get(parentId);
  if (parent?.containerId !== boardId) {
    fail(400, 'the parent is not a block of the board', { field: 'parentId' });
  }
  if (parent.type === 'comment') {
    fail(400, 'a comment is the parent of no block', { field: 'parentId' });
  }
  if (parent.type !== 'card') {
    fail(400, 'the parent of a card is a card of its board', { field: 'parentId' });
  }
  if (isTemplateCard(parent)) {
    fail(400, 'a card template cannot be the parent of a card', { field: 'parentId' });
  }
  if (before?.type === 'card' && !isTemplateCard(before) && before.parentId === parentId) {
    return;
  }
  const depth = cardDepth(world, boardId, parentId);
  const height = isNew ? 1 : cardSubtreeHeight(world, card.id);
  if (depth + height > MAX_CARD_DEPTH) failCardTooDeep();
}

/**
 * Validates restoring a deleted card family without mutating `blocks`.
 * Returns `true` when the top card's parent is gone and the caller should re-root it
 * to `""` once the restore commits.
 */
export function checkCardRestore(world: World, boardId: string, blocks: StoredBlock[]): boolean {
  const [top] = blocks;
  if (top?.type !== 'card' || top.parentId === '') return false;
  const parent = world.blocks.get(top.parentId);
  if (!parent) return true;
  if (isTemplateCard(top)) fail(409, 'the block cannot come back where it was');
  if (parent.containerId !== boardId || parent.type !== 'card' || isTemplateCard(parent)) {
    fail(409, 'the block cannot come back where it was');
  }
  const depth = cardDepth(world, boardId, parent.id);
  if (depth + deletedCardHeight(blocks, top.id) > MAX_CARD_DEPTH) {
    fail(409, `the block cannot come back where it was: deeper than ${MAX_CARD_DEPTH}`);
  }
  return false;
}

/** Validates board cards in an imported archive before anything is stored. */
export function checkArchiveCards(
  containers: readonly Container[],
  blocks: readonly StoredBlock[],
): void {
  const allContainerIds = new Set(containers.map((container) => container.id));
  const boardIds = new Set(
    containers.filter((container) => (container.kind ?? 'board') === 'board').map((c) => c.id),
  );
  const byId = new Map<string, StoredBlock>();
  const boardSeenIds = new Map<string, Set<string>>();
  for (const block of blocks) {
    if (boardIds.has(block.containerId)) {
      const seenOnBoard = boardSeenIds.get(block.containerId) ?? new Set<string>();
      if (seenOnBoard.has(block.id)) {
        fail(400, `the archive's board ${block.containerId} has two blocks of id ${block.id}`);
      }
      seenOnBoard.add(block.id);
      boardSeenIds.set(block.containerId, seenOnBoard);
    }
    if (!byId.has(block.id)) byId.set(block.id, block);
  }
  for (const block of blocks) {
    if (!boardIds.has(block.containerId) || block.type !== 'card') continue;
    if (isTemplateCard(block) && block.parentId !== '') {
      fail(400, 'a card template cannot have a parent card', { field: 'parentId' });
    }
    if (block.parentId === '') continue;
    let depth = 1;
    const seen = new Set<string>([block.id]);
    let cursor = block.parentId;
    while (cursor !== '') {
      if (seen.has(cursor)) {
        fail(400, 'the parent is the block itself or below it', { field: 'parentId' });
      }
      if (allContainerIds.has(cursor)) {
        fail(400, 'the parent is not a block of the board', { field: 'parentId' });
      }
      seen.add(cursor);
      const parent = byId.get(cursor);
      if (parent?.containerId !== block.containerId) {
        fail(400, 'the parent is not a block of the board', { field: 'parentId' });
      }
      if (parent.type !== 'card') {
        fail(400, 'the parent of a card is a card of its board', { field: 'parentId' });
      }
      if (isTemplateCard(parent)) {
        fail(400, 'a card template cannot be the parent of a card', { field: 'parentId' });
      }
      depth += 1;
      if (depth > MAX_CARD_DEPTH) failCardTooDeep();
      cursor = parent.parentId;
    }
  }
}
