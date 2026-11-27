/**
 * The undo manager past its plain cases, and the calculations at their
 * edges. A case that pins a numbered rule names it (Q9, Q14, Q15 in
 * docs/notes.md).
 */
import { describe, expect, it } from 'vitest';

import { CALCULATIONS } from '$lib/logic/calculations.ts';
import { calendarDate } from '$lib/logic/dates.ts';
import { UndoManager } from '$lib/logic/undo.ts';
import {
  createCard,
  setCardProperty,
  type Card,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';

import { storedDate } from './support/factory.ts';

import { at } from '../support/zones.ts';

const nothing = async (): Promise<void> => undefined;
const en = { locale: 'en', timeZone: 'UTC' };

describe('undo manager', () => {
  it('tells its listener about every change of what can be undone, and of being busy', async () => {
    const manager = new UndoManager();
    const seen: string[] = [];
    manager.onStateDidChange = () => {
      seen.push(`${manager.canUndo}/${manager.canRedo}/${manager.busy}`);
    };
    await manager.perform(nothing, nothing);
    await manager.undo();
    await manager.redo();
    manager.clear();
    manager.clear(); // nothing left to clear: no call
    // An undo or a redo is told twice: when it is asked for, and when it is over.
    expect(seen).toEqual([
      'true/false/false',
      'false/true/true',
      'false/true/false',
      'true/false/true',
      'true/false/false',
      'false/false/false',
    ]);
  });

  it('forgets what was undone once a new change is made', async () => {
    const manager = new UndoManager();
    await manager.perform(nothing, nothing, 'first');
    await manager.perform(nothing, nothing, 'second');
    await manager.undo();
    expect(manager.redoDescription).toBe('second');
    await manager.perform(nothing, nothing, 'third');
    expect(manager.canRedo).toBe(false);
    expect(manager.undoDescription).toBe('third');
  });

  it('records a change made while an undo is carried out as the newest step', async () => {
    const manager = new UndoManager();
    let made: Promise<unknown> | undefined;
    await manager.perform(
      nothing,
      async () => {
        // The person's change, made while the undo's write is under way.
        made = manager.perform(nothing, nothing, 'made meanwhile');
        await Promise.resolve();
      },
      'first',
    );
    await manager.undo();
    await made;
    expect(manager.undoDescription).toBe('made meanwhile');
    expect(manager.canRedo).toBe(false);
  });

  it('keeps working after an undo failed', async () => {
    const manager = new UndoManager();
    let fail = true;
    await manager.perform(nothing, async () => {
      if (fail) throw new Error('server said no');
    });
    await expect(manager.undo()).rejects.toThrow('server said no');
    // The failed change is still the one to undo, and it can be tried again.
    expect(manager.canUndo).toBe(true);
    fail = false;
    await manager.undo();
    expect(manager.canUndo).toBe(false);
  });

  it('gives a discardable change the checkpoint of the change before it', async () => {
    const manager = new UndoManager();
    await manager.perform(nothing, nothing, 'a');
    await manager.perform(nothing, nothing, 'b');
    const before = manager.currentCheckpoint;
    await manager.perform(nothing, nothing, 'typing', undefined, true);
    expect(manager.currentCheckpoint).toBe(before);
  });

  it('also when the discardable change is the second one (Q14)', async () => {
    const manager = new UndoManager();
    await manager.perform(nothing, nothing, 'a');
    const before = manager.currentCheckpoint;
    expect(before).toBeGreaterThan(0);
    await manager.perform(nothing, nothing, 'typing', undefined, true);
    expect(manager.currentCheckpoint).toBe(before);
  });

  it('gives a discardable change that comes first the checkpoint 0', async () => {
    const manager = new UndoManager();
    await manager.perform(nothing, nothing, 'typing', undefined, true);
    expect(manager.currentCheckpoint).toBe(0);
  });

  it('keeps working after a redo failed', async () => {
    const manager = new UndoManager();
    let fail = false;
    const values: string[] = [];
    await manager.perform(
      async () => {
        if (fail) throw new Error('server said no');
        values.push('a');
      },
      async () => {
        values.pop();
      },
    );
    await manager.undo();
    fail = true;
    await expect(manager.redo()).rejects.toThrow('server said no');
    // Nothing was redone: the change is still the one to redo.
    expect(manager.canRedo).toBe(true);
    expect(manager.canUndo).toBe(false);
    fail = false;
    await manager.redo();
    expect(values).toEqual(['a']);
    expect(manager.canUndo).toBe(true);
  });

  it('records changes again after a failure', async () => {
    const manager = new UndoManager();
    await manager.perform(nothing, async () => {
      throw new Error('server said no');
    });
    await expect(manager.undo()).rejects.toThrow();
    await manager.perform(nothing, nothing, 'after the failure');
    expect(manager.undoDescription).toBe('after the failure');
  });

  it('undoes a group together, and stops at a failure inside it', async () => {
    const manager = new UndoManager();
    const done: string[] = [];
    const step = (name: string, failing = false) =>
      manager.perform(
        async () => {
          done.push(name);
        },
        async () => {
          if (failing) throw new Error(`cannot undo ${name}`);
          done.splice(done.indexOf(name), 1);
        },
        name,
        'group',
      );
    await step('one');
    await step('two', true);
    await step('three');
    await expect(manager.undo()).rejects.toThrow('cannot undo two');
    // "three" was undone, "two" was not: it is the next to undo.
    expect(done).toEqual(['one', 'two']);
    expect(manager.undoDescription).toBe('two');
  });
});

describe('number calculations at the edges', () => {
  const number: PropertyTemplate = { id: 'n', name: 'N', type: 'number', options: [] };
  const withNumber = (value: string): Card => {
    const card = createCard();
    setCardProperty(card, 'n', value);
    return card;
  };

  it('gives 0 for the minimum and maximum of no values', () => {
    expect(CALCULATIONS.min([], number, en)).toBe('0');
    expect(CALCULATIONS.max([createCard()], number, en)).toBe('0');
    expect(CALCULATIONS.range([], number, en)).toBe('0 - 0');
  });

  it('shows an infinite value as it is', () => {
    expect(CALCULATIONS.max([withNumber('Infinity')], number, en)).toBe('Infinity');
    expect(CALCULATIONS.min([withNumber('-Infinity')], number, en)).toBe('-Infinity');
  });

  it('shows a value that is not a number as NaN', () => {
    expect(CALCULATIONS.sum([withNumber('abc'), withNumber('1')], number, en)).toBe('NaN');
  });
});

describe('checkbox calculations', () => {
  const checkbox: PropertyTemplate = { id: 'done', name: 'Done', type: 'checkbox', options: [] };

  it('shows nothing for the percentage of no cards (Q15)', () => {
    expect(CALCULATIONS.percentChecked([], checkbox, en)).toBe('');
    expect(CALCULATIONS.percentUnchecked([], checkbox, en)).toBe('');
    expect(CALCULATIONS.percentEmpty([], checkbox, en)).toBe('');
  });

  it('counts only a stored `true` as a tick (Q9)', () => {
    const ticked = (value?: string): Card => {
      const card = createCard();
      if (value !== undefined) setCardProperty(card, 'done', value);
      return card;
    };
    const cards = [ticked('true'), ticked('false'), ticked(''), ticked(), ticked('true')];
    expect(CALCULATIONS.countChecked(cards, checkbox, en)).toBe('2');
    expect(CALCULATIONS.countUnchecked(cards, checkbox, en)).toBe('3');
    expect(CALCULATIONS.percentChecked(cards, checkbox, en)).toBe('40%');
    expect(CALCULATIONS.percentUnchecked(cards, checkbox, en)).toBe('60%');
  });
});

describe('date calculations', () => {
  const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
  const created: PropertyTemplate = {
    id: 'created',
    name: 'Created',
    type: 'createdTime',
    options: [],
  };
  const context = { locale: 'en', now: Date.UTC(2026, 5, 1), timeZone: 'UTC' };
  const withDue = (value: PropertyValue): Card => {
    const card = createCard();
    setCardProperty(card, 'due', value);
    return card;
  };
  const day = (date: number) => ({ date: calendarDate(2026, 3, date) });
  const cards = [
    withDue(storedDate({ from: day(9) })),
    withDue(storedDate({ from: day(5), to: day(12) })),
    withDue('not a date'),
    createCard(),
  ];

  it('finds the earliest and latest day over single days and ranges', () => {
    expect(CALCULATIONS.earliest(cards, due, context)).toBe('March 05');
    expect(CALCULATIONS.latest(cards, due, context)).toBe('March 12');
    expect(CALCULATIONS.earliest([createCard()], due, context)).toBe('');
  });

  it('counts both ends of the range of days', () => {
    expect(CALCULATIONS.dateRange(cards, due, context)).toBe('8 days');
    expect(CALCULATIONS.dateRange(cards.slice(0, 1), due, context)).toBe('1 day');
    expect(CALCULATIONS.dateRange([], due, context)).toBe('');
  });

  it('gives the same days in every time zone', () => {
    for (const timeZone of ['Asia/Seoul', 'Pacific/Kiritimati', 'Pacific/Pago_Pago']) {
      expect(CALCULATIONS.earliest(cards, due, { ...context, timeZone })).toBe('March 05');
      expect(CALCULATIONS.latest(cards, due, { ...context, timeZone })).toBe('March 12');
    }
  });

  it('works on the moments cards were created, on the days of the reader', () => {
    const born = (instant: number): Card => {
      const card = createCard();
      card.createdAt = instant;
      return card;
    };
    const seoul = { locale: 'en', now: Date.UTC(2026, 5, 1), timeZone: 'Asia/Seoul' };
    const two = [
      born(at('Asia/Seoul', 2026, 3, 5, 9, 0)),
      born(at('Asia/Seoul', 2026, 3, 8, 9, 0)),
    ];
    expect(CALCULATIONS.earliest(two, created, seoul)).toBe('March 05 at 9:00 AM');
    expect(CALCULATIONS.latest(two, created, seoul)).toBe('March 08 at 9:00 AM');
    // Between two moments the range is the time that passed, not a count of days.
    expect(CALCULATIONS.dateRange(two, created, seoul)).toBe('3 days');
    // The same two moments for a reader in Honolulu: the 4th and the 7th, in the afternoon.
    const honolulu = { ...seoul, timeZone: 'Pacific/Honolulu' };
    expect(CALCULATIONS.earliest(two, created, honolulu)).toBe('March 04 at 2:00 PM');
    expect(CALCULATIONS.dateRange(two, created, honolulu)).toBe('3 days');
  });
});
