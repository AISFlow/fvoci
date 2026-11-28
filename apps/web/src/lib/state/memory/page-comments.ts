/**
 * The comments on the pages of the memory backend's wikis, as the server has
 * them (`getPageComments` and the rest in `contracts/openapi.json`): a
 * comment is a block of the type `comment` below its page, and who may do
 * what to one is what a card's comments let (`comments.ts`), by the role on
 * the wiki. The role is asked first, then the page, then the comment: who
 * may not do it learns nothing of what is there. A comment deleted on its
 * own comes back onto its page (`restoreComment`) by who may delete it, its
 * author only if they deleted it.
 *
 * `MemoryPageComments` is what `MemoryWikis` is besides its pages.
 */
import type { DocNode } from '../../editor/schema.ts';
import type { BodyWrite, Comment, CommentVersion } from '../backend.ts';
import type { WithReactions } from '../reaction-types.ts';

import { memberRole } from './container-access.ts';
import { checkedComment } from './bodies.ts';
import {
  earlierVersions,
  mayChangeComment,
  mayDeleteComment,
  mayRemoveComments,
  noteChange,
} from './comments.ts';
import { noteComment } from './notifications.ts';
import { withMemoryReactions } from './reactions.ts';
import { fail } from './refuse.ts';
import { commentOf, scramble } from './shapes.ts';
import { deletion } from './trash.ts';
import type { StoredBlock, World } from './world.ts';

/** Who asks, in which world, about the comments of which page or unit. */
export interface OnPage {
  readonly world: World;
  readonly userId: string;
  readonly wikiId: string;
  readonly pageId: string;
  readonly containerKind?: 'wiki' | 'course';
  readonly blockType?: 'page' | 'unit';
}

const containerKindOf = (on: OnPage): 'wiki' | 'course' => on.containerKind ?? 'wiki';
const blockTypeOf = (on: OnPage): 'page' | 'unit' => on.blockType ?? 'page';

/**
 * The role on the wiki or course, which is to be one to comment with; refused (403) for
 * a reader, in the server's words: `denial` for a change of a comment.
 */
function commenterRole(
  on: OnPage,
  denial = `access denied to comment on the ${blockTypeOf(on)}`,
): 'admin' | 'editor' | 'commenter' {
  const role = memberRole(on.world, on.userId, on.wikiId, containerKindOf(on));
  if (role === 'viewer') fail(403, denial);
  return role;
}

/** What the server refuses a reader's change of a comment with. */
const changesDenied = (on: OnPage): string =>
  `access denied to change the comments of the ${blockTypeOf(on)}`;

/** The page or unit, of the container and not in the trash: 404 otherwise. */
function pageOn(on: OnPage): StoredBlock {
  const kind = blockTypeOf(on);
  const page = on.world.blocks.get(on.pageId);
  if (page?.type !== kind || page.containerId !== on.wikiId) fail(404, `no such ${kind}`);
  return page;
}

/** A comment on the page; 404 for any other block, or none. */
function commentOn(on: OnPage, commentId: string): StoredBlock {
  const comment = on.world.blocks.get(commentId);
  if (comment?.type !== 'comment' || comment.parentId !== on.pageId) {
    fail(404, 'no such comment');
  }
  return comment;
}

function tell(on: OnPage, comment: StoredBlock): void {
  on.world.tellContainer(on.wikiId, {
    kind: 'blocks',
    containerId: on.wikiId,
    blocks: [commentOf(comment)],
  });
}

export function commentsOf(on: OnPage): Comment[] {
  memberRole(on.world, on.userId, on.wikiId, containerKindOf(on));
  pageOn(on);
  const comments = on.world.childrenOf(on.pageId).filter((block) => block.type === 'comment');
  // In no order to count on, as every list of this backend: the screens sort them.
  return scramble(comments).map(commentOf);
}

export function createComment(on: OnPage, body: DocNode): Comment {
  commenterRole(on);
  pageOn(on);
  const text = checkedComment(on.world, on.wikiId, body);
  if (text === null) fail(400, 'a comment has a text', { field: 'body' });
  const now = on.world.now();
  const comment: StoredBlock = {
    id: on.world.newId(),
    containerId: on.wikiId,
    parentId: on.pageId,
    createdBy: on.userId,
    modifiedBy: on.userId,
    type: 'comment',
    title: '',
    fields: {},
    body: text,
    createdAt: now,
    updatedAt: now,
  };
  on.world.blocks.set(comment.id, comment);
  tell(on, comment);
  noteComment(on.world, comment);
  return commentOf(comment);
}

