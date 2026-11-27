/**
 * The time of a card's last comment: what the card list says, kept current
 * by the board as comments come and go, as the contract says a client does.
 */
import { describe, expect, it, vi } from 'vitest';

import { openFirstBoard, settle, text } from './support.ts';

async function onBeta() {
  const made = await openFirstBoard();
  const beta = made.board.realCards.find((card) => card.title === 'Beta');
  if (!beta) throw new Error('no card');
  return { ...made, betaId: beta.id };
}

describe('the memory backend', () => {
  it('says when the newest comment directly on a card was posted, and not in a change message', async () => {
    const { backend, board, betaId } = await onBeta();
    const listed = () => backend.cards(board.boardId);
    const seeded = (await listed()).find((card) => card.id === betaId)?.lastCommentAt ?? -1;
    // The seed's comments, if any, are older than any posted now.
    expect(seeded).toBeGreaterThanOrEqual(0);

    const first = await backend.createComment(board.boardId, betaId, text('one'));
    const second = await backend.createComment(board.boardId, betaId, text('two'));
    expect((await listed()).find((card) => card.id === betaId)?.lastCommentAt).toBe(
      second.createdAt,
    );
    // A changed comment does not move it; a deleted one does not count.
    await backend.patchComment(board.boardId, first.id, {
      body: text('one, changed'),
      readAt: first.updatedAt,
    });
    expect((await backend.card(board.boardId, betaId)).lastCommentAt).toBe(second.createdAt);
    await backend.deleteBlock(board.boardId, second.id);
    expect((await backend.card(board.boardId, betaId)).lastCommentAt).toBe(first.createdAt);

    const heard: unknown[] = [];
    const stop = backend.watchContainer(board.boardId, (change) => {
      if (change.kind === 'blocks') heard.push(...change.blocks);
    });
    await backend.patchCard(board.boardId, betaId, { title: 'Beta again' });
    await settle();
    stop();
    expect(heard).toHaveLength(1);
    expect(heard[0]).not.toHaveProperty('lastCommentAt');
  });
});

