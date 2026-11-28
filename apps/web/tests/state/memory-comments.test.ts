/**
 * Comments in the memory backend as on the server: who may change and
 * delete which, when a comment counts as changed, and the texts kept of it.
 */
import { describe, expect, it } from 'vitest';

import { BackendError, type ContainerRole } from '$lib/state/index.ts';
import { COMMENT_VERSIONS_KEPT } from '$lib/state/memory/comments.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';
import type { World } from '$lib/state/memory/world.ts';

import { setup, text } from './support.ts';

async function onAlpha() {
  const made = setup();
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  const cards = await made.backend.cards(board?.id ?? '');
  const alpha = cards.find((card) => card.title === 'Alpha');
  if (!board || !alpha) throw new Error('no card');
  return { ...made, boardId: board.id, cardId: alpha.id };
}

/** Gives `who` exactly this role on the board. */
function setRole(world: World, boardId: string, who: MemoryBackend, role: ContainerRole): void {
  const members = world.members.get(boardId) ?? [];
  world.members.set(
    boardId,
    members.map((member) => (member.userId === who.userId ? { ...member, role } : member)),
  );
}

const statusOf = async (promise: Promise<unknown>): Promise<number | 'ok'> => {
  try {
    await promise;
    return 'ok';
  } catch (error) {
    if (error instanceof BackendError) return error.status;
    throw error;
  }
};

describe('who changes and deletes a comment', () => {
  it('its author with the commenter’s role changes it; only an admin changes anybody else’s', async () => {
    const { world, backend, other, boardId, cardId } = await onAlpha();
    const mine = await backend.createComment(boardId, cardId, text('mine'));
    const write = async (who: MemoryBackend, words: string) => {
      const now = world.blocks.get(mine.id)?.updatedAt ?? 0;
      return statusOf(
        who.patchComment(boardId, mine.id, { body: text(words), readAt: now }, cardId),
      );
    };

    setRole(world, boardId, backend, 'commenter');
    expect(await write(backend, 'as a commenter')).toBe('ok');
    setRole(world, boardId, backend, 'viewer');
    expect(await write(backend, 'as a viewer')).toBe(403);

    setRole(world, boardId, other, 'editor');
    expect(await write(other, 'by an editor')).toBe(403);
    setRole(world, boardId, other, 'admin');
    expect(await write(other, 'by an admin')).toBe('ok');
    expect(world.blocks.get(mine.id)?.body).toEqual(text('by an admin'));
  });

  it('its author with the commenter’s role deletes it and brings it back; anybody else’s is for an admin', async () => {
    const { world, backend, other, boardId, cardId } = await onAlpha();
    const first = await backend.createComment(boardId, cardId, text('one'));
    const second = await backend.createComment(boardId, cardId, text('two'));

    setRole(world, boardId, backend, 'viewer');
    expect(await statusOf(backend.deleteComment(boardId, cardId, first.id))).toBe(403);
    setRole(world, boardId, backend, 'commenter');
    expect(await statusOf(backend.deleteComment(boardId, cardId, first.id))).toBe('ok');
    // Somebody else's, deleted or in the trash, is not an editor's.
    setRole(world, boardId, other, 'editor');
    expect(await statusOf(other.restoreComment(boardId, cardId, first.id))).toBe(403);
    expect(await statusOf(other.deleteComment(boardId, cardId, second.id))).toBe(403);
    // Its author, a viewer now, does not bring it back.
    setRole(world, boardId, backend, 'viewer');
    expect(await statusOf(backend.restoreComment(boardId, cardId, first.id))).toBe(403);
    setRole(world, boardId, backend, 'commenter');
    expect(await statusOf(backend.restoreComment(boardId, cardId, first.id))).toBe('ok');
    setRole(world, boardId, other, 'admin');
    expect(await statusOf(other.deleteComment(boardId, cardId, second.id))).toBe('ok');
    // Deleted by the admin, it is not its author's to bring back, but the admin's.
    expect(await statusOf(backend.restoreComment(boardId, cardId, second.id))).toBe(403);
    expect(await statusOf(other.restoreComment(boardId, cardId, second.id))).toBe('ok');
  });
});

describe('a comment with a reply of somebody else', () => {
  it('is deleted and brought back by an admin only, as the reply goes with it', async () => {
    const { world, backend, other, boardId, cardId } = await onAlpha();
    const mine = await backend.createComment(boardId, cardId, text('mine'));
    // A reply: somebody else's comment under it (the server takes one; this app makes none).
    const reply = await other.createComment(boardId, cardId, text('theirs'));
    const stored = world.blocks.get(reply.id);
    if (stored) stored.parentId = mine.id;
    setRole(world, boardId, backend, 'commenter');
    expect(await statusOf(backend.deleteComment(boardId, cardId, mine.id))).toBe(403);
    setRole(world, boardId, backend, 'admin');
    expect(await statusOf(backend.deleteComment(boardId, cardId, mine.id))).toBe('ok');
    setRole(world, boardId, backend, 'commenter');
    expect(await statusOf(backend.restoreComment(boardId, cardId, mine.id))).toBe(403);
    setRole(world, boardId, backend, 'admin');
    expect(await statusOf(backend.restoreComment(boardId, cardId, mine.id))).toBe('ok');

    // Any other block under it is an editor's.
    if (stored) stored.type = 'attachment';
    setRole(world, boardId, backend, 'commenter');
    expect(await statusOf(backend.deleteComment(boardId, cardId, mine.id))).toBe(403);
    setRole(world, boardId, backend, 'editor');
    expect(await statusOf(backend.deleteComment(boardId, cardId, mine.id))).toBe('ok');
  });
});

