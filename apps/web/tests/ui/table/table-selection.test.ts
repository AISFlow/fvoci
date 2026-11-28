/**
 * Selecting rows of the table and changing them together through the bar of
 * the selection (features/views/BulkBar.svelte), over a board store on the
 * in-memory backend. Where the bar sits and how it fits a phone need a
 * browser and are in e2e/bulk.spec.ts.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeAll, describe, expect, it, onTestFinished, vi } from 'vitest';

import { pane } from '$lib/layout/room.svelte.ts';
import type { BoardStore } from '$lib/state/index.ts';
import type { SeedTemplate } from '$lib/state/memory/seed.ts';

import { openFirstBoard, setup, settle } from '../../state/support.ts';

import TableHarness from './TableHarness.svelte';

/** A board with one table view: a select and a text column, three cards. */
const SEED: SeedTemplate = {
  key: 'table-selection',
  onboarding: false,
  board: {
    id: 'b',
    title: 'Table',
    description: '',
    icon: '',
    showDescription: false,
    cardProperties: [
      {
        id: 'status',
        name: 'Status',
        type: 'select',
        options: [
          { id: 'todo', label: 'To do', color: 'blue' },
          { id: 'done', label: 'Done', color: 'green' },
        ],
      },
      { id: 'note', name: 'Note', type: 'text', options: [] },
    ],
  },
  views: [
    {
      id: 'v',
      title: 'All',
      fields: {
        viewType: 'table',
        shownProperties: ['status', 'note'],
        visibleGroups: [],
        hiddenGroups: [],
        collapsedGroups: [],
        sort: [],
        cardOrder: ['c2', 'c1', 'c3'],
        columnWidths: {},
        columnCalculations: {},
        filter: { operation: 'and', filters: [] },
      },
    },
    {
      id: 'w',
      title: 'Other',
      fields: {
        viewType: 'table',
        shownProperties: ['status', 'note'],
        visibleGroups: [],
        hiddenGroups: [],
        collapsedGroups: [],
        sort: [],
        cardOrder: ['c2', 'c1', 'c3'],
        columnWidths: {},
        columnCalculations: {},
        filter: { operation: 'and', filters: [] },
      },
    },
    {
      id: 'f',
      title: 'To do',
      fields: {
        viewType: 'table',
        shownProperties: ['status', 'note'],
        visibleGroups: [],
        hiddenGroups: [],
        collapsedGroups: [],
        sort: [],
        cardOrder: ['c2', 'c1', 'c3'],
        columnWidths: {},
        columnCalculations: {},
        filter: {
          operation: 'and',
          filters: [{ propertyId: 'status', condition: 'includes', values: ['todo'] }],
        },
      },
    },
  ],
  cards: [
    {
      id: 'c1',
      title: 'Alpha',
      fields: { icon: '', isTemplate: false, properties: { status: 'todo', note: 'first' } },
      body: null,
    },
    {
      id: 'c2',
      title: 'Beta',
      fields: { icon: '', isTemplate: false, properties: { status: 'done' } },
      body: null,
    },
    {
      id: 'c3',
      title: 'Gamma',
      fields: { icon: '', isTemplate: false, properties: { status: 'todo' } },
      body: null,
    },
  ],
  files: [],
};

let rerenderTable: ((viewId: string) => Promise<void>) | undefined;

async function mount(title = 'All'): Promise<BoardStore> {
  const { board } = await openFirstBoard(setup([SEED]));
  const viewId = board.orderedViews.find((view) => view.title === title)?.id ?? '';
  const { rerender } = render(TableHarness, { props: { board, viewId } });
  rerenderTable = (next) => rerender({ board, viewId: next });
  await tick();
  return board;
}

