/** The reader's stars in the workspace (`WorkspaceFavorites`), on the backend in memory. */
import { describe, expect, it, vi } from 'vitest';

import { BackendError, MAX_FAVORITES, workspaceFavorites } from '$lib/state/index.ts';

import { openFirstBoard, settle } from './support.ts';

describe('the reader’s stars', () => {
  it('puts a star on a board and on a card, in their order, and takes one off', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const card = await backend.createCard(board.boardId, { title: 'Mine' });
    const stars = workspaceFavorites(workspace);
    expect(stars.has(board.boardId)).toBeUndefined();
    await stars.read();
    expect(stars.has(board.boardId)).toBe(false);

    await stars.toggle(board.boardId);
    await stars.toggle(card.id);
    expect(stars.list.map((one) => [one.kind, one.targetId])).toEqual([
      ['container', board.boardId],
      ['card', card.id],
    ]);
    expect(stars.list[1]?.title).toBe('Mine');

    await stars.reorder([card.id]);
    expect(stars.list.map((one) => one.targetId)).toEqual([card.id, board.boardId]);
    const workspaceId = workspace.info?.id ?? '';
    expect((await backend.favorites.list(workspaceId)).map((one) => one.targetId)).toEqual([
      card.id,
      board.boardId,
    ]);

    await stars.toggle(card.id);
    expect(stars.has(card.id)).toBe(false);
    expect(stars.list.map((one) => one.targetId)).toEqual([board.boardId]);
  });

  it('is one list for every screen of the workspace, read once', async () => {
    const { workspace, backend } = await openFirstBoard();
    const reads = vi.spyOn(backend.favorites, 'list');
    await Promise.all([workspaceFavorites(workspace).read(), workspaceFavorites(workspace).read()]);
    expect(workspaceFavorites(workspace)).toBe(workspaceFavorites(workspace));
    expect(reads).toHaveBeenCalledOnce();
  });

  it('goes back and reports when the server refuses a star', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const stars = workspaceFavorites(workspace);
    await stars.read();
    backend.failNext('favorites', new BackendError(500, { detail: 'down' }));
    await stars.toggle(board.boardId);
    expect(stars.has(board.boardId)).toBe(false);
    expect(workspace.problems).toHaveLength(1);
  });

  it('takes a star off that is off already, as another tab took it, without a word', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const stars = workspaceFavorites(workspace);
    await stars.read();
    await stars.toggle(board.boardId);
    // Taken off in another tab just before: the server has none to take off.
    const unstar = backend.favorites.unstar.bind(backend.favorites);
    vi.spyOn(backend.favorites, 'unstar').mockImplementationOnce(async (where, targetId) => {
      await unstar(where, targetId);
      return unstar(where, targetId);
    });
    await stars.toggle(board.boardId);
    expect([stars.has(board.boardId), stars.list, workspace.problems]).toEqual([false, [], []]);
  });

  it('refuses what is nothing, and a star past the most a workspace keeps', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const workspaceId = workspace.info?.id ?? '';
    await expect(backend.favorites.star(workspaceId, 'no-such-thing')).rejects.toMatchObject({
      status: 404,
    });
    const cards = await Promise.all(
      Array.from({ length: MAX_FAVORITES }, (_, at) =>
        backend.createCard(board.boardId, { title: `card ${String(at)}` }),
      ),
    );
    for (const card of cards) await backend.favorites.star(workspaceId, card.id);
    await expect(backend.favorites.star(workspaceId, board.boardId)).rejects.toMatchObject({
      status: 400,
      reason: 'too_many_favorites',
    });
  });

  it('is of the workspace entered, and hears a change of its stars before it read them', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const stars = workspaceFavorites(workspace);
    await stars.read();
    await stars.toggle(board.boardId);
    const first = workspace.info;
    const firstScope = stars.scope;
    expect(firstScope).toBeDefined();

    // Another workspace entered: none of the first's stars, until its own are read.
    const list = vi.spyOn(backend.favorites, 'list').mockResolvedValueOnce([]);
    workspace.info = { id: 'other-workspace', title: 'Other' };
    expect(stars.scope).not.toBe(firstScope);
    expect([stars.list, stars.has(board.boardId)]).toEqual([[], undefined]);
    await stars.read();
    expect(list).toHaveBeenLastCalledWith('other-workspace');
    expect(stars.has(board.boardId)).toBe(false);
    list.mockRestore();

    // Back in the first, whose list is not held: a change of its stars reads it.
    workspace.info = first;
    expect(stars.scope).toBe(firstScope);
    await backend.favorites.unstar(first?.id ?? '', board.boardId);
    await settle();
    expect(stars.has(board.boardId)).toBe(false);
    expect(stars.list).toEqual([]);
  });

  it('reads the list again when the stars change on another device', async () => {
    const { workspace, board, backend } = await openFirstBoard();
    const stars = workspaceFavorites(workspace);
    await stars.read();
    const workspaceId = workspace.info?.id ?? '';
    await backend.favorites.star(workspaceId, board.boardId);
    await settle();
    expect(stars.has(board.boardId)).toBe(true);
  });
});
