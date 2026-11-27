/**
 * The order of a board's views under single-transaction server fractional
 * indexing (`writeView`, `moveView`, `duplicateView`), optimistic movement,
 * rollback on refusal, undo/redo, and preservation across copies and templates.
 */
import { describe, expect, it, vi } from 'vitest';

import { createBoardView, type BoardView } from '$lib/logic/wire.ts';
import {
  addViewAtEnd,
  BackendError,
  duplicateView,
  moveView,
  Workspace,
  type BoardStore,
} from '$lib/state/index.ts';
import type { SeedTemplate } from '$lib/state/memory/seed.ts';
import { orderViews } from '$lib/state/order.ts';

import { openFirstBoard, settle, setup, SMALL, type Setup } from './support.ts';

const FIVE: SeedTemplate = {
  ...SMALL,
  views: [
    ...SMALL.views,
    ...['Delta', 'Alpha', 'Gamma', 'Beta'].map((title) => ({
      ...SMALL.views[0],
      id: title.toLowerCase(),
      title,
    })),
  ],
} as SeedTemplate;

const titles = (board: BoardStore): string[] => board.orderedViews.map((view) => view.title);
const idOf = (board: BoardStore, title: string): string =>
  board.views.find((view) => view.title === title)?.id ?? '';
const places = (board: BoardStore): (string | undefined)[] =>
  board.orderedViews.map((view) => view.fields.order);

async function twoReaders(): Promise<Setup & { board: BoardStore; theirs: BoardStore }> {
  const made = await openFirstBoard(setup([FIVE]));
  made.world.members.set(
    made.board.boardId,
    (made.world.members.get(made.board.boardId) ?? []).map((member) =>
      member.userId === made.other.userId ? { ...member, role: 'editor' } : member,
    ),
  );
  const elsewhere = new Workspace({ backend: made.other, locale: () => 'en' });
  await elsewhere.load();
  const theirs = await elsewhere.openBoard(made.board.boardId);
  return { ...made, theirs };
}

describe('reading the order', () => {
  const view = (id: string, title: string, order?: unknown): BoardView => {
    const made = createBoardView();
    return {
      ...made,
      id,
      title,
      fields: { ...made.fields, ...(order === undefined ? {} : { order: order as string }) },
    };
  };

  it('lists the views with a place by it, then those without one by title', () => {
    const ordered = orderViews(
      [
        view('1', 'Zulu'),
        view('2', 'Echo', 'X'),
        view('3', 'Alpha'),
        view('4', 'Bravo', 'V'),
        view('6', 'Lima', 'W'),
        view('5', 'Kilo', 'W'),
        view('7', 'Mike', 'V0'),
        view('8', 'Charlie', 7),
      ],
      'en',
    );
    expect(ordered.map((each) => each.title)).toEqual([
      'Bravo',
      'Kilo',
      'Lima',
      'Echo',
      'Alpha',
      'Charlie',
      'Mike',
      'Zulu',
    ]);
  });
});

