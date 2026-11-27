/**
 * The bodies of the backend in memory: what a stored body may be, as the
 * server checks it, and the opening of a body in its room (`rooms.ts`,
 * fetched then with the room's client: Yjs is no part of the first screen).
 */
import { storedSize, validateDoc, type DocNode } from '../../editor/schema.ts';
import type { BodyDoc, BodyDocHears } from '../body-doc.ts';
import { openLater } from '../doc-opener.ts';

import type { RoomUser } from './rooms.ts';
import { fail } from './refuse.ts';
import { bodyFileIds, copy, hasFileBlock, LIMITS } from './shapes.ts';
import type { World } from './world.ts';

/**
 * A body written whole, checked; what the card's room wrote is written into
 * the card first, as the server does, so that it counts against `readAt`.
 */
export function checkedWhole(
  world: World,
  containerId: string,
  cardId: string,
  body: DocNode | null,
): DocNode | null {
  const checked = checkedBody(world, containerId, body);
  world.bodyFlushed?.(cardId);
  return checked;
}

/** A body as it is stored, or the server's refusal of it (400). An empty paragraph alone is none. */
export function checkedBody(
  world: World,
  containerId: string,
  body: DocNode | null,
  field: 'body' | 'appendBody' = 'body',
): DocNode | null {
  if (body === null) return null;
  const problems = validateDoc(body);
  if (problems.length > 0) fail(400, problems[0] ?? 'invalid body', { field });
  if (storedSize(body) > LIMITS.bodyBytes) {
    fail(400, 'the body is too large', { field, limit: LIMITS.bodyBytes });
  }
  for (const fileId of bodyFileIds(body)) {
    if (world.files.get(fileId)?.containerId !== containerId) {
      fail(400, 'an image or a file block names a file that is not of this board', {
        field,
      });
    }
  }
  const only = body.content?.length === 1 ? body.content[0] : undefined;
  const empty = only?.type === 'paragraph' && (only.content?.length ?? 0) === 0;
  return empty ? null : copy(body);
}

/** Why a comment's body is refused: it has a file block, which only a card's or a page's may. */
export const FILE_BLOCK_IN_COMMENT = "a comment's body has no file block";

/** A comment's body as it is stored, or the server's refusal: a comment has no file block. */
export function checkedComment(
  world: World,
  containerId: string,
  body: DocNode | null,
): DocNode | null {
  const checked = checkedBody(world, containerId, body);
  if (checked && hasFileBlock(checked)) {
    fail(400, FILE_BLOCK_IN_COMMENT, { field: 'body' });
  }
  return checked;
}

/** Opens a card's body in the world's room of it, for a user. */
export function openMemoryRoom(
  world: World,
  user: RoomUser,
  containerId: string,
  cardId: string,
  hears: BodyDocHears,
): BodyDoc {
  return openLater(async () => {
    const [{ roomsOf }, { openRoom }] = await Promise.all([
      import('./rooms.ts'),
      import('../doc-client.ts'),
    ]);
    const line = roomsOf(world).line(user, containerId, cardId);
    return (told) => openRoom(line, told, world.heldIdle);
  }, hears);
}

/**
 * What the room of a card took is written into the card first, so that a
 * read of it has it: the read a copy of a card is made from, as the server
 * does (e2e-live/body.spec.ts holds it to that).
 */
export function flushCard<Card extends { id: string }>(world: World, card: Card): Card {
  world.bodyFlushed?.(card.id);
  return card;
}

/** The same for every card and page of boards and wikis: before a copy of them, or an archive. */
export function flushContainers(world: World, containers: readonly { id: string }[]): void {
  for (const board of containers) {
    for (const block of world.blocksOf(board.id)) {
      if (['card', 'page', 'entry', 'unit'].includes(block.type)) flushCard(world, block);
    }
  }
}