describe('a comment brought back', () => {
  it('names whoever brought it back as the last to change it, as the server does', async () => {
    const { world, backend, other, boardId, cardId } = await onAlpha();
    const posted = await backend.createComment(boardId, cardId, text('kept'));
    setRole(world, boardId, other, 'admin');
    await other.deleteComment(boardId, cardId, posted.id);
    await other.restoreComment(boardId, cardId, posted.id);
    const back = (await backend.comments(boardId, cardId)).find((one) => one.id === posted.id);
    expect(back?.modifiedBy).toBe(other.userId);
    expect(back?.updatedAt).toBeGreaterThan(posted.updatedAt);
    expect(back?.editedAt).toBeUndefined();
  });

  it('refuses a viewer before it looks for the comment', async () => {
    const { world, backend, boardId, cardId } = await onAlpha();
    setRole(world, boardId, backend, 'viewer');
    const write = { body: text('x'), readAt: 1 };
    expect(await statusOf(backend.patchComment(boardId, 'nothing', write, cardId))).toBe(403);
  });
});

describe('a changed comment', () => {
  it('says when its text was changed and by whom, and not for a write of the same text', async () => {
    const { world, backend, other, boardId, cardId } = await onAlpha();
    const posted = await backend.createComment(boardId, cardId, text('first'));
    expect(posted.editedAt).toBeUndefined();

    const same = await backend.patchComment(
      boardId,
      posted.id,
      { body: text('first'), readAt: posted.updatedAt },
      cardId,
    );
    expect(same.editedAt).toBeUndefined();
    expect(await backend.commentVersions(boardId, posted.id, cardId)).toEqual([]);

    setRole(world, boardId, other, 'admin');
    const changed = await other.patchComment(
      boardId,
      posted.id,
      { body: text('second'), readAt: same.updatedAt },
      cardId,
    );
    expect(changed.editedAt).toBe(changed.updatedAt);
    expect(changed.modifiedBy).toBe(other.userId);
    expect((await backend.comments(boardId, cardId)).find((one) => one.id === posted.id)).toEqual(
      changed,
    );
  });

  it('keeps what it said before, posted text first, the newest up to the limit', async () => {
    const { world, backend, other, boardId, cardId } = await onAlpha();
    const posted = await backend.createComment(boardId, cardId, text('v1'));
    setRole(world, boardId, other, 'admin');
    let readAt = posted.updatedAt;
    const change = async (who: MemoryBackend, words: string) => {
      readAt = (await who.patchComment(boardId, posted.id, { body: text(words), readAt }, cardId))
        .updatedAt;
    };
    await change(other, 'v2');
    await change(backend, 'v3');

    const versions = await backend.commentVersions(boardId, posted.id, cardId);
    expect(versions.map(({ writtenAt: _at, ...rest }) => rest)).toEqual([
      { version: 1, body: text('v1'), writtenBy: backend.userId },
      { version: 2, body: text('v2'), writtenBy: other.userId },
    ]);
    expect(versions[0]?.writtenAt).toBe(posted.createdAt);
    expect(versions[1]?.writtenAt).toBeGreaterThan(posted.createdAt);

    // Many changes later: the first and the newest, as many as the server keeps.
    for (let at = 4; at <= 61; at += 1) await change(backend, `v${at}`);
    const kept = await backend.commentVersions(boardId, posted.id, cardId);
    expect(kept).toHaveLength(COMMENT_VERSIONS_KEPT);
    expect(kept.map((version) => version.version)).toEqual([
      1,
      ...Array.from({ length: COMMENT_VERSIONS_KEPT - 1 }, (_, index) => 12 + index),
    ]);
    expect(kept.at(-1)?.body).toEqual(text('v60'));
  });

  it('shows its earlier texts to members only, also from the trash, and only of comments', async () => {
    const { world, backend, other, boardId, cardId } = await onAlpha();
    const posted = await backend.createComment(boardId, cardId, text('before'));
    await backend.patchComment(
      boardId,
      posted.id,
      { body: text('after'), readAt: posted.updatedAt },
      cardId,
    );
    setRole(world, boardId, other, 'viewer');
    expect(await other.commentVersions(boardId, posted.id, cardId)).toHaveLength(1);

    world.members.set(
      boardId,
      (world.members.get(boardId) ?? []).filter((member) => member.userId !== other.userId),
    );
    // Not there for who is no member on a private board; readable on an open board.
    expect(await statusOf(other.commentVersions(boardId, posted.id, cardId))).toBe(404);
    const board = world.containers.get(boardId);
    if (board) world.containers.set(boardId, { ...board, visibility: 'open' });
    expect(await other.commentVersions(boardId, posted.id, cardId)).toHaveLength(1);

    await backend.deleteComment(boardId, cardId, posted.id);
    expect(await backend.commentVersions(boardId, posted.id, cardId)).toHaveLength(1);
    expect(await statusOf(backend.commentVersions(boardId, cardId, cardId))).toBe(404);
  });
});