describe('a move', () => {
  it('gives every view a fractional place in one backend call; undo puts back the one moved', async () => {
    const { board, backend, workspace } = await openFirstBoard(setup([FIVE]));
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Delta', 'Gamma']);
    const moveSpy = vi.spyOn(backend, 'moveView');
    const patchSpy = vi.spyOn(backend, 'patchView');
    const done = moveView(board, idOf(board, 'Delta'), -1);
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Delta', 'Board', 'Gamma']);
    await done;
    expect(moveSpy).toHaveBeenCalledTimes(1);
    expect(patchSpy).not.toHaveBeenCalled();
    expect(places(board).every((place) => place !== undefined)).toBe(true);
    const stored = (await backend.views(board.boardId)).map((view) => view.fields.order);
    expect(stored.every((place) => place !== undefined)).toBe(true);

    await workspace.undoLast();
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Delta', 'Gamma']);
    expect(moveSpy).toHaveBeenCalledTimes(2);
    const read = orderViews(await backend.views(board.boardId), 'en');
    expect(read.map((view) => view.title)).toEqual(titles(board));
  });

  it('undone after somebody else added a view, leaves that view at the end', async () => {
    const { board, theirs, workspace } = await twoReaders();
    await moveView(board, idOf(board, 'Delta'), -1);
    await settle();
    await addViewAtEnd(theirs, { title: 'Aardvark', fields: createBoardView().fields });
    await settle();
    expect(titles(board).at(-1)).toBe('Aardvark');
    await workspace.undoLast();
    await settle();
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Delta', 'Gamma', 'Aardvark']);
    expect(titles(theirs)).toEqual(titles(board));
  });

  it('does nothing past an end and broadcasts to other readers', async () => {
    const { board, theirs, backend } = await twoReaders();
    await moveView(board, idOf(board, 'Gamma'), -1);
    const moveSpy = vi.spyOn(backend, 'moveView');
    await moveView(board, idOf(board, 'Alpha'), 1);
    expect(moveSpy).toHaveBeenCalledTimes(1);
    expect(titles(board)).toEqual(['Beta', 'Alpha', 'Board', 'Gamma', 'Delta']);
    expect(await moveView(board, idOf(board, 'Beta'), -1)).toBe(false);
    expect(await moveView(board, idOf(board, 'Delta'), 1)).toBe(false);
    expect(moveSpy).toHaveBeenCalledTimes(1);
    await settle();
    expect(titles(theirs)).toEqual(['Beta', 'Alpha', 'Board', 'Gamma', 'Delta']);
  });

  it('keeps concurrent moves by two editors in server transaction order', async () => {
    const { board, theirs } = await twoReaders();
    await moveView(board, idOf(board, 'Gamma'), -1);
    await settle();
    await Promise.all([
      moveView(board, idOf(board, 'Delta'), -1),
      moveView(theirs, idOf(theirs, 'Alpha'), 1),
    ]);
    await settle();
    expect(titles(board)).toEqual(['Beta', 'Alpha', 'Board', 'Delta', 'Gamma']);
    expect(titles(theirs)).toEqual(titles(board));
  });

  it('is not put back by the late word of an earlier move while a later one is under way', async () => {
    const { board, backend, world } = await openFirstBoard(setup([FIVE]));
    const delta = idOf(board, 'Delta');
    await moveView(board, delta, 1);
    await settle();
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Gamma', 'Delta']);
    const afterFirst = board.views.find((view) => view.id === delta);
    if (!afterFirst) throw new Error('no view');
    backend.latency = 20;
    const second = moveView(board, delta, -1);
    const shown = ['Alpha', 'Beta', 'Board', 'Delta', 'Gamma'];
    expect(titles(board)).toEqual(shown);
    // The word of the first move, late: it would put Delta back at the end.
    world.tellContainer(board.boardId, {
      kind: 'blocks',
      containerId: board.boardId,
      blocks: [afterFirst],
    });
    await settle();
    expect(titles(board)).toEqual(shown);
    await second;
    await settle();
    expect(titles(board)).toEqual(shown);
    const stored = orderViews(await backend.views(board.boardId), 'en');
    expect(stored.map((view) => view.title)).toEqual(shown);
  });

  it('takes what was said of a view during a move once the move is answered', async () => {
    const { board, backend, world } = await openFirstBoard(setup([FIVE]));
    const gamma = board.views.find((view) => view.title === 'Gamma');
    if (!gamma) throw new Error('no view');
    backend.latency = 20;
    const moving = moveView(board, idOf(board, 'Delta'), 1);
    world.tellContainer(board.boardId, {
      kind: 'blocks',
      containerId: board.boardId,
      // Renamed by somebody else after the move is stored: the newer version.
      blocks: [{ ...gamma, title: 'Gamma renamed', updatedAt: Date.now() + 60_000 }],
    });
    await settle();
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Gamma', 'Delta']);
    await moving;
    await settle();
    expect(titles(board)).toContain('Gamma renamed');
  });

  it('restores the previous order and reports once when moveView is refused', async () => {
    const { board, backend, theirs, workspace } = await twoReaders();
    vi.spyOn(backend, 'moveView').mockRejectedValueOnce(new BackendError(403, { detail: 'no' }));
    expect(await moveView(board, idOf(board, 'Beta'), 1)).toBe(false);
    await settle();
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Delta', 'Gamma']);
    expect(titles(theirs)).toEqual(titles(board));
    expect(workspace.problems).toHaveLength(1);
  });
});