const grid = (): HTMLElement => screen.getByRole('grid');
const rowOf = (title: string): HTMLElement => {
  const row = [...grid().querySelectorAll<HTMLElement>('[data-card]')].find(
    (each) => each.querySelector('[data-slot="row-title"]')?.textContent === title,
  );
  if (!row) throw new Error(`no row "${title}"`);
  return row;
};
const cellOf = (title: string, column: string): HTMLElement => {
  const cell = rowOf(title).querySelector<HTMLElement>(`[data-cell][data-col="${column}"]`);
  if (!cell) throw new Error(`no cell "${column}" in "${title}"`);
  return cell;
};
const titleOf = (title: string): HTMLElement => {
  const cell = rowOf(title).querySelector<HTMLElement>('[data-cell][data-kind="title"]');
  if (!cell) throw new Error(`no title cell in "${title}"`);
  return cell;
};
const selected = (): string[] =>
  [...grid().querySelectorAll('[data-card][data-selected] [data-slot="row-title"]')].map(
    (title) => title.textContent,
  );
const bar = (): HTMLElement | null => screen.queryByTestId('selection-bar');
/** The element `selector` finds in `root`; the test fails where there is none. */
const one = (root: ParentNode | null | undefined, selector: string): HTMLElement => {
  const found = root?.querySelector<HTMLElement>(selector);
  if (!found) throw new Error(`nothing is ${selector}`);
  return found;
};
const statusOf = (board: BoardStore, title: string): unknown =>
  board.cards.find((card) => card.title === title)?.fields.properties.status;

beforeAll(() => {
  vi.stubGlobal('CSS', { escape: (value: string) => value });
  Element.prototype.scrollIntoView = () => undefined;
});

afterEach(() => {
  cleanup();
});

describe('selecting rows', () => {
  it('takes a row in with Ctrl, a range with Shift, and lets one go with Ctrl again', async () => {
    await mount();
    expect(bar()).toBeNull();
    await fireEvent.click(cellOf('Beta', 'note'), { ctrlKey: true });
    expect(selected()).toEqual(['Beta']);
    expect(bar()?.textContent).toContain('1 selected');
    await fireEvent.click(titleOf('Gamma'), { shiftKey: true });
    expect(selected()).toEqual(['Beta', 'Alpha', 'Gamma']);
    await fireEvent.click(cellOf('Alpha', 'status'), { ctrlKey: true });
    expect(selected()).toEqual(['Beta', 'Gamma']);
    expect(bar()?.textContent).toContain('2 selected');
    // Selecting edited nothing.
    expect(grid().querySelector('[data-editing]')).toBeNull();
  });

  it('tells a screen reader which rows are selected, and that X selects', async () => {
    await mount();
    expect(grid().getAttribute('aria-multiselectable')).toBe('true');
    expect(
      document.getElementById(grid().getAttribute('aria-describedby') ?? '')?.textContent,
    ).toContain('X selects the row of a cell');
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    expect(rowOf('Alpha').getAttribute('aria-selected')).toBe('true');
    expect(rowOf('Beta').getAttribute('aria-selected')).toBe('false');
  });

  it('takes the row of a cell in with X, and lets the selection go with Escape', async () => {
    await mount();
    const cell = cellOf('Alpha', 'note');
    cell.focus();
    await fireEvent.keyDown(cell, { key: 'x' });
    expect(selected()).toEqual(['Alpha']);
    await fireEvent.keyDown(cell, { key: 'x', isComposing: true });
    expect(selected()).toEqual(['Alpha']);
    // On Hangul's layout the key where X is gives ㅌ; on Dvorak's that key is Q, and X is elsewhere.
    await fireEvent.keyDown(cellOf('Beta', 'note'), { key: 'ㅌ', code: 'KeyX' });
    expect(selected()).toEqual(['Beta', 'Alpha']);
    await fireEvent.keyDown(cellOf('Gamma', 'note'), { key: 'q', code: 'KeyX' });
    expect(selected()).toEqual(['Beta', 'Alpha']);
    await fireEvent.keyDown(cell, { key: 'Escape' });
    expect(selected()).toEqual([]);
    expect(bar()).toBeNull();
  });

  it('keeps the selection when Escape only ends the editing of a cell', async () => {
    await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(titleOf('Beta'), { ctrlKey: true });
    const cell = cellOf('Gamma', 'note');
    cell.focus();
    await fireEvent.keyDown(cell, { key: 'Enter' });
    const field = await waitFor(() => one(cellOf('Gamma', 'note'), 'input'));
    await fireEvent.keyDown(field, { key: 'Escape' });
    await waitFor(() => {
      expect(grid().querySelector('[data-editing]')).toBeNull();
    });
    expect(selected()).toEqual(['Beta', 'Alpha']);
    // The next Escape, on the cell, is the selection's.
    await fireEvent.keyDown(cellOf('Gamma', 'note'), { key: 'Escape' });
    expect(selected()).toEqual([]);
  });

  it('by the box of a row, and by "Select" in its menu, after which a plain press selects', async () => {
    await mount();
    const box = one(rowOf('Gamma'), '[data-select-row]');
    expect(box.getAttribute('aria-checked')).toBe('false');
    await fireEvent.click(box);
    expect(selected()).toEqual(['Gamma']);
    expect(box.getAttribute('aria-checked')).toBe('true');
    await fireEvent.click(box);
    expect(selected()).toEqual([]);

    await fireEvent.click(one(rowOf('Beta'), '[data-more]'));
    const menu = await screen.findByRole('menu');
    await fireEvent.click(within(menu).getByText('Select'));
    await waitFor(() => {
      expect(selected()).toEqual(['Beta']);
    });
    await fireEvent.click(cellOf('Alpha', 'note'));
    expect(selected()).toEqual(['Beta', 'Alpha']);
    expect(grid().querySelector('[data-editing]')).toBeNull();
  });
});

