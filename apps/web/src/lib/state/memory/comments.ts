/**
 * The memory backend's comments as the server has them: who may change or
 * delete one (`patchCardComment` and `deleteCardComment` in
 * `contracts/openapi.json`), and the texts kept of one that was changed
 * (`getCardCommentVersions`).
 */
import type { DocNode } from '../../editor/schema.ts';
import type { ContainerMember } from '../../logic/wire.ts';
import type { CommentVersion } from '../backend.ts';

import { fail } from './refuse.ts';
import type { StoredBlock, World } from './world.ts';

export type Role = 'admin' | 'editor' | 'commenter' | 'viewer' | '';

/** The highest role of a user on a board; none for who is no member. */
export function roleOn(members: readonly ContainerMember[], userId: string): Role {
  return members.find((each) => each.userId === userId)?.role ?? '';
}

/** Its author changes a comment with the commenter's role; anybody else's is for the admins. */
export function mayChangeComment(role: Role, isAuthor: boolean): boolean {
  if (role === 'admin') return true;
  return isAuthor && (role === 'editor' || role === 'commenter');
}

/** Its author deletes a comment, and brings it back, with the commenter's role; anybody else's is for the admins. */
export function mayDeleteComment(role: Role, isAuthor: boolean): boolean {
  if (role === 'admin') return true;
  return isAuthor && (role === 'editor' || role === 'commenter');
}

/**
 * When the newest comment directly on the card was posted; 0 for none. A
 * deleted one is in the trash and does not count.
 */
export function lastCommentAt(blocks: readonly StoredBlock[]): number {
  return Math.max(0, ...blocks.filter((block) => block.type === 'comment').map((c) => c.createdAt));
}

/**
 * Whether the user may delete a comment with what goes with it, or bring that
 * back: every comment of it, its replies too, theirs to delete, and any other
 * block under it an editor's. Brought back (with `deletedBy`, who deleted
 * it), a comment is its author's only if they deleted it, as the server has it.
 */
export function mayRemoveComments(
  role: Role,
  userId: string,
  { blocks, deletedBy = userId }: Removal,
): boolean {
  return blocks.every((block) =>
    block.type === 'comment'
      ? mayDeleteComment(role, block.createdBy === userId && deletedBy === userId)
      : role === 'admin' || role === 'editor',
  );
}

/** What is deleted, or brought back with who deleted it. */
interface Removal {
  readonly blocks: readonly Pick<StoredBlock, 'type' | 'createdBy'>[];
  readonly deletedBy?: string;
}

/** How many texts before the one it has now a comment keeps: the server's default. */
export const COMMENT_VERSIONS_KEPT = 50;

/** A comment's text as it was: what a change writes down. */
export interface CommentText {
  body: DocNode | null;
  by: string;
  at: number;
}

/**
 * Writes down a change of a comment's text. `versions` holds every text
 * written down, the one it has now last; the first change also writes the
 * text it was posted with (`posted`). The first and the newest are kept,
 * `COMMENT_VERSIONS_KEPT` before the one it has now.
 */
export function recordChange(
  versions: readonly CommentVersion[],
  posted: CommentText,
  now: CommentText,
): CommentVersion[] {
  const kept = versions.length > 0 ? [...versions] : [versionOf(1, posted)];
  const newest = kept.at(-1)?.version ?? 1;
  kept.push(versionOf(newest + 1, now));
  const cut = newest + 1 - COMMENT_VERSIONS_KEPT;
  return kept.filter((version, index) => index === 0 || version.version > cut);
}

/**
 * Writes down a change of a stored comment's text from `before` by `userId`,
 * its time the comment's `updatedAt`; the same text again is no change.
 */
export function noteChange(
  world: World,
  block: StoredBlock,
  before: DocNode | null,
  userId: string,
): void {
  const now = block.body ?? null;
  if (JSON.stringify(before) === JSON.stringify(now)) return;
  block.editedAt = block.updatedAt;
  world.commentVersions.set(
    block.id,
    recordChange(
      world.commentVersions.get(block.id) ?? [],
      { body: before, by: block.createdBy, at: block.createdAt },
      { body: now, by: userId, at: block.updatedAt },
    ),
  );
}

/** The earlier texts of a comment of the board, also of one in the trash (as on the server); 404 for none. */
export function versionsOf(world: World, boardId: string, commentId: string): CommentVersion[] {
  const trashed = [...world.deletedBlocks.values()]
    .filter((deleted) => deleted.containerId === boardId)
    .flatMap((deleted) => deleted.blocks);
  const comment = world.blocks.get(commentId) ?? trashed.find((one) => one.id === commentId);
  if (comment?.containerId !== boardId || comment.type !== 'comment') fail(404, 'no such comment');
  return earlierVersions(world.commentVersions.get(commentId) ?? []);
}

/** The texts of a comment before the one it has now, the first first. */
export function earlierVersions(versions: readonly CommentVersion[]): CommentVersion[] {
  return structuredClone(versions.slice(0, -1));
}

function versionOf(version: number, text: CommentText): CommentVersion {
  return { version, body: structuredClone(text.body), writtenBy: text.by, writtenAt: text.at };
}