describe('a new view and copies', () => {
  it('goes at the end in one writeView call, and undo deletes the new view only', async () => {
    const { board, backend, workspace } = await openFirstBoard(setup([FIVE]));
    const writeSpy = vi.spyOn(backend, 'writeView');
    const patchSpy = vi.spyOn(backend, 'patchView');
    const made = await addViewAtEnd(board, {
      title: 'Aardvark',
      fields: createBoardView().fields,
    });
    expect(made?.fields.order).toBeDefined();
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Delta', 'Gamma', 'Aardvark']);
    expect(writeSpy).toHaveBeenCalledTimes(1);
    expect(patchSpy).not.toHaveBeenCalled();

    await addViewAtEnd(board, { title: 'Aardwolf', fields: createBoardView().fields });
    expect(writeSpy).toHaveBeenCalledTimes(2);
    expect(titles(board).slice(-2)).toEqual(['Aardvark', 'Aardwolf']);

    await workspace.undoLast();
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Delta', 'Gamma', 'Aardvark']);
  });

  it('puts a copy of a view right after it in one duplicateView call', async () => {
    const { board, backend } = await openFirstBoard(setup([FIVE]));
    const dupSpy = vi.spyOn(backend, 'duplicateView');
    await duplicateView(board, idOf(board, 'Delta'), 'Copy of Delta');
    expect(dupSpy).toHaveBeenCalledTimes(1);
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Board', 'Delta', 'Copy of Delta', 'Gamma']);
    expect(board.views.every((view) => view.fields.order !== undefined)).toBe(true);
    await duplicateView(board, idOf(board, 'Alpha'), 'Zebra');
    expect(titles(board).slice(0, 2)).toEqual(['Alpha', 'Zebra']);
  });

  it('reports refusal once when writeView or duplicateView fails', async () => {
    const { board, backend, workspace } = await openFirstBoard(setup([FIVE]));
    vi.spyOn(backend, 'writeView').mockRejectedValueOnce(new BackendError(403, { detail: 'no' }));
    vi.spyOn(backend, 'duplicateView').mockRejectedValueOnce(
      new BackendError(403, { detail: 'no' }),
    );
    expect(
      await addViewAtEnd(board, { title: 'Aardvark', fields: createBoardView().fields }),
    ).toBeUndefined();
    expect(workspace.problems).toHaveLength(1);
    expect(await duplicateView(board, idOf(board, 'Alpha'), 'Zebra')).toBeUndefined();
    expect(workspace.problems).toHaveLength(2);
  });

  it('keeps the order in a copy of the board, and through a template', async () => {
    const { board, backend, workspace, world } = await openFirstBoard(setup([FIVE]));
    await moveView(board, idOf(board, 'Gamma'), -1);
    await moveView(board, idOf(board, 'Gamma'), -1);
    await moveView(board, idOf(board, 'Board'), 1);
    const order = titles(board);
    expect(order).toEqual(['Alpha', 'Beta', 'Gamma', 'Delta', 'Board']);
    const read = async (boardId: string): Promise<string[]> =>
      orderViews(await backend.views(boardId), 'en').map((view) => view.title);
    const copy = await backend.duplicateContainer(board.boardId, false, world.workspace.id);
    expect(await read(copy.id)).toEqual(order);
    const template = await backend.duplicateContainer(board.boardId, true, world.workspace.id);
    const fromTemplate = await backend.duplicateContainer(template.id, false, world.workspace.id);
    expect(await read(fromTemplate.id)).toEqual(order);
    expect(workspace.problems).toEqual([]);
  });
});