describe('a row that leaves the view', () => {
  it('leaves the selection, and comes back unselected', async () => {
    const board = await mount('To do');
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(one(bar(), '[data-bulk-property="status"]'));
    await fireEvent.click(await screen.findByRole('option', { name: /Done/ }));
    // Done is not to do: the row leaves the view, and the bar with it.
    await waitFor(() => {
      expect(bar()).toBeNull();
    });
    await settle();
    await board.workspace.undoLast();
    await waitFor(() => {
      expect(statusOf(board, 'Alpha')).toBe('todo');
    });
    await tick();
    expect(selected()).toEqual([]);
    expect(bar()).toBeNull();
    // A plain press is the row's again.
    await fireEvent.click(cellOf('Gamma', 'note'));
    expect(selected()).toEqual([]);
  });
});

describe('another view of the board', () => {
  it('starts with nothing selected', async () => {
    const board = await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    expect(bar()).not.toBeNull();
    const other = board.orderedViews.find((view) => view.title === 'Other')?.id;
    if (!other || !rerenderTable) throw new Error('no second view');
    await rerenderTable(other);
    await tick();
    expect(bar()).toBeNull();
    expect(selected()).toEqual([]);
  });
});

describe('the bar of the selection', () => {
  it('sets a value on every selected row in one step of undo', async () => {
    const board = await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(titleOf('Gamma'), { ctrlKey: true });
    const status = one(bar(), '[data-bulk-property="status"]');
    expect(status.getAttribute('title')).toBe('Status');
    await fireEvent.click(status);
    const done = await screen.findByRole('option', { name: /Done/ });
    await fireEvent.click(done);
    await waitFor(() => {
      expect(statusOf(board, 'Alpha')).toBe('done');
      expect(statusOf(board, 'Gamma')).toBe('done');
    });
    await settle();
    expect(statusOf(board, 'Beta')).toBe('done');

    await board.workspace.undoLast();
    await waitFor(() => {
      expect(statusOf(board, 'Alpha')).toBe('todo');
      expect(statusOf(board, 'Gamma')).toBe('todo');
    });
    expect(board.workspace.undo.canUndo).toBe(false);
  });

  it('says where the selected values differ', async () => {
    await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(titleOf('Beta'), { shiftKey: true });
    expect(one(bar(), '[data-bulk-property="status"]').getAttribute('title')).toBe(
      'Status: mixed values',
    );
  });

  it('clears a text property on every selected row, whatever was typed before "Clear"', async () => {
    const board = await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(titleOf('Gamma'), { ctrlKey: true });
    await fireEvent.click(one(bar(), '[data-bulk-menu]'));
    const menu = await screen.findByRole('menu');
    await fireEvent.click(within(menu).getByText('Note'));
    const field = await waitFor(() => one(document, '[data-slot="bulk-field"] input'));
    await fireEvent.input(field, { target: { value: 'typed' } });
    const clear = within(one(document, '[data-slot="bulk-field"]')).getByRole('button', {
      name: 'Clear',
    });
    // The press keeps the focus in the field, which would save "typed" when left.
    expect(await fireEvent.mouseDown(clear)).toBe(false);
    await fireEvent.click(clear);
    // A field that is left as the surface closes does not put its text over the clearing.
    if (field.isConnected) await fireEvent.blur(field);
    await settle();
    const noteOf = (title: string): unknown =>
      board.cards.find((card) => card.title === title)?.fields.properties.note;
    expect(noteOf('Alpha')).toBeUndefined();
    expect(noteOf('Gamma')).toBeUndefined();
  });

  it('gives the focus back to "More" when a property chosen there is closed, its button not seen', async () => {
    // A narrow pane: the bar keeps its icons and the rest under "More".
    const root = document.documentElement.style;
    root.setProperty('--container-view-row', '40rem');
    root.fontSize = '16px';
    pane.width = 320;
    onTestFinished(() => {
      root.removeProperty('--container-view-row');
      root.fontSize = '';
      pane.width = 0;
    });
    await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(titleOf('Gamma'), { ctrlKey: true });
    const more = one(bar(), '[data-bulk-menu]');
    await fireEvent.click(more);
    const menu = await screen.findByRole('menu');
    await fireEvent.click(within(menu).getByText('Note'));
    const field = await waitFor(() => one(document, '[data-slot="bulk-field"] input'));
    const opener = one(bar(), '[data-bulk-property="note"]');
    expect(opener.classList.contains('sr-only')).toBe(true);
    field.focus();
    await fireEvent.click(
      within(one(document, '[data-slot="bulk-field"]')).getByRole('button', { name: 'Clear' }),
    );
    await waitFor(() => {
      expect(opener.getAttribute('aria-expanded')).toBe('false');
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(one(bar(), '[data-bulk-menu]'));
    });
  });

  it('gives the focus back to a selected row when ✕ lets the selection go', async () => {
    await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(titleOf('Gamma'), { ctrlKey: true });
    const clear = one(bar(), '[data-bulk-clear]');
    clear.focus();
    await fireEvent.click(clear);
    await waitFor(() => {
      expect(bar()).toBeNull();
    });
    await tick();
    // Alpha is shown above Gamma: its first cell, the title.
    expect(document.activeElement).toBe(titleOf('Alpha'));
  });

  it('gives the focus to a row left when Delete took the selected ones', async () => {
    await mount();
    await fireEvent.click(titleOf('Beta'), { ctrlKey: true });
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    const remove = one(bar(), '[data-bulk-delete]');
    remove.focus();
    await fireEvent.click(remove);
    await waitFor(() => {
      expect(bar()).toBeNull();
    });
    await tick();
    expect(document.activeElement).toBe(titleOf('Gamma'));
  });

  it('gives the focus to the cell that adds a row when Delete took every row', async () => {
    await mount();
    for (const title of ['Alpha', 'Beta', 'Gamma']) {
      await fireEvent.click(titleOf(title), { ctrlKey: true });
    }
    const remove = one(bar(), '[data-bulk-delete]');
    remove.focus();
    await fireEvent.click(remove);
    await waitFor(() => {
      expect(bar()).toBeNull();
    });
    await tick();
    expect(document.activeElement).toBe(one(document, '[data-cell][data-kind="add"]'));
  });

  it('deletes the selected rows in one step, and lets the selection go', async () => {
    const board = await mount();
    await fireEvent.click(titleOf('Alpha'), { ctrlKey: true });
    await fireEvent.click(titleOf('Beta'), { ctrlKey: true });
    // Without layout the view counts as wide: Delete is a button of the bar, not a line of "More".
    await fireEvent.click(one(bar(), '[data-bulk-delete]'));
    await waitFor(() => {
      expect(board.cards.map((card) => card.title)).toEqual(['Gamma']);
    });
    expect(bar()).toBeNull();
    await settle();
    await board.workspace.undoLast();
    await waitFor(() => {
      expect(board.cards).toHaveLength(3);
    });
  });
});
