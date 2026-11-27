/** What the kanban works out without a page (features/views/kanban/kanban.ts). */
import { describe, expect, it } from 'vitest';

import {
  cardLists,
  columnIdOf,
  drawnColumns,
  hiddenIdOf,
  listsAfterDrop,
  nearestRest,
  optionIdOf,
  sortedIndex,
  toggled,
  withColumnHidden,
  withColumnShown,
  withColumnStepped,
  withRange,
} from '$lib/features/views/kanban/kanban.ts';
import { createCard, type Card } from '$lib/logic/wire.ts';

function card(id: string): Card {
  const made = createCard();
  made.id = id;
  return made;
}

describe('the names of columns while dragging', () => {
  it('are never empty, and give the option back', () => {
    for (const optionId of ['', 'todo', 'c-odd']) {
      expect(columnIdOf(optionId)).not.toBe('');
      expect(optionIdOf(columnIdOf(optionId))).toBe(optionId);
      expect(optionIdOf(hiddenIdOf(optionId))).toBe(optionId);
    }
  });

  it('tell a column from a hidden column of the same option', () => {
    expect(columnIdOf('todo')).not.toBe(hiddenIdOf('todo'));
  });

  it('make the visible columns the lists of a sortable', () => {
    const groups = [
      {
        option: { id: 'todo', label: 'To do', color: 'default' as const },
        cards: [card('a'), card('b')],
      },
      { option: { id: '', label: 'No status', color: 'default' as const }, cards: [] },
    ];
    expect(cardLists(groups)).toEqual([
      { id: columnIdOf('todo'), items: ['a', 'b'] },
      { id: columnIdOf(''), items: [] },
    ]);
  });
});

describe('where a sorted view shows a dropped card', () => {
  const order = ['a', 'b', 'c', 'd', 'e'];

  it('is after the cards of the column that come before it in the order of the view', () => {
    expect(sortedIndex(order, ['a', 'e'], 'c', ['c'])).toBe(1);
    expect(sortedIndex(order, ['d', 'e'], 'c', ['c'])).toBe(0);
    expect(sortedIndex(order, ['a', 'b'], 'c', ['c'])).toBe(2);
    expect(sortedIndex(order, [], 'c', ['c'])).toBe(0);
  });

  it('does not count the cards that travel with it', () => {
    expect(sortedIndex(order, ['a', 'b', 'e'], 'd', ['b', 'd'])).toBe(1);
  });

  it('is the end for a card the view does not know', () => {
    expect(sortedIndex(order, ['a', 'b'], 'new', ['new'])).toBe(2);
  });
});

describe('the lists as a drop leaves them', () => {
  const lists = [
    { id: 'one', items: ['a', 'b', 'c'] },
    { id: 'two', items: ['d'] },
  ];

  it('have the cards at the place, in the order they had', () => {
    expect(listsAfterDrop(lists, ['c', 'a'], 'two', 0)).toEqual([
      { id: 'one', items: ['b'] },
      { id: 'two', items: ['a', 'c', 'd'] },
    ]);
    expect(listsAfterDrop(lists, ['a'], 'one', Number.POSITIVE_INFINITY)[0]?.items).toEqual([
      'b',
      'c',
      'a',
    ]);
  });

  it('lose the cards dropped on a column that is not shown', () => {
    expect(listsAfterDrop(lists, ['a', 'd'], 'hidden', 0)).toEqual([
      { id: 'one', items: ['b', 'c'] },
      { id: 'two', items: [] },
    ]);
  });
});

describe('selecting', () => {
  it('takes a card in, and out when it was in', () => {
    expect(toggled([], 'a')).toEqual(['a']);
    expect(toggled(['a', 'b'], 'a')).toEqual(['b']);
  });

  const order = ['a', 'b', 'c', 'd', 'e'];

  it('takes in the range from the anchor, in either direction', () => {
    expect(withRange(order, ['b'], 'b', 'd')).toEqual(['b', 'c', 'd']);
    expect(withRange(order, ['d'], 'd', 'b')).toEqual(['d', 'b', 'c']);
  });

  it('keeps what was selected outside the range', () => {
    expect(withRange(order, ['a', 'e'], 'e', 'c')).toEqual(['a', 'e', 'c', 'd']);
  });

  it('adds the card alone without an anchor on the board', () => {
    expect(withRange(order, [], undefined, 'c')).toEqual(['c']);
    expect(withRange(order, ['a'], 'gone', 'c')).toEqual(['a', 'c']);
  });

  it('changes nothing for a card that is not on the board', () => {
    expect(withRange(order, ['a'], 'a', 'gone')).toEqual(['a']);
  });
});

