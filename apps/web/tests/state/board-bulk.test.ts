/**
 * One value on many cards at once (state/board-bulk.ts): shown at once, one
 * request, all or none, taken back on refusal, and one step of the history
 * that gives each card its own value back.
 */
import { describe, expect, it, vi } from 'vitest';

import { BackendError, CARD_BATCH_LIMIT, setCardsProperty } from '$lib/state/index.ts';
import { clearCardsProperty } from '$lib/state/board-bulk.ts';
import type { Backend, BoardStore } from '$lib/state/index.ts';

import { OWNER, openFirstBoard, settle, STATUS } from './support.ts';

/** The small board open, its cards Alpha, Beta and Gamma as `c1`, `c2`, `c3`; the backend as the store sees it. */
async function opened() {
  const made = await openFirstBoard();
  const idOf = (title: string): string => {
    const card = made.board.cards.find((one) => one.title === title);
    if (!card) throw new Error(`no card ${title}`);
    return card.id;
  };
  const api: Backend = made.backend;
  return { ...made, api, c1: idOf('Alpha'), c2: idOf('Beta'), c3: idOf('Gamma') };
}

function held() {
  let release = (): void => undefined;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

const statusOf = (board: BoardStore, cardId: string): unknown =>
  board.card(cardId)?.fields.properties[STATUS];

describe('setting one value on many cards', () => {
  it('shows the value at once and stores it with one request', async () => {
    const { board, backend, api, workspace, c1, c2, c3 } = await opened();
    const gate = held();
    const batch = api.batchPatchCards.bind(api);
    const sent = vi.spyOn(api, 'batchPatchCards').mockImplementation(async (...args) => {
      await gate.promise;
      return batch(...args);
    });
    const patch = vi.spyOn(backend, 'patchCard');
    const writing = setCardsProperty(board, [c1, c2, c3], STATUS, 'done');
    expect([c1, c2, c3].map((id) => statusOf(board, id))).toEqual(['done', 'done', 'done']);
    gate.release();
    await writing;
    expect(sent).toHaveBeenCalledTimes(1);
    expect(sent.mock.calls[0]?.slice(1, 3)).toEqual([
      [c1, c2, c3],
      { updatedProperties: { [STATUS]: 'done' } },
    ]);
    // The first time the change tells whom it names.
    expect(sent.mock.calls[0]?.[3]).toBeUndefined();
    expect(patch).not.toHaveBeenCalled();
    const stored = await backend.cards(board.boardId);
    expect(stored.filter((card) => card.fields.properties[STATUS] === 'done')).toHaveLength(3);
    expect(workspace.undo.canUndo).toBe(true);
  });

  it('leaves out the cards that have the value already, and does nothing when all have it', async () => {
    const { board, api, workspace, c1, c3 } = await opened();
    const sent = vi.spyOn(api, 'batchPatchCards');
    await setCardsProperty(board, [c1, c3], STATUS, 'doing');
    expect(sent.mock.calls[0]?.[1]).toEqual([c1]);
    await setCardsProperty(board, [c1, c3], STATUS, 'doing');
    expect(sent).toHaveBeenCalledTimes(1);
    await workspace.undoLast();
    expect(statusOf(board, c1)).toBe('todo');
    expect(workspace.undo.canUndo).toBe(false);
  });

  it('takes every card back when the server refuses, and leaves no step', async () => {
    const { board, backend, workspace, c1, c2, c3 } = await opened();
    backend.failNext('batchPatchCards', new BackendError(403));
    const report = vi.spyOn(workspace, 'report');
    await setCardsProperty(board, [c1, c2, c3], STATUS, 'done');
    expect([c1, c2, c3].map((id) => statusOf(board, id))).toEqual(['todo', 'todo', 'doing']);
    expect(report).toHaveBeenCalledWith('save', expect.any(BackendError));
    expect(workspace.undo.canUndo).toBe(false);
    const stored = await backend.cards(board.boardId);
    expect(stored.find((card) => card.id === c3)?.fields.properties[STATUS]).toBe('doing');
  });

  it('keeps a value changed over the refused one while it waited', async () => {
    const { board, api, c1, c2 } = await opened();
    const gate = held();
    vi.spyOn(api, 'batchPatchCards').mockImplementationOnce(async () => {
      await gate.promise;
      throw new BackendError(409);
    });
    const writing = setCardsProperty(board, [c1, c2], STATUS, 'done');
    await board.setCardProperty(c2, STATUS, 'doing');
    gate.release();
    await writing;
    expect(statusOf(board, c1)).toBe('todo');
    expect(statusOf(board, c2)).toBe('doing');
  });

  it('keeps what the server sent of a card while it waited, though it holds the refused value', async () => {
    const { board, api, other, c1, c2 } = await opened();
    const gate = held();
    vi.spyOn(api, 'batchPatchCards').mockImplementationOnce(async () => {
      await gate.promise;
      throw new BackendError(409);
    });
    vi.spyOn(board.workspace, 'report').mockImplementation(() => undefined);
    const writing = setCardsProperty(board, [c1, c2], STATUS, 'done');
    const before = board.card(c1)?.updatedAt ?? 0;
    // Someone else stores the same value on c1 meanwhile, later than it was read.
    vi.setSystemTime(before + 1_000);
    await other.patchCard(board.boardId, c1, { updatedProperties: { [STATUS]: 'done' } });
    vi.useRealTimers();
    await settle();
    expect(board.card(c1)?.updatedAt).toBeGreaterThan(before);
    gate.release();
    await writing;
    expect(statusOf(board, c1)).toBe('done');
    expect(statusOf(board, c2)).toBe('todo');
  });

  it('sends a card that shows the value only as a batch under way writes it, which is refused', async () => {
    const { board, api, backend, workspace, c1, c2 } = await opened();
    const gate = held();
    const batch = api.batchPatchCards.bind(api);
    let calls = 0;
    const sent = vi.spyOn(api, 'batchPatchCards').mockImplementation(async (...args) => {
      calls += 1;
      if (calls > 1) return batch(...args);
      await gate.promise;
      throw new BackendError(500);
    });
    vi.spyOn(workspace, 'report').mockImplementation(() => undefined);
    const first = setCardsProperty(board, [c1], STATUS, 'done');
    // c1 shows done already, but only as the first batch writes it.
    const second = setCardsProperty(board, [c1, c2], STATUS, 'done');
    gate.release();
    await Promise.all([first, second]);
    expect(sent.mock.calls[1]?.[1]).toEqual([c1, c2]);
    const stored = async (id: string): Promise<unknown> =>
      (await backend.card(board.boardId, id)).fields.properties[STATUS];
    expect([statusOf(board, c1), await stored(c1), await stored(c2)]).toEqual([
      'done',
      'done',
      'done',
    ]);
    // The first was never stored: undo gives c1 back what was, to do.
    await workspace.undoLast();
    expect([await stored(c1), await stored(c2)]).toEqual(['todo', 'todo']);
  });

  it('undoes a change made over one that was refused to what was stored, not to the refused value', async () => {
    const { board, api, backend, workspace, c1 } = await opened();
    const gate = held();
    const batch = api.batchPatchCards.bind(api);
    let calls = 0;
    vi.spyOn(api, 'batchPatchCards').mockImplementation(async (...args) => {
      calls += 1;
      if (calls > 1) return batch(...args);
      await gate.promise;
      throw new BackendError(500);
    });
    vi.spyOn(workspace, 'report').mockImplementation(() => undefined);
    const stored = async (): Promise<unknown> =>
      (await backend.card(board.boardId, c1)).fields.properties[STATUS];
    // To doing, which waits and is refused; meanwhile to done, over it.
    const first = setCardsProperty(board, [c1], STATUS, 'doing');
    const second = setCardsProperty(board, [c1], STATUS, 'done');
    gate.release();
    await Promise.all([first, second]);
    expect([statusOf(board, c1), await stored()]).toEqual(['done', 'done']);

    // Doing was never stored: undo gives back what was, to do.
    await workspace.undoLast();
    expect([statusOf(board, c1), await stored()]).toEqual(['todo', 'todo']);
  });

  it('undoes a change made over a refused one to what the server sent of the card meanwhile', async () => {
    const { board, api, backend, other, workspace, c1 } = await opened();
    const gate = held();
    const batch = api.batchPatchCards.bind(api);
    let calls = 0;
    vi.spyOn(api, 'batchPatchCards').mockImplementation(async (...args) => {
      calls += 1;
      if (calls > 1) return batch(...args);
      await gate.promise;
      throw new BackendError(500);
    });
    vi.spyOn(workspace, 'report').mockImplementation(() => undefined);
    const stored = async (): Promise<unknown> =>
      (await backend.card(board.boardId, c1)).fields.properties[STATUS];
    // To doing, which waits and is refused; meanwhile someone else stores doing,
    // and then to done, over it.
    const first = setCardsProperty(board, [c1], STATUS, 'doing');
    const before = board.card(c1)?.updatedAt ?? 0;
    vi.setSystemTime(before + 1_000);
    await other.patchCard(board.boardId, c1, { updatedProperties: { [STATUS]: 'doing' } });
    vi.useRealTimers();
    await settle();
    const second = setCardsProperty(board, [c1], STATUS, 'done');
    gate.release();
    await Promise.all([first, second]);
    expect([statusOf(board, c1), await stored()]).toEqual(['done', 'done']);

    // Doing was stored by the other: undo gives back that, not to do.
    await workspace.undoLast();
    expect([statusOf(board, c1), await stored()]).toEqual(['doing', 'doing']);
  });

  it('undoes a change made over one that was stored to that one, and is sent after it', async () => {
    const { board, api, backend, workspace, c1 } = await opened();
    const gate = held();
    const batch = api.batchPatchCards.bind(api);
    const order: unknown[] = [];
    vi.spyOn(api, 'batchPatchCards').mockImplementation(async (...args) => {
      const [, , patch] = args;
      if (order.length === 0) {
        order.push(patch.updatedProperties?.[STATUS]);
        await gate.promise;
      } else {
        order.push(patch.updatedProperties?.[STATUS]);
      }
      return batch(...args);
    });
    const first = setCardsProperty(board, [c1], STATUS, 'doing');
    const second = setCardsProperty(board, [c1], STATUS, 'done');
    await settle();
    // The second waits for the first, which holds the same card.
    expect(order).toEqual(['doing']);
    gate.release();
    await Promise.all([first, second]);
    expect(order).toEqual(['doing', 'done']);
    await workspace.undoLast();
    expect((await backend.card(board.boardId, c1)).fields.properties[STATUS]).toBe('doing');
    expect(statusOf(board, c1)).toBe('doing');
  });

  it('is undone in one step: each card gets its own value back, and is done again', async () => {
    const { board, backend, api, workspace, c1, c2, c3 } = await opened();
    await board.setCardProperty(c2, OWNER, backend.userId);
    const sent = vi.spyOn(api, 'batchPatchCards');
    await setCardsProperty(board, [c1, c2, c3], STATUS, 'done');
    await workspace.undoLast();
    expect([c1, c2, c3].map((id) => statusOf(board, id))).toEqual(['todo', 'todo', 'doing']);
    // One request for each value the cards had, told to nobody.
    const undone = sent.mock.calls.slice(1);
    expect(undone.map((call) => [call[1], call[2], call[3]])).toEqual(
      expect.arrayContaining([
        [[c1, c2], { updatedProperties: { [STATUS]: 'todo' } }, { quiet: true }],
        [[c3], { updatedProperties: { [STATUS]: 'doing' } }, { quiet: true }],
      ]),
    );
    expect(undone).toHaveLength(2);
    // The other property was not part of the change.
    expect(board.card(c2)?.fields.properties[OWNER]).toBe(backend.userId);
    const stored = await backend.cards(board.boardId);
    expect(stored.find((card) => card.id === c3)?.fields.properties[STATUS]).toBe('doing');
    // The step before it is still there: the owner of c2.
    expect(workspace.undo.canUndo).toBe(true);
    await workspace.redoLast();
    expect([c1, c2, c3].map((id) => statusOf(board, id))).toEqual(['done', 'done', 'done']);
    expect(sent.mock.calls.at(-1)?.[3]).toEqual({ quiet: true });
  });

  it('clears a value on all, and gives the cards that had none no value again', async () => {
    const { board, backend, workspace, c1, c3 } = await opened();
    await clearCardsProperty(board, [c1, c3], OWNER);
    expect(board.card(c1)?.fields.properties).not.toHaveProperty(OWNER);
    expect(board.card(c3)?.fields.properties).not.toHaveProperty(OWNER);
    await setCardsProperty(board, [c1, c3], OWNER, backend.userId);
    expect(board.card(c3)?.fields.properties[OWNER]).toBe(backend.userId);
    await workspace.undoLast();
    expect(board.card(c1)?.fields.properties).not.toHaveProperty(OWNER);
    expect(board.card(c3)?.fields.properties).not.toHaveProperty(OWNER);
    const stored = await backend.cards(board.boardId);
    expect(stored.find((card) => card.id === c3)?.fields.properties).not.toHaveProperty(OWNER);
  });

  it('undoes without the cards that were deleted meanwhile', async () => {
    const { board, api, other, workspace, c1, c2 } = await opened();
    await setCardsProperty(board, [c1, c2], STATUS, 'done');
    await other.batchDeleteCards(board.boardId, [c2]);
    await settle();
    const sent = vi.spyOn(api, 'batchPatchCards');
    await workspace.undoLast();
    expect(sent.mock.calls.map((call) => call[1])).toEqual([[c1]]);
    expect(statusOf(board, c1)).toBe('todo');
  });

  it('takes an answer that was lost for stored when the cards hold the value', async () => {
    const { board, api, workspace, c1, c2 } = await opened();
    const batch = api.batchPatchCards.bind(api);
    vi.spyOn(api, 'batchPatchCards').mockImplementationOnce(async (...args) => {
      await batch(...args);
      throw new BackendError(0);
    });
    await setCardsProperty(board, [c1, c2], STATUS, 'done');
    expect(statusOf(board, c1)).toBe('done');
    expect(workspace.undo.canUndo).toBe(true);
  });
});

describe('the memory backend of many cards', () => {
  it('changes none when one of them cannot be changed', async () => {
    const { board, backend, c1 } = await opened();
    await expect(
      backend.batchPatchCards(board.boardId, [c1, 'nothing'], {
        updatedProperties: { [STATUS]: 'done' },
      }),
    ).rejects.toMatchObject({ status: 404 });
    const stored = await backend.cards(board.boardId);
    expect(stored.find((card) => card.id === c1)?.fields.properties[STATUS]).toBe('todo');
  });

  it('takes 1 to 200 distinct cards', async () => {
    const { board, backend, c1 } = await opened();
    const patch = { updatedProperties: { [STATUS]: 'done' } };
    await expect(backend.batchPatchCards(board.boardId, [], patch)).rejects.toMatchObject({
      status: 400,
    });
    await expect(backend.batchPatchCards(board.boardId, [c1, c1], patch)).rejects.toMatchObject({
      status: 400,
    });
    const many = Array.from({ length: CARD_BATCH_LIMIT + 1 }, (_, at) => `c${at}`);
    await expect(backend.batchPatchCards(board.boardId, many, patch)).rejects.toMatchObject({
      status: 400,
    });
  });

  it('answers the cards as they are now, in the order asked', async () => {
    const { board, backend, c1, c3 } = await opened();
    const answer = await backend.batchPatchCards(board.boardId, [c3, c1], {
      deletedProperties: [STATUS],
    });
    expect(answer.map((card) => card.id)).toEqual([c3, c1]);
    expect(answer.every((card) => !(STATUS in card.fields.properties))).toBe(true);
  });
});
