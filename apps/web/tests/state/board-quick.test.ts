/**
 * The quick filters a reader has turned on: kept in the browser under the
 * reader and the board, or for the visit by a reader who is not signed in;
 * the cards the reader made since the chips changed; and the cards a view
 * shows with them, between its filter and its search.
 */
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { QuickFilterId } from '$lib/logic/quick-filter.ts';
import { createCard, type BoardView, type Card, type Container } from '$lib/logic/wire.ts';
import {
  QUICK_FILTERS_KEY,
  QuickFilterChoices,
  quickFilters,
  quickFiltersKey,
  type VisitedBoard,
} from '$lib/state/board-quick.svelte.ts';
import type { CardSummary } from '$lib/state/backend.ts';
import {
  cardsOfBoardView,
  filteredCardsOfBoardView,
  type BoardCardsSource,
} from '$lib/state/board-parts.ts';
import { forgetUnscoped, forgetUser } from '../../src/app/last-workspace.ts';

import { watched } from './watched.svelte.ts';

/** A storage of its own, as `localStorage` is. */
function memoryStorage(): Storage {
  const items = new Map<string, string>();
  return {
    get length() {
      return items.size;
    },
    clear: () => {
      items.clear();
    },
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem: (key) => {
      items.delete(key);
    },
    setItem: (key, value) => {
      items.set(key, value);
    },
  };
}

const cardOf = (id: string, createdBy = 'me'): Card => ({ ...createCard(), id, createdBy });

/** A board store as the chips see it. */
const boardOf = (boardId: string, cards: Card[] = [], visit = 1): VisitedBoard => ({
  boardId,
  visit,
  realCards: cards,
});

