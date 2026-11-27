/** The order of a board's card properties: moved, stored by its own request, followed elsewhere, undone. */
import { describe, expect, it, vi } from 'vitest';

import { movedProperty, withPropertyOrder } from '$lib/logic/property-order.ts';
import { createBoard } from '$lib/logic/wire.ts';
import { BackendError, Workspace } from '$lib/state/index.ts';

import { openFirstBoard, OWNER, serverBackend, settle, STATUS } from './support.ts';

const ids = (board: { cardProperties: { id: string }[] } | undefined): string[] =>
  board?.cardProperties.map((template) => template.id) ?? [];

describe('an order of properties', () => {
  it('moves one a place up or down, and not past an end', () => {
    expect(movedProperty(['a', 'b', 'c'], 'c', -1)).toEqual(['a', 'c', 'b']);
    expect(movedProperty(['a', 'b', 'c'], 'a', 1)).toEqual(['b', 'a', 'c']);
    expect(movedProperty(['a', 'b', 'c'], 'a', -1)).toEqual(['a', 'b', 'c']);
    expect(movedProperty(['a', 'b', 'c'], 'c', 1)).toEqual(['a', 'b', 'c']);
    expect(movedProperty(['a', 'b'], 'x', 1)).toEqual(['a', 'b']);
    // Among those a list shows: past the next of them, over one it hides.
    expect(movedProperty(['a', 'h', 'b', 'c'], 'a', 1, ['a', 'b', 'c'])).toEqual([
      'h',
      'b',
      'a',
      'c',
    ]);
    expect(movedProperty(['a', 'h', 'b'], 'b', -1, ['a', 'b'])).toEqual(['b', 'a', 'h']);
    expect(movedProperty(['h', 'a', 'b'], 'a', -1, ['a', 'b'])).toEqual(['h', 'a', 'b']);
  });

  it('puts a board in it, keeping one it does not name after the others', () => {
    const board = createBoard();
    board.cardProperties = ['a', 'b', 'c'].map((id) => ({
      id,
      name: id,
      type: 'text',
      options: [],
    }));
    expect(ids(withPropertyOrder(board, ['c', 'a', 'b']))).toEqual(['c', 'a', 'b']);
    expect(ids(withPropertyOrder(board, ['c', 'a']))).toEqual(['c', 'a', 'b']);
    expect(ids(board)).toEqual(['a', 'b', 'c']);
  });
});

describe('the memory backend', () => {
  it('takes an order naming every property once, and refuses any other with nothing changed', async () => {
    const { backend, board } = await openFirstBoard();
    const refused = async (order: string[]) => {
      const error = await backend
        .reorderCardProperties(board.boardId, order)
        .catch((e: unknown) => e);
      return error instanceof BackendError ? error.status : 'taken';
    };
    expect(await refused([OWNER])).toBe(400);
    expect(await refused([OWNER, OWNER])).toBe(400);
    expect(await refused([OWNER, STATUS, 'nothing'])).toBe(400);
    expect(ids(await backend.container(board.boardId))).toEqual([STATUS, OWNER]);
    expect(await backend.reorderCardProperties(board.boardId, [OWNER, STATUS])).toEqual([
      OWNER,
      STATUS,
    ]);
    expect(ids(await backend.container(board.boardId))).toEqual([OWNER, STATUS]);
    // Only who may change the properties: an editor or an admin.
    expect(await refused([STATUS, OWNER])).toBe('taken');
    const commenter = backend.as(
      (backend.world.members.get(board.boardId) ?? []).find((member) => member.role === 'commenter')
        ?.userId ?? '',
    );
    const asCommenter = await commenter
      .reorderCardProperties(board.boardId, [OWNER, STATUS])
      .catch((e: unknown) => e);
    expect(asCommenter instanceof BackendError && asCommenter.status).toBe(403);
  });
});

