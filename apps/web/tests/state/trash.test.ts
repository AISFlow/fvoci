/**
 * The trash: the memory backend lists and brings back as the server does,
 * the server backend asks for its pages, and the store of a list keeps its
 * pages and its rows right as things come back.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { createBoardView } from '$lib/logic/wire.ts';
import { BackendError, containerTrash, cardTrash, type Change } from '$lib/state/index.ts';

import { openFirstBoard, serverBackend, settle, setup } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

afterEach(() => {
  vi.unstubAllGlobals();
});

async function firstBoard() {
  const made = setup();
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  if (!board || !workspace) throw new Error('no board');
  const cards = await made.backend.cards(board.id);
  const id = (title: string): string => cards.find((card) => card.title === title)?.id ?? '';
  const otherId = (await made.other.me()).id;
  return { ...made, workspaceId: workspace.id, boardId: board.id, id, otherId };
}

describe('the cards in the trash, in memory', () => {
  it('lists the cards deleted by themselves, the last first, with who and when', async () => {
    const { backend, boardId, id } = await firstBoard();
    const meId = (await backend.me()).id;
    await backend.batchDeleteCards(boardId, [id('Alpha')]);
    await backend.batchDeleteCards(boardId, [id('Beta')]);
    const listed = await backend.deletedCards(boardId);
    expect(listed.map((item) => item.title)).toEqual(['Beta', 'Alpha']);
    expect(listed.every((item) => item.deletedBy === meId)).toBe(true);
    expect(listed[0]?.deletedAt).toBeGreaterThan(listed[1]?.deletedAt ?? Infinity);
    // A page of one, and the second page.
    expect(
      (await backend.deletedCards(boardId, { page: 1, perPage: 1 })).map((item) => item.title),
    ).toEqual(['Alpha']);
    // Pages of 1 to 1000, as the server cuts them: anything else is refused.
    expect(await backend.deletedCards(boardId, { page: 0, perPage: 1000 })).toHaveLength(2);
    for (const paging of [
      { page: -1, perPage: 1 },
      { page: 2_147_483_648, perPage: 1 },
      { page: 0, perPage: 0 },
      { page: 0, perPage: -1 },
      { page: 0, perPage: 1001 },
    ]) {
      const refused = await rejection(backend.deletedCards(boardId, paging));
      expect(refused.status, JSON.stringify(paging)).toBe(400);
    }
    // A comment deleted is no card of the trash.
    const [comment] = await backend.comments(boardId, id('Gamma'));
    if (comment) await backend.deleteComment(boardId, id('Gamma'), comment.id);
    expect(await backend.deletedCards(boardId)).toHaveLength(2);
    await backend.batchRestoreCards(boardId, [id('Alpha')]);
    expect((await backend.deletedCards(boardId)).map((item) => item.title)).toEqual(['Beta']);
  });

  it('is for the editors of the board: a commenter or viewer is refused', async () => {
    const { backend, other, boardId, otherId } = await firstBoard();
    await backend.setMemberRole(boardId, otherId, 'commenter');
    expect((await rejection(other.deletedCards(boardId))).status).toBe(403);
    await backend.setMemberRole(boardId, otherId, 'editor');
    expect(await other.deletedCards(boardId)).toEqual([]);
  });

  it('is brought back by editors only, and a private board is not there for a non-member', async () => {
    const { backend, other, world, boardId, otherId, id } = await firstBoard();
    await backend.batchDeleteCards(boardId, [id('Alpha')]);
    await backend.setMemberRole(boardId, otherId, 'commenter');
    expect((await rejection(other.batchRestoreCards(boardId, [id('Alpha')]))).status).toBe(403);
    const strangerId = [...world.users.keys()].find(
      (userId) => !(world.members.get(boardId) ?? []).some((member) => member.userId === userId),
    );
    const stranger = backend.as(strangerId ?? '');
    expect((await rejection(stranger.deletedCards(boardId))).status).toBe(404);
  });
});

describe('the boards in the trash, in memory', () => {
  it('are listed for who was their admin, and brought back by them only', async () => {
    const { backend, other, world, workspaceId, boardId, otherId } = await firstBoard();
    await backend.setMemberRole(boardId, otherId, 'editor');
    await backend.deleteContainer(boardId);
    expect((await backend.deletedContainers(workspaceId)).map((item) => item.id)).toEqual([
      boardId,
    ]);
    expect(await other.deletedContainers(workspaceId)).toEqual([]);
    expect((await rejection(other.restoreContainer(boardId))).status).toBe(403);
    const stranger = backend.as(
      [...world.users.keys()].find(
        (userId) =>
          !(world.deletedContainers.get(boardId)?.members ?? []).some(
            (member) => member.userId === userId,
          ),
      ) ?? '',
    );
    expect((await rejection(stranger.restoreContainer(boardId))).status).toBe(404);
    await backend.restoreContainer(boardId);
    expect(await backend.deletedContainers(workspaceId)).toEqual([]);
  });

  it('come back where each person had filed them, hidden or not, and the sidebar is told', async () => {
    const { backend, workspaceId, boardId } = await firstBoard();
    const projects = await backend.createCategory(workspaceId, { name: 'Projects' });
    await backend.moveContainerToCategory(workspaceId, boardId, projects.id, 0);
    const second = await backend.createBoard(
      workspaceId,
      { title: 'Second' },
      { title: 'View', fields: createBoardView().fields },
    );
    await backend.moveContainerToCategory(workspaceId, second.board.id, projects.id, 1);
    await backend.setContainerHidden(workspaceId, projects.id, boardId, true);
    const before = (await backend.categories(workspaceId)).find((one) => one.id === projects.id);
    expect(before?.containerIds).toEqual([boardId, second.board.id]);

    await backend.deleteContainer(boardId);
    const changes: Change[] = [];
    backend.watchSelf((change) => changes.push(change));
    await backend.restoreContainer(boardId);
    await settle();
    const after = (await backend.categories(workspaceId)).find((one) => one.id === projects.id);
    expect(after?.containerIds).toEqual([boardId, second.board.id]);
    expect(after?.hiddenContainerIds).toEqual([boardId]);
    const told = changes.find((change) => change.kind === 'categories');
    expect(
      told?.kind === 'categories' &&
        told.categories.find((one) => one.id === projects.id)?.containerIds,
    ).toEqual([boardId, second.board.id]);
  });

  it('go to the default category when theirs went meanwhile', async () => {
    const { backend, workspaceId, boardId } = await firstBoard();
    const projects = await backend.createCategory(workspaceId, { name: 'Projects' });
    await backend.moveContainerToCategory(workspaceId, boardId, projects.id, 0);
    await backend.deleteContainer(boardId);
    await backend.deleteCategory(workspaceId, projects.id);
    await backend.restoreContainer(boardId);
    const system = (await backend.categories(workspaceId)).find((one) => one.type === 'system');
    expect(system?.containerIds).toContain(boardId);
  });
});

describe('the list of a trash', () => {
  it('reads a page, reads one more, and takes out what comes back', async () => {
    const made = await openFirstBoard();
    const { board, workspace, backend } = made;
    const cards = await backend.cards(board.boardId);
    for (const card of cards) await backend.batchDeleteCards(board.boardId, [card.id]);
    const trash = cardTrash(board);
    const spy = vi.spyOn(backend, 'deletedCards');
    await trash.read();
    expect(trash.status).toBe('ready');
    expect(trash.items).toHaveLength(cards.length);
    expect(trash.hasMore).toBe(false);
    const [first] = trash.items;
    expect(await trash.restore(first?.id ?? '')).toBe(true);
    expect(trash.items.map((item) => item.id)).not.toContain(first?.id);
    await settle();
    expect(board.cards.map((card) => card.id)).toContain(first?.id);
    // One that is not in the trash any more (gone meanwhile) leaves the list and is told.
    const [second] = trash.items;
    await backend.batchRestoreCards(board.boardId, [second?.id ?? '']);
    expect(await trash.restore(second?.id ?? '')).toBe(false);
    expect(trash.items.map((item) => item.id)).not.toContain(second?.id);
    expect(workspace.problems.length).toBeGreaterThan(0);
    expect(spy).toHaveBeenCalledWith(board.boardId, { page: 0, perPage: 50 });
  });

  it('reads again the whole list one page longer for more, so that nothing is skipped', async () => {
    const made = await openFirstBoard();
    const { workspace } = made;
    const items = Array.from({ length: 75 }, (_, at) => ({
      id: `b${at}`,
      title: `Board ${at}`,
      deletedAt: 1000 - at,
      deletedBy: '',
    }));
    const asked: unknown[] = [];
    vi.spyOn(workspace.backend, 'deletedContainers').mockImplementation((_workspaceId, paging) => {
      asked.push(paging);
      return Promise.resolve(items.slice(0, paging?.perPage ?? 100));
    });
    const trash = containerTrash(workspace);
    await trash.read();
    expect(trash.items).toHaveLength(50);
    expect(trash.hasMore).toBe(true);
    await trash.more();
    expect(trash.items).toHaveLength(75);
    expect(trash.hasMore).toBe(false);
    expect(asked).toEqual([
      { page: 0, perPage: 50 },
      { page: 0, perPage: 100 },
    ]);
  });

  it('says a list that could not be read failed, once: in the list, not also on the page', async () => {
    const { workspace } = await openFirstBoard();
    vi.spyOn(workspace.backend, 'deletedContainers').mockRejectedValue(new BackendError(500));
    const trash = containerTrash(workspace);
    await trash.read();
    expect(trash.status).toBe('failed');
    expect(trash.items).toEqual([]);
    expect(workspace.problems).toEqual([]);
  });

  it('does not show again one brought back while a reading begun before was on its way', async () => {
    const { workspace } = await openFirstBoard();
    const item = { id: 'b1', title: 'One', deletedAt: 1, deletedBy: '' };
    let answer: (items: (typeof item)[]) => void = () => undefined;
    vi.spyOn(workspace.backend, 'deletedContainers')
      .mockResolvedValueOnce([item])
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            answer = resolve;
          }),
      );
    vi.spyOn(workspace, 'restoreContainer').mockResolvedValue();
    const trash = containerTrash(workspace);
    await trash.read();
    const reading = trash.read();
    expect(await trash.restore('b1')).toBe(true);
    answer([item]);
    await reading;
    expect(trash.items).toEqual([]);
  });
});

describe('the trash, on the server', () => {
  it('asks for the pages of cards and boards, and brings a board back', async () => {
    const sent: string[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      sent.push(`${init.method ?? 'GET'} ${url}`);
      const item = { id: 'x', title: 'X', deletedAt: 5, deletedBy: 'u' };
      return Promise.resolve(
        new Response(JSON.stringify([item]), { headers: { 'Content-Type': 'application/json' } }),
      );
    });
    const backend = serverBackend('');
    expect(await backend.deletedCards('b1', { page: 2, perPage: 50 })).toEqual([
      { id: 'x', title: 'X', deletedAt: 5, deletedBy: 'u' },
    ]);
    await backend.deletedContainers('t1');
    expect(sent).toEqual([
      'GET /api/v1/w/w1/boards/b1/trash/cards?page=2&perPage=50',
      'GET /api/v1/w/t1/trash/containers',
    ]);
  });
});

describe('a trash read again after a failure', () => {
  it('keeps the pages asked for: a failed "more" is tried again as more', async () => {
    const { workspace } = await openFirstBoard();
    const items = Array.from({ length: 60 }, (_, at) => ({
      id: `b${at}`,
      title: `Board ${at}`,
      deletedAt: 1000 - at,
      deletedBy: '',
    }));
    const asked: unknown[] = [];
    vi.spyOn(workspace.backend, 'deletedContainers').mockImplementation((_workspaceId, paging) => {
      asked.push(paging);
      if (asked.length === 2) return Promise.reject(new BackendError(500));
      return Promise.resolve(items.slice(0, paging?.perPage ?? 100));
    });
    const trash = containerTrash(workspace);
    await trash.read();
    await trash.more();
    expect(trash.status).toBe('failed');
    await trash.retry();
    expect(trash.items).toHaveLength(60);
    expect(asked.at(-1)).toEqual({ page: 0, perPage: 100 });
  });
});