export function patchComment(on: OnPage, commentId: string, write: BodyWrite): Comment {
  const role = commenterRole(on, changesDenied(on));
  pageOn(on);
  const comment = commentOn(on, commentId);
  if (!mayChangeComment(role, comment.createdBy === on.userId)) {
    fail(403, "access denied to change other users' comments");
  }
  const text = checkedComment(on.world, on.wikiId, write.body);
  if (text === null) fail(400, 'a comment has a text', { field: 'body' });
  if (comment.updatedAt !== write.readAt) fail(409, 'the comment changed since it was read');
  const before = comment.body ?? null;
  comment.body = text;
  comment.updatedAt = Math.max(on.world.now(), comment.updatedAt + 1);
  comment.modifiedBy = on.userId;
  noteChange(on.world, comment, before, on.userId);
  tell(on, comment);
  return commentOf(comment);
}

export function deleteComment(on: OnPage, commentId: string): void {
  const role = commenterRole(on, changesDenied(on));
  pageOn(on);
  const comment = commentOn(on, commentId);
  if (!mayDeleteComment(role, comment.createdBy === on.userId)) {
    fail(403, "access denied to delete other users' comments");
  }
  on.world.blocks.delete(commentId);
  on.world.deletedBlocks.set(commentId, {
    containerId: on.wikiId,
    blocks: [comment],
    ...deletion(on),
  });
  on.world.tellContainer(on.wikiId, {
    kind: 'blocksDeleted',
    containerId: on.wikiId,
    blockIds: [commentId],
  });
}

/** A comment deleted on its own, brought back onto its page, which is to be there. */
export function restoreComment(on: OnPage, commentId: string): Comment {
  const role = commenterRole(on, changesDenied(on));
  pageOn(on);
  const deleted = on.world.deletedBlocks.get(commentId);
  const [comment] = deleted?.blocks ?? [];
  if (deleted?.containerId !== on.wikiId || comment?.type !== 'comment')
    fail(404, 'no such comment');
  if (comment.id !== commentId || comment.parentId !== on.pageId) fail(404, 'no such comment');
  if (!mayRemoveComments(role, on.userId, deleted)) {
    // In the server's words: its author is told who brings back what somebody else deleted.
    const theirs = comment.createdBy === on.userId && deleted.deletedBy !== on.userId;
    fail(
      403,
      theirs
        ? "a comment somebody else deleted is brought back by the board's admins"
        : "access denied to undelete other users' comments",
    );
  }
  on.world.deletedBlocks.delete(commentId);
  // As the server: whoever brings it back is the last to change it, now.
  comment.updatedAt = on.world.now();
  comment.modifiedBy = on.userId;
  on.world.blocks.set(comment.id, comment);
  tell(on, comment);
  return commentOf(comment);
}

/** Also of a comment deleted on its own, whose page or unit is there: as the server keeps them. */
export function versionsOf(on: OnPage, commentId: string): CommentVersion[] {
  memberRole(on.world, on.userId, on.wikiId, containerKindOf(on));
  pageOn(on);
  const deleted = on.world.deletedBlocks.get(commentId)?.blocks[0];
  const comment = on.world.blocks.get(commentId) ?? deleted;
  if (comment?.type !== 'comment' || comment.parentId !== on.pageId) fail(404, 'no such comment');
  return earlierVersions(on.world.commentVersions.get(commentId) ?? []);
}

type CommentMethod =
  | 'comments'
  | 'createComment'
  | 'patchComment'
  | 'deleteComment'
  | 'restoreComment'
  | 'commentVersions';

/**
 * The comments of `Wikis` or `Courses` in memory: each call waiting as the
 * backend's do (`enter`) and asked as its user (`on`).
 */
export abstract class MemoryPageComments {
  protected abstract enter(method: CommentMethod): Promise<void>;
  protected abstract on(wikiId: string, pageId: string): OnPage;

  async comments(wikiId: string, pageId: string): Promise<WithReactions<Comment>[]> {
    await this.enter('comments');
    const on = this.on(wikiId, pageId);
    return commentsOf(on).map((comment) => withMemoryReactions(on.world, comment));
  }

  async createComment(wikiId: string, pageId: string, body: DocNode): Promise<Comment> {
    await this.enter('createComment');
    return createComment(this.on(wikiId, pageId), body);
  }

  async patchComment(
    wikiId: string,
    pageId: string,
    commentId: string,
    write: BodyWrite,
  ): Promise<Comment> {
    await this.enter('patchComment');
    return patchComment(this.on(wikiId, pageId), commentId, write);
  }

  async deleteComment(wikiId: string, pageId: string, commentId: string): Promise<void> {
    await this.enter('deleteComment');
    deleteComment(this.on(wikiId, pageId), commentId);
  }

  async restoreComment(wikiId: string, pageId: string, commentId: string): Promise<Comment> {
    await this.enter('restoreComment');
    return restoreComment(this.on(wikiId, pageId), commentId);
  }

  async commentVersions(
    wikiId: string,
    pageId: string,
    commentId: string,
  ): Promise<CommentVersion[]> {
    await this.enter('commentVersions');
    return versionsOf(this.on(wikiId, pageId), commentId);
  }
}
