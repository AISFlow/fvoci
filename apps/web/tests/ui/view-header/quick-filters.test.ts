/**
 * The quick filters under the header of a view, over the real stores and
 * the in-memory backend: the chips a board offers, what pressing one does
 * to the cards of the view and to nothing else, the button that turns them
 * off, another tab that changes them, and a card the reader makes while they
 * are on.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import { quickFilters, quickFiltersKey } from '$lib/state/board-quick.svelte.ts';
import type { BoardStore } from '$lib/state/index.ts';

import { openFirstBoard, OWNER, setup, STATUS } from '../../state/support.ts';

import Harness from './Harness.svelte';

afterEach(() => {
  quickFilters.forget();
  localStorage.clear();
  cleanup();
});

/**
 * The small board, given a rule of done (the option "Done") and a due date.
 * `before` is done to it before it is shown.
 */
async function mount(before?: (board: BoardStore) => void): Promise<BoardStore> {
  const { board } = await openFirstBoard(setup());
  const definition = board.board;
  if (!definition) throw new Error('no board');
  board.board = {
    ...definition,
    cardProperties: [
      ...definition.cardProperties,
      { id: 'due', name: 'Due', type: 'date', options: [] },
    ],
    doneWhen: { kind: 'option', propertyId: STATUS, optionIds: ['done'] },
    dueDatePropertyId: 'due',
  };
  // The seed gives its views identifiers of its own: the view of the small board is "Board".
  viewId = board.views.find((view) => view.title === 'Board')?.id ?? '';
  before?.(board);
  render(Harness, { props: { board, viewId } });
  return board;
}
let viewId = '';

const row = (): HTMLElement => screen.getByTestId('quick-filters');
const chip = (name: string): HTMLElement => within(row()).getByRole('button', { name });
/** Where the reader's chips of the board are kept. */
const keyOf = (board: BoardStore): string =>
  quickFiltersKey(board.workspace.me?.id ?? '', board.boardId);
const titles = (board: BoardStore): string[] => {
  const view = board.view(viewId);
  return view ? board.filteredCardsOf(view).map((card) => card.title) : [];
};

describe('the quick filters', () => {
  it('are the chips the board has what for, none pressed, in a row of their own', async () => {
    await mount();
    expect(row()).toHaveAccessibleName('Quick filters');
    expect(screen.getByTestId('view-header')).not.toContainElement(row());
    const chips = within(row()).getAllByRole('button');
    expect(chips.map((each) => each.textContent.trim())).toEqual([
      'Mine',
      'Not done',
      'Overdue',
      'Due this week',
    ]);
    for (const each of chips) expect(each).toHaveAttribute('aria-pressed', 'false');
    // Nothing of them is a tool of the view.
    expect(row().querySelector('[data-tool]')).toBeNull();
  });

  it('narrow the cards of the view as they are pressed, and leave the view as it was', async () => {
    const board = await mount();
    board.cards = board.cards.map((card) =>
      card.title === 'Gamma'
        ? { ...card, fields: { ...card.fields, properties: { [STATUS]: 'done' } } }
        : card,
    );
    const before = board.view(viewId);
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Gamma']);

    await fireEvent.click(chip('Not done'));
    expect(chip('Not done')).toHaveAttribute('aria-pressed', 'true');
    expect(titles(board)).toEqual(['Alpha', 'Beta']);
    expect(board.view(viewId)).toBe(before);
    expect(localStorage.getItem(keyOf(board))).toBe('["incomplete"]');

    await fireEvent.click(chip('Overdue'));
    expect(titles(board)).toEqual([]);
    const clear = within(row()).getByRole('button', { name: 'Clear quick filters (2 on)' });
    expect(clear).toHaveTextContent('2');

    clear.focus();
    await fireEvent.click(clear);
    expect(chip('Not done')).toHaveAttribute('aria-pressed', 'false');
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Gamma']);
    expect(within(row()).queryByRole('button', { name: /^Clear/ })).toBeNull();
    expect(localStorage.getItem(keyOf(board))).toBeNull();
    // The button pressed is gone: the focus is on the first chip, not lost.
    await waitFor(() => {
      expect(chip('Mine')).toHaveFocus();
    });
  });

  it('let a card the reader makes while they are on stay, without values given to meet them', async () => {
    const board = await mount();
    await fireEvent.click(chip('Mine'));
    const mine = titles(board);
    expect(mine.length).toBeLessThan(3);
    const made = await board.createCard({ title: 'Fresh' });
    expect(made?.fields.properties[OWNER]).toBeUndefined();
    expect(titles(board)).toEqual([...mine, 'Fresh']);
  });

  it('kept on from before let a card the reader makes stay, with no chip pressed first', async () => {
    const board = await mount((shown) => {
      localStorage.setItem(keyOf(shown), '["overdue"]');
    });
    expect(chip('Overdue')).toHaveAttribute('aria-pressed', 'true');
    expect(titles(board)).toEqual([]);
    await board.createCard({ title: 'Fresh' });
    expect(titles(board)).toEqual(['Fresh']);
  });

  it('follow another tab that turns them on or off', async () => {
    const board = await mount();
    // Another tab of the same reader writes the chips of the board, and says so.
    localStorage.setItem(keyOf(board), '["incomplete"]');
    window.dispatchEvent(
      new StorageEvent('storage', { key: keyOf(board), storageArea: localStorage }),
    );
    await waitFor(() => {
      expect(chip('Not done')).toHaveAttribute('aria-pressed', 'true');
    });
    expect(within(row()).getByRole('button', { name: 'Clear quick filters (1 on)' })).toBeVisible();
    // One this tab turned on before goes as the other tab turns them off.
    await fireEvent.click(chip('Overdue'));
    localStorage.removeItem(keyOf(board));
    window.dispatchEvent(
      new StorageEvent('storage', { key: keyOf(board), storageArea: localStorage }),
    );
    await waitFor(() => {
      expect(chip('Overdue')).toHaveAttribute('aria-pressed', 'false');
    });
    expect(chip('Not done')).toHaveAttribute('aria-pressed', 'false');
  });

  it('take the first date property for the due date of a board that names none', async () => {
    const board = await mount();
    const definition = board.board;
    if (!definition) throw new Error('no board');
    // "Automatic": the server's due date is then the board's first date property.
    board.board = { ...definition, dueDatePropertyId: null };
    await Promise.resolve();
    expect(within(row()).queryByRole('button', { name: 'Overdue' })).not.toBeNull();
  });

  it('are offered without "Not done" and the due ones on a board that has no rule and no date', async () => {
    const board = await mount();
    const definition = board.board;
    if (!definition) throw new Error('no board');
    await fireEvent.click(chip('Overdue'));
    board.board = {
      ...definition,
      cardProperties: definition.cardProperties.filter((property) => property.type !== 'date'),
      doneWhen: null,
      dueDatePropertyId: null,
    };
    await Promise.resolve();
    expect(
      within(row())
        .getAllByRole('button')
        .map((each) => each.textContent.trim()),
    ).toEqual(['Mine']);
    // The chip that is not shown narrows nothing.
    expect(titles(board)).toEqual(['Alpha', 'Beta', 'Gamma']);
  });
});