describe('the chips of a board', () => {
  it('are kept in the browser under the reader and the board, as their names and nothing else', () => {
    const storage = memoryStorage();
    const choices = new QuickFilterChoices(() => storage);
    const board = boardOf('board-1');
    choices.toggle(board, 'overdue', 'me');
    choices.toggle(board, 'mine', 'me');
    expect(choices.chosen('board-1', 'me')).toEqual(['overdue', 'mine']);
    expect(storage.getItem(`${QUICK_FILTERS_KEY}me:board-1`)).toBe('["overdue","mine"]');
    expect(quickFiltersKey('me', 'board-1')).toBe('fvoci:quick-filters:me:board-1');
    expect(choices.chosen('board-2', 'me')).toEqual([]);
    // Another reader of the same browser has none of them.
    expect(choices.chosen('board-1', 'you')).toEqual([]);
    // Another page reads them back.
    expect(new QuickFilterChoices(() => storage).chosen('board-1', 'me')).toEqual([
      'overdue',
      'mine',
    ]);
  });

  it('are taken off one at a time, or all at once, and leave nothing behind', () => {
    const storage = memoryStorage();
    const choices = new QuickFilterChoices(() => storage);
    const board = boardOf('board-1');
    choices.toggle(board, 'overdue', 'me');
    choices.toggle(board, 'incomplete', 'me');
    choices.toggle(board, 'overdue', 'me');
    expect(choices.chosen('board-1', 'me')).toEqual(['incomplete']);
    choices.clear(board, 'me');
    expect(choices.chosen('board-1', 'me')).toEqual([]);
    expect(storage.length).toBe(0);
  });

  it('are neither read from storage nor written to it for a reader who is not signed in', () => {
    const storage = memoryStorage();
    storage.setItem(quickFiltersKey('', 'board-1'), '["incomplete"]');
    storage.setItem(quickFiltersKey('me', 'board-1'), '["mine"]');
    const choices = new QuickFilterChoices(() => storage);
    // A public link's reader has an empty identifier.
    for (const reader of [undefined, '']) expect(choices.chosen('board-1', reader)).toEqual([]);
    choices.toggle(boardOf('board-1'), 'overdue', '');
    expect(choices.chosen('board-1', undefined)).toEqual(['overdue']);
    expect(storage.length).toBe(2);
    expect(storage.getItem(quickFiltersKey('', 'board-1'))).toBe('["incomplete"]');
    // The signed-in reader's are their own still.
    expect(choices.chosen('board-1', 'me')).toEqual(['mine']);
  });

  it('last for the visit in a browser without storage', () => {
    const blocked = new QuickFilterChoices(() => {
      throw new Error('storage is blocked');
    });
    blocked.toggle(boardOf('board-1'), 'overdue', 'me');
    expect(blocked.chosen('board-1', 'me')).toEqual(['overdue']);
  });

  it('read nothing they do not know from storage', () => {
    const storage = memoryStorage();
    storage.setItem(quickFiltersKey('me', 'a'), '["mine","mine","everything",3]');
    storage.setItem(quickFiltersKey('me', 'b'), 'not json');
    storage.setItem(quickFiltersKey('me', 'c'), '{"mine":true}');
    const choices = new QuickFilterChoices(() => storage);
    expect(choices.chosen('a', 'me')).toEqual(['mine']);
    expect(choices.chosen('b', 'me')).toEqual([]);
    expect(choices.chosen('c', 'me')).toEqual([]);
  });

  it('are forgotten, in storage and on the page, when the reader signs out', () => {
    const board = boardOf('board-1');
    quickFilters.toggle(board, 'mine', 'me');
    quickFilters.toggle(board, 'overdue', undefined);
    localStorage.setItem('fvoci:locale', 'ko');
    forgetUnscoped();
    expect(localStorage.getItem(quickFiltersKey('me', 'board-1'))).toBeNull();
    expect(localStorage.getItem('fvoci:locale')).toBe('ko');
    expect(quickFilters.chosen('board-1', 'me')).toEqual([]);
    expect(quickFilters.chosen('board-1', undefined)).toEqual([]);
    localStorage.clear();
  });

  it('are read again as a board is shown again, after another tab changed them while none was', async () => {
    const choices = new QuickFilterChoices(() => localStorage);
    const key = quickFiltersKey('me', 'board-1');
    let seen: readonly QuickFilterId[] = [];
    const show = (): (() => void) =>
      watched(() => {
        seen = choices.chosen('board-1', 'me');
      });
    let stop = show();
    choices.toggle(boardOf('board-1'), 'mine', 'me');
    stop();
    // The last board shown goes: nothing here listens to storage.
    await tick();
    localStorage.setItem(key, '["overdue"]');
    stop = show();
    expect(seen).toEqual(['overdue']);
    stop();
    localStorage.clear();
  });

  it('are read from storage once no board shows them, though nothing shows them again', async () => {
    const choices = new QuickFilterChoices(() => localStorage);
    const stop = watched(() => {
      choices.chosen('board-1', 'me');
    });
    choices.toggle(boardOf('board-1'), 'mine', 'me');
    stop();
    await tick();
    localStorage.setItem(quickFiltersKey('me', 'board-1'), '["overdue"]');
    expect(choices.chosen('board-1', 'me')).toEqual(['overdue']);
    localStorage.clear();
  });

  it('that storage did not take stay on for the visit as a board is shown again', async () => {
    const blocked = new QuickFilterChoices(() => {
      throw new Error('storage is blocked');
    });
    let seen: readonly QuickFilterId[] = [];
    const show = (): (() => void) =>
      watched(() => {
        seen = blocked.chosen('board-1', 'me');
      });
    let stop = show();
    blocked.toggle(boardOf('board-1'), 'overdue', 'me');
    stop();
    await tick();
    stop = show();
    expect(seen).toEqual(['overdue']);
    stop();
  });

  it('of one account go with it, and those of another stay', () => {
    localStorage.setItem(quickFiltersKey('user-1', 'a'), '["mine"]');
    localStorage.setItem(quickFiltersKey('user-2', 'a'), '["overdue"]');
    forgetUser('user-1');
    expect(localStorage.getItem(quickFiltersKey('user-1', 'a'))).toBeNull();
    expect(localStorage.getItem(quickFiltersKey('user-2', 'a'))).toBe('["overdue"]');
    localStorage.clear();
  });
});

