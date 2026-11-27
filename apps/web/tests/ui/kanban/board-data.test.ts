/**
 * What the kanban reads from and writes to the board store besides drops
 * (features/views/kanban/board-data.ts): the options a board is grouped by,
 * and the properties of a card made in a column.
 */
import { describe, expect, it } from 'vitest';

import {
  addGroup,
  badgesOf,
  changeOption,
  columnValue,
  deleteOption,
  propertiesForColumn,
} from '$lib/features/views/kanban/board-data.ts';
import { createCard } from '$lib/logic/wire.ts';

import { openFirstBoard, STATUS } from '../../state/support.ts';

const NO_STATUS = (name: string): string => `No ${name}`;

describe('the options a board is grouped by', () => {
  it('get a new one that the view shows last, and one undo takes both back', async () => {
    const { board, workspace } = await openFirstBoard();
    const view = board.orderedViews[0];
    if (!view) throw new Error('no view');
    const id = await addGroup(board, view, STATUS, 'Review', ['todo', 'doing', 'done', '']);

    const options = board.board?.cardProperties.find((template) => template.id === STATUS)?.options;
    expect(options?.at(-1)).toEqual({ id, label: 'Review', color: 'default' });
    expect(board.view(view.id)?.fields.visibleGroups).toEqual(['todo', 'doing', 'done', '', id]);

    await workspace.undoLast();
    expect(
      board.board?.cardProperties.find((template) => template.id === STATUS)?.options,
    ).toHaveLength(3);
    expect(board.view(view.id)?.fields.visibleGroups).toEqual(['todo', 'doing', 'done', '']);
  });

  it('are renamed and given another colour', async () => {
    const { board } = await openFirstBoard();
    await changeOption(board, STATUS, 'doing', { label: 'Under way' });
    await changeOption(board, STATUS, 'doing', { color: 'pink' });
    const option = board.board?.cardProperties
      .find((template) => template.id === STATUS)
      ?.options.find((candidate) => candidate.id === 'doing');
    expect(option).toEqual({ id: 'doing', label: 'Under way', color: 'pink' });
  });

  it('lose a deleted one, whose cards then show without a value, until undo', async () => {
    const { board, workspace } = await openFirstBoard();
    const view = board.orderedViews[0];
    if (!view) throw new Error('no view');
    await deleteOption(board, STATUS, 'todo');
    const groups = board.groupsOf(view, board.cardsOf(view), NO_STATUS);
    expect(groups.visible.map((group) => [group.option.label, group.cards.length])).toEqual([
      ['Doing', 1],
      ['Done', 0],
      ['No Status', 2],
    ]);
    await workspace.undoLast();
    const back = board.groupsOf(view, board.cardsOf(view), NO_STATUS);
    expect(back.visible[0]?.cards.map((card) => card.title)).toEqual(['Alpha', 'Beta']);
  });
});

describe('a card made in a column', () => {
  it('gets the value of the column, and none in the column of cards without one', async () => {
    const { board } = await openFirstBoard();
    const view = board.orderedViews[0];
    if (!view || !board.board) throw new Error('no view');
    expect(propertiesForColumn(board.board, view, STATUS, 'done')).toEqual({ [STATUS]: 'done' });
    expect(propertiesForColumn(board.board, view, STATUS, '')).toEqual({});
  });

  it('gets what the filter of the view asks for, but the column decides its own property', async () => {
    const { board } = await openFirstBoard();
    const view = board.orderedViews[0];
    if (!view || !board.board) throw new Error('no view');
    const filtered = {
      ...view,
      fields: {
        ...view.fields,
        filter: {
          operation: 'and' as const,
          filters: [{ propertyId: STATUS, condition: 'includes' as const, values: ['todo'] }],
        },
      },
    };
    expect(propertiesForColumn(board.board, filtered, STATUS, 'done')).toEqual({
      [STATUS]: 'done',
    });
    expect(propertiesForColumn(board.board, filtered, STATUS, '')).toEqual({});
  });
});

describe('what a card says for its face and its column', () => {
  it('reads the body and tasks of a card from a list, and nothing from a card without them', async () => {
    const { board } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    expect(badgesOf(alpha)).toEqual({ hasBody: true, tasksTotal: 0, tasksDone: 0 });
    expect(badgesOf(createCard())).toEqual({ hasBody: false, tasksTotal: 0, tasksDone: 0 });
  });

  it('reads the column of a card as the store does: one identifier, or none', () => {
    const card = createCard();
    card.fields.properties = { status: 'todo', tags: ['a', 'b'] };
    expect(columnValue(card, 'status')).toBe('todo');
    expect(columnValue(card, 'tags')).toBe('');
    expect(columnValue(card, 'missing')).toBe('');
  });
});