describe('an open board', () => {
  it('shows a move at once, sends it by itself, and undo moves it back', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const reorder = vi.spyOn(backend, 'reorderCardProperties');
    const patch = vi.spyOn(backend, 'patchContainer');
    const done = board.moveProperty(OWNER, -1);
    await Promise.resolve();
    await Promise.resolve();
    expect(ids(board.board)).toEqual([OWNER, STATUS]);
    await done;
    expect(reorder).toHaveBeenCalledWith(board.boardId, [OWNER, STATUS]);
    expect(patch).not.toHaveBeenCalled();
    expect(ids(await backend.container(board.boardId))).toEqual([OWNER, STATUS]);

    await workspace.undoLast();
    expect(ids(board.board)).toEqual([STATUS, OWNER]);
    expect(ids(await backend.container(board.boardId))).toEqual([STATUS, OWNER]);
    // At an end it does not move, and its undo moves nothing back.
    await board.moveProperty(STATUS, -1);
    expect(reorder).toHaveBeenCalledTimes(2);
    await workspace.undoLast();
    expect(reorder).toHaveBeenCalledTimes(2);
    expect(ids(board.board)).toEqual([STATUS, OWNER]);
  });

  it('undoes a move after somebody else added a property', async () => {
    const { board, backend, other, workspace } = await openFirstBoard();
    await board.moveProperty(OWNER, -1);
    const stored = await other.container(board.boardId);
    await other.patchContainer(board.boardId, {
      updatedCardProperties: [
        ...stored.cardProperties,
        { id: 'later', name: 'Later', type: 'text', options: [] },
      ],
    });
    await settle();
    await workspace.undoLast();
    expect(workspace.problems).toEqual([]);
    expect(ids(await backend.container(board.boardId))).toEqual([STATUS, OWNER, 'later']);
  });

  it('sends two quick moves one after the other, each on the order the one before left', async () => {
    const { board, backend } = await openFirstBoard();
    backend.latency = 10;
    const first = board.moveProperty(OWNER, -1);
    const second = board.moveProperty(OWNER, 1);
    await Promise.all([first, second]);
    expect(ids(await backend.container(board.boardId))).toEqual([STATUS, OWNER]);
    expect(ids(board.board)).toEqual([STATUS, OWNER]);
  });

  it('shows the board as the server has it when a move is refused, and tells the reader', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    backend.failNext('reorderCardProperties', new BackendError(403));
    await board.moveProperty(OWNER, -1);
    expect(ids(board.board)).toEqual([STATUS, OWNER]);
    expect(workspace.problems.at(-1)).toMatchObject({ action: 'save', error: { status: 403 } });
  });

  it('follows a move made by somebody else', async () => {
    const made = await openFirstBoard();
    // Somebody who may change the properties: an editor.
    made.world.members.set(
      made.board.boardId,
      (made.world.members.get(made.board.boardId) ?? []).map((member) =>
        member.userId === made.other.userId ? { ...member, role: 'editor' } : member,
      ),
    );
    const elsewhere = new Workspace({ backend: made.other, locale: () => 'en' });
    await elsewhere.load();
    const theirs = await elsewhere.openBoard(made.board.boardId);
    await theirs.moveProperty(OWNER, -1);
    await settle();
    expect(ids(made.board.board)).toEqual([OWNER, STATUS]);
  });
});

describe('the server backend', () => {
  it('puts the order to the board’s property order', async () => {
    const sent: { url: string; method: string; body: unknown }[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      sent.push({
        url,
        method: init.method ?? 'GET',
        body: JSON.parse(init.body as string) as unknown,
      });
      return Promise.resolve(
        new Response(JSON.stringify(['b', 'a']), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    try {
      expect(await serverBackend('').reorderCardProperties('x', ['b', 'a'])).toEqual(['b', 'a']);
    } finally {
      vi.unstubAllGlobals();
    }
    expect(sent).toEqual([
      { url: '/api/v1/w/w1/boards/x/property-order', method: 'PUT', body: ['b', 'a'] },
    ]);
  });
});