describe('an open board', () => {
  it('moves a card’s time on when somebody comments, and keeps it through a change of the card', async () => {
    const { board, other, betaId } = await onBeta();
    const posted = await other.createComment(board.boardId, betaId, text('hello'));
    await settle();
    expect(board.card(betaId)?.lastCommentAt).toBe(posted.createdAt);

    // The card comes in a change message, without the time: it keeps it.
    await other.patchCard(board.boardId, betaId, { title: 'Beta, renamed' });
    await settle();
    expect(board.card(betaId)).toMatchObject({
      title: 'Beta, renamed',
      lastCommentAt: posted.createdAt,
    });
  });

  it('reads the card again when a comment it heard of is deleted', async () => {
    const { board, backend, other, betaId } = await onBeta();
    const older = await other.createComment(board.boardId, betaId, text('older'));
    const newer = await other.createComment(board.boardId, betaId, text('newer'));
    await settle();
    expect(board.card(betaId)?.lastCommentAt).toBe(newer.createdAt);

    // The reader, the board's admin, deletes somebody else's comment.
    await backend.deleteBlock(board.boardId, newer.id);
    await settle();
    await vi.waitFor(() => {
      expect(board.card(betaId)?.lastCommentAt).toBe(older.createdAt);
    });
    // Brought back, it counts again.
    await backend.restoreBlock(board.boardId, newer.id);
    await settle();
    expect(board.card(betaId)?.lastCommentAt).toBe(newer.createdAt);
  });

  it('reads the times of all cards again when a comment of no card it knows is deleted', async () => {
    const { board, backend, world, betaId } = await onBeta();
    // Posted while nobody watched: the board never heard of them.
    const silently = (words: string): string => {
      const id = world.newId();
      const at = world.now();
      world.blocks.set(id, {
        id,
        containerId: board.boardId,
        parentId: betaId,
        createdBy: backend.userId,
        modifiedBy: backend.userId,
        type: 'comment',
        title: '',
        fields: {},
        createdAt: at,
        updatedAt: at,
        body: text(words),
      });
      return id;
    };
    const kept = silently('kept');
    const gone = silently('gone');
    const cards = vi.spyOn(backend, 'cards');
    await backend.deleteBlock(board.boardId, gone);
    await settle();
    await vi.waitFor(() => {
      expect(board.card(betaId)?.lastCommentAt).toBe(world.blocks.get(kept)?.createdAt);
    });
    expect(cards).toHaveBeenCalledOnce();
  });

  it('does not take an older time from a reading begun before a comment was heard', async () => {
    const { board, backend, other, betaId } = await onBeta();
    const card = vi.spyOn(backend, 'card');
    // A reading of the card on its way (opening it) while somebody comments.
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const real = backend.card.bind(backend);
    card.mockImplementationOnce(async (boardId, cardId) => {
      const answer = await real(boardId, cardId);
      await held;
      return answer;
    });
    const opening = board.showCard(betaId);
    await settle();
    const posted = await other.createComment(board.boardId, betaId, text('meanwhile'));
    await settle();
    expect(board.card(betaId)?.lastCommentAt).toBe(posted.createdAt);
    release();
    await opening;
    await settle();
    expect(board.card(betaId)?.lastCommentAt).toBe(posted.createdAt);
  });

  it('reads the card again after a deletion even when another comment of it is changed meanwhile', async () => {
    const { board, backend, other, betaId } = await onBeta();
    const older = await other.createComment(board.boardId, betaId, text('older'));
    const newer = await other.createComment(board.boardId, betaId, text('newer'));
    await settle();
    // The reading of the card after the deletion is held until the change below is heard.
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const real = backend.card.bind(backend);
    vi.spyOn(backend, 'card').mockImplementationOnce(async (boardId, cardId) => {
      const answer = await real(boardId, cardId);
      await held;
      return answer;
    });
    await backend.deleteBlock(board.boardId, newer.id);
    await settle();
    // The older one is changed while the card is read again: it does not make the time newer.
    await other.patchComment(board.boardId, older.id, {
      body: text('older, changed'),
      readAt: older.updatedAt,
    });
    await settle();
    release();
    await vi.waitFor(() => {
      expect(board.card(betaId)?.lastCommentAt).toBe(older.createdAt);
    });
  });

  it('reads nothing of the whole board for a comment the reader deleted', async () => {
    const { board, backend, betaId } = await onBeta();
    const card = await board.showCard(betaId);
    await card.addComment(text('mine'));
    await settle();
    const mine = card.comments.at(-1);
    if (!mine) throw new Error('no comment');
    const cards = vi.spyOn(backend, 'cards');
    await card.deleteComment(mine.id);
    await settle();
    expect(cards).not.toHaveBeenCalled();
  });

  it('takes a lower time from reading the board again after the connection was back', async () => {
    const { board, other, world, betaId } = await onBeta();
    const before = board.card(betaId)?.lastCommentAt ?? 0;
    const posted = await other.createComment(board.boardId, betaId, text('soon gone'));
    await settle();
    expect(board.card(betaId)?.lastCommentAt).toBe(posted.createdAt);
    // Deleted while the connection was down: nothing is heard of it, then the board is read again.
    world.blocks.delete(posted.id);
    world.tellContainer(board.boardId, { kind: 'resync' });
    await settle();
    await vi.waitFor(() => {
      expect(board.card(betaId)?.lastCommentAt).toBe(before);
    });
  });

  it('keeps the time of a comment brought back while the card was read again for its deletion', async () => {
    const { board, backend, other, betaId } = await onBeta();
    const posted = await other.createComment(board.boardId, betaId, text('back again'));
    await settle();
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const real = backend.card.bind(backend);
    vi.spyOn(backend, 'card').mockImplementationOnce(async (boardId, cardId) => {
      const answer = await real(boardId, cardId);
      await held;
      return answer;
    });
    await backend.deleteBlock(board.boardId, posted.id);
    await settle();
    await backend.restoreBlock(board.boardId, posted.id);
    await settle();
    release();
    await settle();
    expect(board.card(betaId)?.lastCommentAt).toBe(posted.createdAt);
  });
});