describe('the cards the reader made', () => {
  it('are those that were not there when the row of chips was shown, and the reader made', () => {
    const choices = new QuickFilterChoices(memoryStorage);
    const board = { ...boardOf('board-1', [cardOf('old')]) };
    // Nothing noted yet: no card is let through.
    expect(choices.made(board, 'me')(cardOf('new'))).toBe(false);
    choices.opened(board);
    board.realCards = [cardOf('old'), cardOf('new'), cardOf('theirs', 'you')];
    const made = choices.made(board, 'me');
    expect(made(cardOf('old'))).toBe(false);
    expect(made(cardOf('new'))).toBe(true);
    expect(made(cardOf('theirs', 'you'))).toBe(false);
    expect(choices.made(board, undefined)(cardOf('new'))).toBe(false);
    expect(choices.made(board, '')(cardOf('new'))).toBe(false);
  });

  it('are counted anew as the chips change: a card made before is one the chips judge', () => {
    const choices = new QuickFilterChoices(memoryStorage);
    const board = { ...boardOf('board-1', [cardOf('old')]) };
    choices.opened(board);
    board.realCards = [cardOf('old'), cardOf('morning')];
    expect(choices.made(board, 'me')(cardOf('morning'))).toBe(true);
    choices.toggle(board, 'overdue', 'me');
    expect(choices.made(board, 'me')(cardOf('morning'))).toBe(false);
    board.realCards = [...board.realCards, cardOf('afternoon')];
    expect(choices.made(board, 'me')(cardOf('afternoon'))).toBe(true);
  });

  it('are counted anew as another tab changes the chips of the board, and of no other', () => {
    const choices = new QuickFilterChoices(() => localStorage);
    const board = { ...boardOf('board-1', [cardOf('old')]) };
    const other = { ...boardOf('board-2', []) };
    choices.opened(board);
    choices.opened(other);
    board.realCards = [cardOf('old'), cardOf('morning')];
    other.realCards = [cardOf('theirs-morning')];
    const stop = watched(() => {
      choices.chosen('board-1', 'me');
    });
    const tell = (key: string | null): void => {
      window.dispatchEvent(new StorageEvent('storage', { key, storageArea: localStorage }));
    };
    expect(choices.made(board, 'me')(cardOf('morning'))).toBe(true);
    localStorage.setItem(quickFiltersKey('me', 'board-1'), '["overdue"]');
    tell(quickFiltersKey('me', 'board-1'));
    // Made before the other tab's change: the chips judge it.
    expect(choices.made(board, 'me')(cardOf('morning'))).toBe(false);
    board.realCards = [...board.realCards, cardOf('afternoon')];
    expect(choices.made(board, 'me')(cardOf('afternoon'))).toBe(true);
    // The other board's chips did not change.
    expect(choices.made(other, 'me')(cardOf('theirs-morning'))).toBe(true);
    // Storage cleared changes the chips of every board.
    tell(null);
    expect(choices.made(other, 'me')(cardOf('theirs-morning'))).toBe(false);
    expect(choices.made(board, 'me')(cardOf('afternoon'))).toBe(false);
    stop();
    localStorage.clear();
  });

  it('are not counted anew as another tab clears storage, on a board whose chips it did not take', () => {
    const choices = new QuickFilterChoices(() => localStorage);
    const board = { ...boardOf('board-1', [cardOf('old')]) };
    const other = { ...boardOf('board-2', []) };
    choices.opened(other);
    const stop = watched(() => {
      choices.chosen('board-1', 'me');
    });
    const full = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('full', 'QuotaExceededError');
    });
    choices.toggle(board, 'mine', 'me');
    full.mockRestore();
    board.realCards = [cardOf('old'), cardOf('morning')];
    other.realCards = [cardOf('theirs-morning')];
    window.dispatchEvent(new StorageEvent('storage', { key: null, storageArea: localStorage }));
    // Its chips are on as they were: a card made since they changed is the reader's still.
    expect(choices.chosen('board-1', 'me')).toEqual(['mine']);
    expect(choices.made(board, 'me')(cardOf('morning'))).toBe(true);
    // The other board's chips were in storage, and are gone with it.
    expect(choices.made(other, 'me')(cardOf('theirs-morning'))).toBe(false);
    stop();
    localStorage.clear();
  });

  it('are counted on the visit their row is shown: another tab changing the chips before is no note', () => {
    const choices = new QuickFilterChoices(() => localStorage);
    const board = { ...boardOf('board-1', []) };
    choices.opened(board);
    const stop = watched(() => {
      choices.chosen('board-1', 'me');
    });
    // Another visit, its cards not in yet, when the other tab changes the chips.
    board.visit = 2;
    board.realCards = [cardOf('old')];
    window.dispatchEvent(
      new StorageEvent('storage', {
        key: quickFiltersKey('me', 'board-1'),
        storageArea: localStorage,
      }),
    );
    board.realCards = [cardOf('old'), cardOf('loaded')];
    choices.opened(board);
    expect(choices.made(board, 'me')(cardOf('loaded'))).toBe(false);
    stop();
  });

  it('are counted once a visit: the row shown again is no change, another visit is', () => {
    const choices = new QuickFilterChoices(memoryStorage);
    const board = { ...boardOf('board-1', []) };
    choices.opened(board);
    board.realCards = [cardOf('new')];
    choices.opened(board);
    expect(choices.made(board, 'me')(cardOf('new'))).toBe(true);
    board.visit = 2;
    choices.opened(board);
    expect(choices.made(board, 'me')(cardOf('new'))).toBe(false);
  });
});

