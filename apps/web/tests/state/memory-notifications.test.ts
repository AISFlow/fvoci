/**
 * Notifications in the memory backend, as the server makes those of
 * comments: who watches a card or a page is told of a comment somebody else
 * writes on it, at the comment; what happens to one within the hour is one
 * notification that counts it; a switch turned off tells nothing; and what
 * the reader no longer reads says nothing of itself.
 */
import { describe, expect, it } from 'vitest';

import { BackendError, type Change } from '$lib/state/index.ts';

import { settle, setup, text } from './support.ts';

const HOUR = 60 * 60 * 1000;

/** A card the reader writes on the seeded board, which the other person is a member of. */
async function aCard() {
  const made = setup();
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  if (!workspace || !board) throw new Error('no board');
  const card = await made.backend.createCard(board.id, { title: 'Plan' });
  // A clock the test moves on: what happens within the hour joins.
  // Ahead of the times the world has given out already: it never goes back.
  let now = Date.now() + HOUR;
  made.world.useClock(() => now);
  const later = (by: number): void => {
    now += by;
  };
  return { ...made, workspaceId: workspace.id, boardId: board.id, cardId: card.id, later };
}

const notifications = <T extends { notifications: unknown }>(backend: T) => {
  const kept = backend.notifications;
  if (!kept) throw new Error('the memory backend keeps notifications');
  return kept as NonNullable<T['notifications']>;
};

describe('the notifications of comments, in memory', () => {
  it('tell who wrote the card of a comment somebody else writes, at it; one’s own tells nobody', async () => {
    const made = await aCard();
    const told: Change[] = [];
    made.backend.watchSelf((change) => told.push(change));
    const theirs = await made.other.createComment(made.boardId, made.cardId, text('looks good'));
    await settle();
    expect(told).toContainEqual({ kind: 'notifications' });
    const mine = notifications(made.backend);
    expect(await mine.unread()).toBe(1);
    const [first] = await mine.list();
    expect(first?.subject).toEqual({
      kind: 'comment',
      actorId: made.other.userId,
      times: 1,
      people: 1,
      workspaceId: made.workspaceId,
      containerId: made.boardId,
      containerKind: 'board',
      blockId: made.cardId,
      blockKind: 'card',
      title: 'Plan',
      commentId: theirs.id,
    });
    expect(await notifications(made.other).unread()).toBe(0);

    // The other person watches now, having commented: the reader's comment tells them.
    await made.backend.createComment(made.boardId, made.cardId, text('thanks'));
    expect(await notifications(made.other).unread()).toBe(1);
    expect(await mine.unread()).toBe(1);
  });

  it('join what happens to the card within the hour, by the latest and unread again; after it, one more', async () => {
    const made = await aCard();
    const mine = notifications(made.backend);
    await made.other.createComment(made.boardId, made.cardId, text('one'));
    const [first] = await mine.list();
    if (!first) throw new Error('told');
    await mine.read(first.id);
    expect(await mine.unread()).toBe(0);

    made.later(HOUR - 10);
    const second = await made.other.createComment(made.boardId, made.cardId, text('two'));
    const [joined, ...more] = await mine.list();
    expect(more).toEqual([]);
    expect(joined?.id).toBe(first.id);
    expect([joined?.subject?.times, joined?.subject?.commentId, joined?.readAt]).toEqual([
      2,
      second.id,
      0,
    ]);
    expect(joined?.updatedAt).toBeGreaterThan(first.updatedAt);

    made.later(20);
    await made.other.createComment(made.boardId, made.cardId, text('three'));
    const lines = await mine.list();
    expect(lines.map((line) => line.subject?.times)).toEqual([1, 2]);
    expect(await mine.unread()).toBe(2);

    // Read through the older one: the newer stays unread; then all of them.
    await mine.readAll(joined?.updatedAt);
    expect(await mine.unread()).toBe(1);
    await mine.readAll();
    expect(await mine.unread()).toBe(0);
  });

  it('tell nothing of comments to whom switched them off, and what was off does not come later', async () => {
    const made = await aCard();
    await made.backend.patchSettings({ notifyComments: false });
    await made.other.createComment(made.boardId, made.cardId, text('unheard'));
    await made.backend.patchSettings({ notifyComments: null });
    expect(await notifications(made.backend).list()).toEqual([]);
    await made.other.createComment(made.boardId, made.cardId, text('heard'));
    expect(await notifications(made.backend).unread()).toBe(1);
  });

  it('say nothing of a card the reader no longer reads, and a page’s comment tells its writer', async () => {
    const made = await aCard();
    const mine = notifications(made.backend);
    await made.other.createComment(made.boardId, made.cardId, text('soon gone'));
    await made.backend.deleteBlock(made.boardId, made.cardId);
    const [gone] = await mine.list();
    expect(gone?.subject).toBeNull();

    const wiki = await made.backend.wiki.createWiki(made.workspaceId, {
      title: 'Notes',
      visibility: 'open',
    });
    await made.other.joinContainer(wiki.id);
    const page = await made.backend.wiki.createPage(wiki.id, { title: 'Minutes' });
    const said = await made.other.wiki.createComment(wiki.id, page.id, text('on the page'));
    const [newest] = await mine.list();
    expect(newest?.subject).toMatchObject({
      blockKind: 'page',
      containerKind: 'wiki',
      blockId: page.id,
      containerId: wiki.id,
      commentId: said.id,
    });
  });

  it('are read by their reader alone', async () => {
    const made = await aCard();
    await made.other.createComment(made.boardId, made.cardId, text('mine to read'));
    const [one] = await notifications(made.backend).list();
    await expect(notifications(made.other).read(one?.id ?? '')).rejects.toBeInstanceOf(
      BackendError,
    );
    expect(await notifications(made.backend).unread()).toBe(1);
  });
});
