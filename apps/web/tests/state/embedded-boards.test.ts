/**
 * The boards the board views of bodies show (`embeddedBoards`): one store
 * for each board however many views of it are shown, apart from the board
 * open and the boards the workspace keeps; changed live as an open board is;
 * kept a while after the last view of it goes, then closed. And what a
 * reader chooses a view from.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  BackendError,
  boardsToShow,
  embeddedBoards,
  RELEASE_AFTER_MS,
  viewsOfBoard,
} from '$lib/state/index.ts';

import { loadTemplates, openFirstBoard, settle, setup } from './support.ts';

afterEach(() => {
  vi.useRealTimers();
});

/** Lets the board read, and waits for it. */
async function loaded(store: { status: string }): Promise<void> {
  for (let turn = 0; turn < 20 && store.status !== 'ready'; turn += 1) await settle();
}

describe('the boards of the board views shown', () => {
  it('is one store for each board, read once, and none of the board open', async () => {
    const made = await openFirstBoard(setup(await loadTemplates()));
    const { workspace, board: open } = made;
    const boards = embeddedBoards(workspace);
    const first = boards.hold(open.boardId);
    const second = boards.hold(open.boardId);
    expect(second.store).toBe(first.store);
    expect(first.store).not.toBe(open);
    await loaded(first.store);
    expect(first.store.status).toBe('ready');
    expect(first.store.realCards.map((card) => card.id).sort()).toEqual(
      open.realCards.map((card) => card.id).sort(),
    );
    // The board open stays the board open, and is not closed with the views.
    expect(workspace.board).toBe(open);
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    first.release();
    second.release();
    vi.advanceTimersByTime(RELEASE_AFTER_MS);
    expect(first.store.isClosed()).toBe(true);
    expect(open.isClosed()).toBe(false);
    expect(boards.boardIds).toEqual([]);
  });

  it('is kept a while after the last view of it goes, for a body drawn again', async () => {
    const { workspace } = await openFirstBoard(setup(await loadTemplates()));
    const boardId = boardsToShow(workspace)[1]?.id ?? '';
    const boards = embeddedBoards(workspace);
    const held = boards.hold(boardId);
    await loaded(held.store);
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    held.release();
    held.release();
    vi.advanceTimersByTime(RELEASE_AFTER_MS - 1);
    const again = boards.hold(boardId);
    expect(again.store).toBe(held.store);
    vi.advanceTimersByTime(RELEASE_AFTER_MS);
    expect(held.store.isClosed()).toBe(false);
    again.release();
    vi.advanceTimersByTime(RELEASE_AFTER_MS);
    expect(held.store.isClosed()).toBe(true);
    // Held after it was closed, a board is read anew.
    vi.useRealTimers();
    const later = boards.hold(boardId);
    expect(later.store).not.toBe(held.store);
    await loaded(later.store);
    expect(later.store.status).toBe('ready');
    later.release();
  });

  it('is read again when held again after its reading failed, unless the board is gone', async () => {
    const made = await openFirstBoard(setup(await loadTemplates()));
    const [first, second] = boardsToShow(made.workspace);
    if (!first || !second) throw new Error('the seed has no two boards');
    const boards = embeddedBoards(made.workspace);
    vi.spyOn(made.workspace, 'report').mockImplementation(() => undefined);
    made.backend.failNext('cards', new BackendError(500));
    const failed = boards.hold(first.id);
    await settle();
    expect(failed.store.status).toBe('failed');
    const again = boards.hold(first.id);
    expect(again.store).toBe(failed.store);
    await loaded(again.store);
    expect(again.store.status).toBe('ready');

    made.backend.failNext('cards', new BackendError(404));
    const gone = boards.hold(second.id);
    await settle();
    expect([gone.store.status, gone.store.missing]).toEqual(['failed', true]);
    const load = vi.spyOn(gone.store, 'load');
    boards.hold(second.id);
    expect(load).not.toHaveBeenCalled();
  });

  it('is not taken for gone by a refusal explained once it was read again', async () => {
    const made = await openFirstBoard(setup(await loadTemplates()));
    const [first] = boardsToShow(made.workspace);
    if (!first) throw new Error('the seed has no board');
    const boards = embeddedBoards(made.workspace);
    vi.spyOn(made.workspace, 'report').mockImplementation(() => undefined);
    let release = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const read = made.backend.container.bind(made.backend);
    let reads = 0;
    // The second reading of the board is the one that explains the refusal: gone, it says, late.
    vi.spyOn(made.backend, 'container').mockImplementation(async (...args) => {
      reads += 1;
      if (reads !== 2) return read(...args);
      await gate;
      throw new BackendError(404);
    });
    made.backend.failNext('cards', new BackendError(403));
    const refused = boards.hold(first.id);
    await settle();
    expect(refused.store.status).toBe('failed');
    const again = boards.hold(first.id);
    await loaded(again.store);
    expect(again.store.status).toBe('ready');
    release();
    await settle();
    expect([again.store.status, again.store.missing]).toEqual(['ready', false]);
  });

  it('shows what other people change on the board, by itself', async () => {
    const { workspace, other } = await openFirstBoard(setup(await loadTemplates()));
    const boardId = boardsToShow(workspace)[1]?.id ?? '';
    const held = embeddedBoards(workspace).hold(boardId);
    await loaded(held.store);
    const card = held.store.realCards[0];
    if (!card) throw new Error('no card');
    await other.patchCard(boardId, card.id, { title: 'Changed by someone else' });
    await settle();
    expect(held.store.card(card.id)?.title).toBe('Changed by someone else');
    held.release();
  });
});

describe('what a view to show is chosen from', () => {
  it('is the boards of the workspace, and the views of the board chosen in its order', async () => {
    const { workspace, board } = await openFirstBoard(setup(await loadTemplates()));
    const shown = boardsToShow(workspace);
    expect(shown.length).toBeGreaterThan(1);
    expect(shown.every((each) => (each.kind ?? 'board') === 'board' && !each.isTemplate)).toBe(
      true,
    );
    const views = await viewsOfBoard(workspace, board.boardId);
    expect(views.map((view) => view.id)).toEqual(board.orderedViews.map((view) => view.id));
  });
});
