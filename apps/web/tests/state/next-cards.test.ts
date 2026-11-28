/**
 * The next card that completing a card that repeats makes, as a board shows
 * it: the one who completed it is told its number, and the undo of the
 * change makes the card not done and repeating again and takes the next card
 * away; a redo makes a next card anew. A drop into the column of done, and a
 * value set on several cards at once, are the same.
 */
import { describe, expect, it } from 'vitest';

import { cardProperty, cardRecurredTo, cardRecurrence } from '$lib/logic/wire.ts';
import { nextCardNotices, setCardsProperty } from '$lib/state/index.ts';

import { settle, setup, STATUS } from './support.ts';

const DUE = 'due';
const weekly = { every: 1, unit: 'week', anchor: 'due' } as const;

/** The small board with a due date it names and `done` as its rule of done, open, with a card that repeats. */
async function chores() {
  const made = setup();
  const found = [...made.world.containers.values()].find((board) => board.title === 'Small');
  if (!found) throw new Error('the seed has no such board');
  await made.backend.patchContainer(found.id, {
    updatedCardProperties: [{ id: DUE, name: 'Due', type: 'date', options: [] }],
    doneWhen: { kind: 'option', propertyId: STATUS, optionIds: ['done'] },
    dueDatePropertyId: DUE,
  });
  await made.workspace.load();
  const board = await made.workspace.openBoard(found.id);
  const card = await board.createCard({
    title: 'Water',
    properties: { [STATUS]: 'todo', [DUE]: { from: '2099-01-15' } },
    recurrence: weekly,
  });
  if (!card) throw new Error('the card was not made');
  const waters = async () =>
    (await made.backend.cards(found.id)).filter((each) => each.title === 'Water');
  return { ...made, board, card, waters, notices: nextCardNotices(made.workspace) };
}

describe('the next card of a card that repeats, on a board', () => {
  it('is told to the one who completed it, and goes with the undo', async () => {
    const { board, card, waters, notices } = await chores();
    await board.setCardProperty(card.id, STATUS, 'done');
    await settle();
    const nextId = cardRecurredTo(board.card(card.id) ?? card);
    expect(nextId).toBeDefined();
    const next = board.card(nextId);
    expect(next?.number).toBeDefined();
    expect(notices.notice).toMatchObject({ number: next?.number, count: 1 });
    expect(await waters()).toHaveLength(2);

    await notices.undo();
    await settle();
    expect(notices.notice).toBeUndefined();
    const undone = board.card(card.id);
    expect(undone && cardProperty(undone, STATUS)).toBe('todo');
    expect(undone && cardRecurrence(undone)).toEqual(weekly);
    expect(undone && cardRecurredTo(undone)).toBeUndefined();
    expect(board.card(nextId)).toBeUndefined();
    expect(await waters()).toHaveLength(1);
  });

  it('is made anew by the redo, and taken back by the undo after it', async () => {
    const { board, card, waters, workspace } = await chores();
    await board.setCardProperty(card.id, STATUS, 'done');
    const first = cardRecurredTo(board.card(card.id) ?? card);
    await workspace.undoLast();
    await workspace.redoLast();
    const again = cardRecurredTo(board.card(card.id) ?? card);
    expect(again).toBeDefined();
    expect(again).not.toBe(first);
    expect(await waters()).toHaveLength(2);
    await workspace.undoLast();
    expect(await waters()).toHaveLength(1);
    expect(cardRecurrence(board.card(card.id) ?? card)).toEqual(weekly);
  });

  it('comes of a drop into the column of done, and goes with its undo', async () => {
    const { board, card, waters, notices, workspace } = await chores();
    const view = board.orderedViews[0];
    if (!view) throw new Error('no view');
    await board.dropCards({ viewId: view.id, cardIds: [card.id], optionId: 'done', index: 0 });
    await settle();
    expect(notices.notice?.count).toBe(1);
    expect(await waters()).toHaveLength(2);
    await workspace.undoLast();
    expect(await waters()).toHaveLength(1);
    const undone = board.card(card.id);
    expect(undone && cardProperty(undone, STATUS)).toBe('todo');
    expect(undone && cardRecurrence(undone)).toEqual(weekly);
  });

  it('comes of a value set on several cards at once, and goes with its undo', async () => {
    const { board, card, waters, notices, workspace } = await chores();
    const other = await board.createCard({
      title: 'Water',
      properties: { [STATUS]: 'todo', [DUE]: { from: '2099-01-16' } },
      recurrence: weekly,
    });
    if (!other) throw new Error('the card was not made');
    await setCardsProperty(board, [card.id, other.id], STATUS, 'done');
    await settle();
    expect(notices.notice?.count).toBe(2);
    expect(await waters()).toHaveLength(4);
    await workspace.undoLast();
    expect(await waters()).toHaveLength(2);
    for (const id of [card.id, other.id]) {
      const undone = board.card(id);
      expect(undone && cardProperty(undone, STATUS)).toBe('todo');
      expect(undone && cardRecurrence(undone)).toEqual(weekly);
      expect(undone && cardRecurredTo(undone)).toBeUndefined();
    }
  });

  it('stops repeating and goes before the card repeats again, and stays once done', async () => {
    const { backend, board, card, waters, workspace } = await chores();
    await board.setCardProperty(card.id, STATUS, 'done');
    const calls: string[] = [];
    const patching = backend.patchCard.bind(backend);
    const deleting = backend.batchDeleteCards.bind(backend);
    backend.patchCard = async (...args) => {
      if (args[2].recurrence === null) calls.push('stop');
      else if (args[2].recurrence) calls.push('repeat');
      return patching(...args);
    };
    backend.batchDeleteCards = async (...args) => {
      calls.push('delete');
      return deleting(...args);
    };
    const firstNext = cardRecurredTo(board.card(card.id) ?? card) ?? '';
    await workspace.undoLast();
    expect(calls).toEqual(['stop', 'delete', 'repeat']);

    // Done by another before the undo: the chain goes on from it.
    await workspace.redoLast();
    const nextId = cardRecurredTo(board.card(card.id) ?? card) ?? '';
    await backend.patchCard(board.boardId, nextId, { updatedProperties: { [STATUS]: 'done' } });
    calls.length = 0;
    await workspace.undoLast();
    expect(calls).toEqual([]);
    const undone = board.card(card.id);
    expect(undone && cardProperty(undone, STATUS)).toBe('todo');
    expect(undone && cardRecurrence(undone)).toBeUndefined();
    expect(await waters()).toHaveLength(3);
    // Back from the trash, the first next card does not repeat: no second chain.
    const [back] = await backend.batchRestoreCards(board.boardId, [firstNext]);
    expect(back && cardRecurrence(back)).toBeUndefined();
  });

  it('leaves the history alone once a later change came', async () => {
    const { board, card, waters, notices } = await chores();
    await board.setCardProperty(card.id, STATUS, 'done');
    await settle();
    await board.changeCard(card.id, (each) => ({ ...each, title: 'Water the fern' }));
    await notices.undo();
    expect(notices.notice).toBeUndefined();
    expect(board.card(card.id)?.title).toBe('Water the fern');
    // The next card, still called as the card was, stays.
    expect(await waters()).toHaveLength(1);
  });
});