describe('the cards a view shows', () => {
  const boardId = 'quick-board';
  afterEach(() => {
    quickFilters.forget();
    localStorage.clear();
  });

  const definition = {
    id: boardId,
    cardProperties: [
      {
        id: 'status',
        name: 'Status',
        type: 'select',
        options: [
          { id: 'todo', label: 'To do', color: 'blue' },
          { id: 'finished', label: 'Done', color: 'green' },
        ],
      },
      { id: 'owner', name: 'Owner', type: 'person', options: [] },
      { id: 'due', name: 'Due', type: 'date', options: [] },
    ],
    doneWhen: { kind: 'option', propertyId: 'status', optionIds: ['finished'] },
    dueDatePropertyId: 'due',
  } as unknown as Container;

  const card = (
    id: string,
    title: string,
    properties: Card['fields']['properties'],
  ): CardSummary => {
    const made = createCard();
    return {
      ...made,
      id,
      title,
      createdBy: 'someone',
      fields: { ...made.fields, properties },
      hasBody: false,
      tasksTotal: 0,
      tasksDone: 0,
    };
  };

  const view = {
    id: 'view',
    fields: {
      viewType: 'table',
      sort: [],
      filter: { operation: 'and', filters: [] },
      cardOrder: [],
    },
  } as unknown as BoardView;

  function source(cards: CardSummary[], meId = 'me'): BoardCardsSource & VisitedBoard {
    return {
      boardId,
      visit: 1,
      realCards: cards,
      board: definition,
      cardsById: new Map(cards.map((each) => [each.id, each])),
      workspace: {
        userName: () => undefined,
        me: { id: meId },
        locale: 'en',
        regional: { timeZone: 'Asia/Seoul', weekStart: 1 },
      },
    };
  }

  const CARDS = [
    card('a', 'Plan the launch', { owner: 'me', status: 'todo', due: { from: '2020-01-01' } }),
    card('b', 'Plan the party', { owner: 'you', status: 'todo' }),
    card('c', 'Write the plan', { owner: 'me', status: 'finished' }),
  ];
  const titles = (cards: readonly Card[]): string[] => cards.map((each) => each.title);

  it('are narrowed by the chips after the filter of the view, and by the search after the chips', () => {
    const shown = source(CARDS);
    expect(titles(filteredCardsOfBoardView(shown, view))).toHaveLength(3);
    quickFilters.toggle(shown, 'mine', 'me');
    expect(titles(filteredCardsOfBoardView(shown, view))).toEqual([
      'Plan the launch',
      'Write the plan',
    ]);
    quickFilters.toggle(shown, 'incomplete', 'me');
    expect(titles(cardsOfBoardView(shown, view))).toEqual(['Plan the launch']);
    expect(titles(cardsOfBoardView(shown, view, 'party'))).toEqual([]);
    quickFilters.clear(shown, 'me');
    expect(titles(cardsOfBoardView(shown, view, 'party'))).toEqual(['Plan the party']);
  });

  it('keep a card the reader made after the chips changed, and not one made before or by another', () => {
    const shown = source(CARDS);
    const before = { ...card('d', 'Made before', {}), createdBy: 'me' };
    Object.assign(shown, { realCards: [...CARDS, before] });
    quickFilters.toggle(shown, 'overdue', 'me');
    const mine = { ...card('e', 'Fresh', {}), createdBy: 'me' };
    const theirs = card('f', 'Theirs', {});
    // The same board store, its cards changed.
    const later = [...CARDS, before, mine, theirs];
    Object.assign(shown, { realCards: later, cardsById: new Map(later.map((c) => [c.id, c])) });
    expect(titles(filteredCardsOfBoardView(shown, view))).toEqual(['Plan the launch', 'Fresh']);
    expect(mine.fields.properties).toEqual({});
  });

  it('are not narrowed on a public link by chips kept in this browser', () => {
    localStorage.setItem(quickFiltersKey('', boardId), '["incomplete"]');
    localStorage.setItem(quickFiltersKey('me', boardId), '["incomplete"]');
    expect(titles(filteredCardsOfBoardView(source(CARDS, ''), view))).toHaveLength(3);
    expect(titles(filteredCardsOfBoardView(source(CARDS), view))).toHaveLength(2);
  });

  it('take today from the clock for the due chips', () => {
    const shown = source([
      card('x', 'Due long ago', { due: { from: '2020-01-01' } }),
      card('y', 'Due far off', { due: { from: '2999-01-01' } }),
    ]);
    quickFilters.toggle(shown, 'overdue', 'me');
    expect(titles(cardsOfBoardView(shown, view))).toEqual(['Due long ago']);
  });
});