describe('the columns of a view', () => {
  const shown = ['todo', 'doing', 'done', ''];

  it('lose a hidden column, which is listed as hidden once', () => {
    expect(withColumnHidden(shown, [], 'doing')).toEqual({
      visibleGroups: ['todo', 'done', ''],
      hiddenGroups: ['doing'],
    });
    expect(withColumnHidden(shown, ['doing', 'old'], 'doing').hiddenGroups).toEqual([
      'old',
      'doing',
    ]);
  });

  it('can hide the column of cards without a value', () => {
    expect(withColumnHidden(shown, [], '')).toEqual({
      visibleGroups: ['todo', 'doing', 'done'],
      hiddenGroups: [''],
    });
  });

  it('get a shown column back as the last one', () => {
    expect(withColumnShown(['todo', ''], ['doing', 'done'], 'doing')).toEqual({
      visibleGroups: ['todo', '', 'doing'],
      hiddenGroups: ['done'],
    });
  });

  it('move a column by a place and stop at the ends', () => {
    expect(withColumnStepped(shown, 'doing', 1)).toEqual(['todo', 'done', 'doing', '']);
    expect(withColumnStepped(shown, 'doing', -1)).toEqual(['doing', 'todo', 'done', '']);
    expect(withColumnStepped(shown, 'todo', -1)).toEqual(shown);
    expect(withColumnStepped(shown, '', 1)).toEqual(shown);
    expect(withColumnStepped(shown, 'gone', 1)).toEqual(shown);
  });
});

describe('the columns as drawn while one is dragged', () => {
  const groups = ['a', 'b', 'c'].map((id) => ({
    option: { id, label: id, color: 'default' as const },
    cards: [],
  }));
  const drawn = (drag: Parameters<typeof drawnColumns>[1]): string[] =>
    drawnColumns(groups, drag).map((entry) =>
      entry.kind === 'gap' ? 'gap' : `${entry.group.option.id}${entry.at}`,
    );

  it('are the columns in their order when nothing is dragged', () => {
    expect(drawn(undefined)).toEqual(['a0', 'b1', 'c2']);
  });

  it('keep the column a pointer drags where it is, and show a gap where it would land', () => {
    const drag = { mode: 'pointer' as const, itemId: columnIdOf('a') };
    // The gap stands before the column that would follow; the dragged one is hidden, wherever it is.
    expect(drawn({ ...drag, index: 0 })).toEqual(['a0', 'gap', 'b1', 'c2']);
    expect(drawn({ ...drag, index: 1 })).toEqual(['a0', 'b1', 'gap', 'c2']);
    expect(drawn({ ...drag, index: 2 })).toEqual(['a0', 'b1', 'c2', 'gap']);
    expect(drawn({ ...drag, index: 99 })).toEqual(['a0', 'b1', 'c2', 'gap']);
  });

  it('stand the column the keyboard carries at the place it would land', () => {
    const drag = { mode: 'keyboard' as const, itemId: columnIdOf('a') };
    expect(drawn({ ...drag, index: 0 })).toEqual(['a0', 'b1', 'c2']);
    expect(drawn({ ...drag, index: 1 })).toEqual(['b1', 'a0', 'c2']);
    expect(drawn({ ...drag, index: 2 })).toEqual(['b1', 'c2', 'a0']);
  });
});

describe('the column a board rests on', () => {
  it('is the one whose resting place is nearest', () => {
    expect(nearestRest([0, 300, 600], 0)).toBe(0);
    expect(nearestRest([0, 300, 600], 140)).toBe(0);
    expect(nearestRest([0, 300, 600], 160)).toBe(1);
    expect(nearestRest([0, 300, 600], 9000)).toBe(2);
  });

  it('is never a column that is not laid out', () => {
    expect(nearestRest([0, Number.NaN, 600], 290)).toBe(0);
    expect(nearestRest([0, Number.NaN, 600], 310)).toBe(2);
  });

  it('is the first when there is nothing to rest on', () => {
    expect(nearestRest([], 100)).toBe(0);
  });
});
