/**
 * The table over a board store on the in-memory backend, in a simulated page
 * without layout: what it draws, what a press and a key lead to, and what
 * the store has afterwards. Sizes, scrolling and dragging need a browser and
 * are in e2e/table.spec.ts.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';

import type { SeedTemplate } from '$lib/state/memory/seed.ts';
import type { BoardStore } from '$lib/state/index.ts';

import { openFirstBoard, setup, settle } from '../../state/support.ts';

import TableHarness from './TableHarness.svelte';

const OPTIONS: SeedTemplate['board']['cardProperties'][number]['options'] = [
  { id: 'todo', label: 'To do', color: 'blue' },
  { id: 'done', label: 'Done', color: 'green' },
];

/** A board with one table view: a select, a text, a number and a checkbox column, three cards. */
function template(overrides: Record<string, unknown> = {}, key = 'table'): SeedTemplate {
  return {
    key,
    onboarding: false,
    board: {
      id: 'b',
      title: 'Table',
      description: '',
      icon: '',
      showDescription: false,
      cardProperties: [
        { id: 'status', name: 'Status', type: 'select', options: OPTIONS },
        { id: 'note', name: 'Note', type: 'text', options: [] },
        { id: 'hours', name: 'Hours', type: 'number', options: [] },
        { id: 'ready', name: 'Ready', type: 'checkbox', options: [] },
        { id: 'made', name: 'Made', type: 'createdTime', options: [] },
      ],
    },
    views: [
      {
        id: 'v',
        title: 'All',
        fields: {
          viewType: 'table',
          shownProperties: ['status', 'note', 'hours', 'ready', 'made'],
          visibleGroups: [],
          hiddenGroups: [],
          collapsedGroups: [],
          sort: [],
          cardOrder: ['c2', 'c1', 'c3'],
          columnWidths: {},
          columnCalculations: { hours: 'sum' },
          filter: { operation: 'and', filters: [] },
          ...overrides,
        },
      },
    ],
    cards: [
      {
        id: 'c1',
        title: 'Alpha',
        fields: {
          icon: '',
          isTemplate: false,
          properties: { status: 'todo', note: 'first', hours: '3' },
        },
        body: null,
      },
      {
        id: 'c2',
        title: 'Beta',
        fields: {
          icon: '',
          isTemplate: false,
          properties: { status: 'done', hours: '4', ready: 'true' },
        },
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
}

interface Mounted {
  board: BoardStore;
  viewId: string;
  onOpenCard: ReturnType<typeof vi.fn<(cardId: string) => void>>;
}

async function mount(seed: SeedTemplate = template()): Promise<Mounted> {
  const { board } = await openFirstBoard(setup([seed]));
  const viewId = board.orderedViews[0]?.id ?? '';
  const onOpenCard = vi.fn<(cardId: string) => void>();
  render(TableHarness, { props: { board, viewId, onOpenCard } });
  await tick();
  return { board, viewId, onOpenCard };
}

const grid = (): HTMLElement => screen.getByRole('grid');
const titles = (): string[] =>
  [...grid().querySelectorAll('[data-slot="row-title"]')].map((title) => title.textContent);
const rowOf = (title: string): HTMLElement => {
  const row = [...grid().querySelectorAll<HTMLElement>('[data-nav-row]')].find(
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
const cardOf = (board: BoardStore, title: string): BoardStore['cards'][number] => {
  const card = board.cards.find((each) => each.title === title);
  if (!card) throw new Error(`no card "${title}"`);
  return card;
};

beforeAll(() => {
  vi.stubGlobal('CSS', { escape: (value: string) => value });
  Element.prototype.scrollIntoView = () => undefined;
});

afterEach(() => {
  cleanup();
});

describe('what the table draws', () => {
  it('a heading per column of the view, the title column first', async () => {
    await mount();
    const headings = within(grid())
      .getAllByRole('columnheader')
      .map((heading) => heading.getAttribute('aria-label'));
    expect(headings).toEqual(['Name', 'Status', 'Note', 'Hours', 'Ready', 'Made']);
  });

  it('a row per card, in the manual order of the view', async () => {
    await mount();
    expect(titles()).toEqual(['Beta', 'Alpha', 'Gamma']);
  });

  it('the values, for reading: no field and no picker until a cell is pressed', async () => {
    await mount();
    expect(cellOf('Alpha', 'status')).toHaveTextContent('To do');
    expect(cellOf('Alpha', 'note')).toHaveTextContent('first');
    expect(cellOf('Beta', 'hours')).toHaveTextContent('4');
    expect(grid().querySelectorAll('input')).toHaveLength(0);
    expect(grid().querySelectorAll('button[data-slot="property-select"]')).toHaveLength(0);
  });

  it('the calculation of each column over the cards shown', async () => {
    await mount();
    const summary = grid().querySelector<HTMLElement>('[data-row="calculations"]');
    expect(summary?.querySelector('[data-col="__title"]')).toHaveTextContent(/Count\s*3/);
    expect(summary?.querySelector('[data-col="hours"]')).toHaveTextContent(/Sum\s*7/);
    expect(summary?.querySelector('[data-col="status"]')).toHaveAttribute(
      'data-calculation',
      'none',
    );
  });

  it('a calculation the kind of the column cannot have is no calculation', async () => {
    await mount(template({ columnCalculations: { status: 'sum', note: 'somethingElse' } }));
    const summary = grid().querySelector<HTMLElement>('[data-row="calculations"]');
    expect(summary?.querySelector('[data-col="status"]')).toHaveAttribute(
      'data-calculation',
      'none',
    );
    expect(summary?.querySelector('[data-col="note"]')).toHaveAttribute('data-calculation', 'none');
  });

  it('sorted rows when the view has a sort, and then no row can be dragged', async () => {
    await mount(template({ sort: [{ propertyId: '__title', reversed: true }] }));
    expect(titles()).toEqual(['Gamma', 'Beta', 'Alpha']);
    expect(grid().querySelectorAll('[data-nav-row][data-drag-item]')).toHaveLength(0);
    expect(within(grid()).getByRole('columnheader', { name: 'Name' })).toHaveAttribute(
      'aria-sort',
      'descending',
    );
  });

  it('rows that can be dragged by their title when the view has no sort', async () => {
    await mount();
    expect(grid().querySelectorAll('[data-nav-row][data-drag-item]')).toHaveLength(3);
    expect(rowOf('Alpha').querySelector('[data-kind="title"]')).toHaveAttribute('data-drag-handle');
  });
});

describe('pressing a cell', () => {
  it('of a text gives its field, focused; Enter stores the text and the cell shows again', async () => {
    const { board } = await mount();
    await fireEvent.click(cellOf('Alpha', 'note'));
    const field = within(cellOf('Alpha', 'note')).getByRole('textbox');
    expect(field).toHaveFocus();
    expect(grid().querySelectorAll('input')).toHaveLength(1);

    await fireEvent.input(field, { target: { value: 'second' } });
    await fireEvent.keyDown(field, { key: 'Enter' });
    await waitFor(() => {
      expect(grid().querySelectorAll('input')).toHaveLength(0);
    });
    expect(cellOf('Alpha', 'note')).toHaveTextContent('second');
    expect(cardOf(board, 'Alpha').fields.properties.note).toBe('second');
    // The keyboard continues from the cell.
    expect(cellOf('Alpha', 'note')).toHaveFocus();
  });

  it('of another row moves the one editor there', async () => {
    await mount();
    await fireEvent.click(cellOf('Alpha', 'note'));
    await fireEvent.click(cellOf('Gamma', 'note'));
    await waitFor(() => {
      expect(within(cellOf('Gamma', 'note')).getByRole('textbox')).toBeInTheDocument();
    });
    expect(grid().querySelectorAll('input')).toHaveLength(1);
  });

  it('of a checkbox changes it at once, without an editor', async () => {
    const { board } = await mount();
    await fireEvent.click(cellOf('Alpha', 'ready'));
    await waitFor(() => {
      expect(cardOf(board, 'Alpha').fields.properties.ready).toBe('true');
    });
    await fireEvent.click(cellOf('Beta', 'ready'));
    await waitFor(() => {
      expect(cardOf(board, 'Beta').fields.properties.ready).toBeUndefined();
    });
  });

  it('of a select opens its options', async () => {
    await mount();
    await fireEvent.click(cellOf('Gamma', 'status'));
    await waitFor(() => {
      expect(cellOf('Gamma', 'status').querySelector('[aria-expanded="true"]')).not.toBeNull();
    });
  });

  it('of a value the card itself gives does nothing', async () => {
    await mount();
    await fireEvent.click(cellOf('Alpha', 'made'));
    await tick();
    expect(grid().querySelectorAll('input')).toHaveLength(0);
    expect(grid().querySelector('[data-editing]')).toBeNull();
  });

  it('of the title edits it in place', async () => {
    const { board } = await mount();
    const title = rowOf('Alpha').querySelector<HTMLElement>('[data-kind="title"]');
    if (!title) throw new Error('no title cell');
    await fireEvent.click(title);
    const field = within(title).getByRole('textbox', { name: 'Name' });
    expect(field).toHaveValue('Alpha');
    await fireEvent.input(field, { target: { value: 'Alpha one' } });
    await fireEvent.keyDown(field, { key: 'Enter' });
    await waitFor(() => {
      expect(titles()).toContain('Alpha one');
    });
    expect(cardOf(board, 'Alpha one').title).toBe('Alpha one');
  });

  it('"Open" opens the card and does not start editing the title', async () => {
    const { board, onOpenCard } = await mount();
    const open = rowOf('Alpha').querySelector<HTMLElement>('a[data-open-card]');
    if (!open) throw new Error('no link to the card');
    expect(open).toHaveAttribute('href', `/card/${cardOf(board, 'Alpha').id}`);
    await fireEvent.click(open);
    expect(onOpenCard).toHaveBeenCalledExactlyOnceWith(cardOf(board, 'Alpha').id);
    expect(grid().querySelectorAll('input')).toHaveLength(0);
  });
});

describe('rows', () => {
  it('"+ New" makes a card and puts the caret in the title of its row', async () => {
    const { board } = await mount();
    await fireEvent.click(within(grid()).getByRole('button', { name: '+ New' }));
    await waitFor(() => {
      expect(board.realCards).toHaveLength(4);
    });
    await waitFor(() => {
      const field = grid().querySelector('[data-kind="title"] input');
      expect(field).not.toBeNull();
      expect(field).toHaveFocus();
    });
    // It is the last row: a card nobody placed comes after the placed ones.
    const rows = [...grid().querySelectorAll('[data-kind="title"]')];
    expect(rows.at(-1)?.querySelector('input')).not.toBeNull();
  });

  it('a new card of a filtered view gets the value the filter asks for', async () => {
    const filter = {
      operation: 'and',
      filters: [{ propertyId: 'status', condition: 'includes', values: ['done'] }],
    };
    const { board } = await mount(template({ filter }));
    expect(titles()).toEqual(['Beta']);
    await fireEvent.click(within(grid()).getByRole('button', { name: '+ New' }));
    await waitFor(() => {
      expect(board.realCards).toHaveLength(4);
    });
    const made = board.realCards.find((card) => card.title === '');
    expect(made?.fields.properties.status).toBe('done');
  });
});

describe('groups', () => {
  const grouped = {
    groupBy: 'status',
    visibleGroups: ['todo', 'done'],
    hiddenGroups: [''],
  };

  it('are drawn in the order of the view, each with its cards and their number', async () => {
    await mount(template(grouped));
    const groups = [...grid().querySelectorAll<HTMLElement>('[data-group-header]')];
    expect(
      groups.map((group) => group.querySelector('[data-slot="group-name"]')?.textContent.trim()),
    ).toEqual(['To do', 'Done']);
    expect(
      groups.map(
        (group) => group.querySelector('[data-slot="group-count"] [aria-hidden]')?.textContent,
      ),
    ).toEqual(['2', '1']);
    expect(titles()).toEqual(['Alpha', 'Gamma', 'Beta']);
  });

  it('a hidden group is not drawn, and the calculation still counts its cards', async () => {
    await mount(template({ ...grouped, visibleGroups: ['todo'], hiddenGroups: ['done', ''] }));
    expect(titles()).toEqual(['Alpha', 'Gamma']);
    expect(
      grid().querySelector('[data-row="calculations"] [data-col="__title"]'),
    ).toHaveTextContent(/Count\s*3/);
  });

  it('folding one hides its rows and is stored in the view', async () => {
    const { board, viewId } = await mount(template(grouped));
    await fireEvent.click(within(grid()).getByRole('button', { name: 'Collapse To do' }));
    await waitFor(() => {
      expect(titles()).toEqual(['Beta']);
    });
    expect(board.view(viewId)?.fields.collapsedGroups).toEqual(['todo']);
    await fireEvent.click(within(grid()).getByRole('button', { name: 'Expand To do' }));
    await waitFor(() => {
      expect(titles()).toEqual(['Alpha', 'Gamma', 'Beta']);
    });
  });

  it('"+" on one makes a card with its value, and unfolds it so that the card shows', async () => {
    const { board } = await mount(template({ ...grouped, collapsedGroups: ['done'] }));
    expect(titles()).toEqual(['Alpha', 'Gamma']);
    await fireEvent.click(within(grid()).getByRole('button', { name: 'New card in Done' }));
    await waitFor(() => {
      expect(board.realCards).toHaveLength(4);
    });
    expect(board.realCards.find((card) => card.title === '')?.fields.properties.status).toBe(
      'done',
    );
    await waitFor(() => {
      expect(grid().querySelector('[data-list="group:done"] input')).toHaveFocus();
    });
  });
});

describe('a reader who may only look', () => {
  // The seed makes the reader a viewer of the board with this key.
  const viewOnly = (overrides: Record<string, unknown> = {}): SeedTemplate =>
    template(overrides, 'study-notes');

  it('is offered nothing that changes the board', async () => {
    const { board } = await mount(viewOnly());
    expect(board.canEdit).toBe(false);
    expect(grid()).toHaveAttribute('aria-readonly', 'true');
    expect(within(grid()).queryByRole('button', { name: '+ New' })).toBeNull();
    expect(grid().querySelectorAll('[data-resize]')).toHaveLength(0);
    expect(grid().querySelectorAll('[data-drag-item]')).toHaveLength(0);

    await fireEvent.click(cellOf('Alpha', 'note'));
    await fireEvent.click(cellOf('Alpha', 'ready'));
    const title = rowOf('Alpha').querySelector<HTMLElement>('[data-kind="title"]');
    if (title) await fireEvent.click(title);
    await settle();
    expect(grid().querySelectorAll('input')).toHaveLength(0);
    expect(cardOf(board, 'Alpha').fields.properties.ready).toBeUndefined();
  });

  it('can fold a group for themselves: nothing is stored', async () => {
    const { board, viewId } = await mount(
      viewOnly({ groupBy: 'status', visibleGroups: ['todo', 'done'], hiddenGroups: [''] }),
    );
    await fireEvent.click(within(grid()).getByRole('button', { name: 'Collapse To do' }));
    await waitFor(() => {
      expect(titles()).toEqual(['Beta']);
    });
    expect(board.view(viewId)?.fields.collapsedGroups).toEqual([]);
  });
});

describe('the keyboard', () => {
  it('has one stop in the table, and it follows the focus', async () => {
    await mount();
    const stops = (): Element[] => [...grid().querySelectorAll('[data-cell][tabindex="0"]')];
    expect(stops()).toHaveLength(1);
    expect(stops()[0]).toBe(within(grid()).getByRole('columnheader', { name: 'Name' }));
    cellOf('Alpha', 'note').focus();
    expect(stops()).toEqual([cellOf('Alpha', 'note')]);
  });

  it('arrows move between cells and stop at the edges', async () => {
    await mount();
    const name = within(grid()).getByRole('columnheader', { name: 'Name' });
    name.focus();
    await fireEvent.keyDown(name, { key: 'ArrowUp' });
    expect(name).toHaveFocus();
    await fireEvent.keyDown(name, { key: 'ArrowDown' });
    const beta = rowOf('Beta').querySelector<HTMLElement>('[data-kind="title"]');
    expect(beta).toHaveFocus();
    if (!beta) return;
    await fireEvent.keyDown(beta, { key: 'ArrowLeft' });
    expect(beta).toHaveFocus();
    await fireEvent.keyDown(beta, { key: 'ArrowRight' });
    expect(cellOf('Beta', 'status')).toHaveFocus();
    await fireEvent.keyDown(cellOf('Beta', 'status'), { key: 'ArrowDown' });
    expect(cellOf('Alpha', 'status')).toHaveFocus();
    await fireEvent.keyDown(cellOf('Alpha', 'status'), { key: 'End' });
    expect(cellOf('Alpha', 'made')).toHaveFocus();
    await fireEvent.keyDown(cellOf('Alpha', 'made'), { key: 'Home' });
    expect(rowOf('Alpha').querySelector('[data-kind="title"]')).toHaveFocus();
  });

  it('keeps the column across a row that has one cell only', async () => {
    await mount(
      template({ groupBy: 'status', visibleGroups: ['todo', 'done'], hiddenGroups: [''] }),
    );
    cellOf('Gamma', 'hours').focus();
    // Down: "+ New" of the first group, the heading of the second, then its first card.
    await fireEvent.keyDown(cellOf('Gamma', 'hours'), { key: 'ArrowDown' });
    const add = document.activeElement;
    expect(add).toHaveAttribute('data-kind', 'add');
    if (add) await fireEvent.keyDown(add, { key: 'ArrowDown' });
    const heading = document.activeElement;
    expect(heading).toHaveAttribute('data-kind', 'group');
    if (heading) await fireEvent.keyDown(heading, { key: 'ArrowDown' });
    expect(cellOf('Beta', 'hours')).toHaveFocus();
  });

  it('Enter edits the cell, and Space ticks a checkbox', async () => {
    const { board } = await mount();
    const note = cellOf('Gamma', 'note');
    note.focus();
    await fireEvent.keyDown(note, { key: 'Enter' });
    expect(within(note).getByRole('textbox')).toHaveFocus();

    const ready = cellOf('Gamma', 'ready');
    ready.focus();
    await waitFor(() => {
      expect(grid().querySelectorAll('input')).toHaveLength(0);
    });
    await fireEvent.keyDown(ready, { key: ' ' });
    await waitFor(() => {
      expect(cardOf(board, 'Gamma').fields.properties.ready).toBe('true');
    });
  });

  it('leaves Ctrl+Z to the shell: the key passes through the table untouched', async () => {
    const { board } = await mount();
    await fireEvent.click(cellOf('Alpha', 'ready'));
    await waitFor(() => {
      expect(cardOf(board, 'Alpha').fields.properties.ready).toBe('true');
    });
    await settle();
    const seen: boolean[] = [];
    const outer = (event: KeyboardEvent): void => {
      seen.push(event.defaultPrevented);
    };
    window.addEventListener('keydown', outer);
    const cell = cellOf('Alpha', 'ready');
    cell.focus();
    await fireEvent.keyDown(cell, { key: 'z', ctrlKey: true });
    window.removeEventListener('keydown', outer);
    await settle();
    // The shell above the table has the shortcut (features/shell/undo-keys.ts):
    // it sees the key once, not taken, and the table undid nothing itself.
    expect(seen).toEqual([false]);
    expect(cardOf(board, 'Alpha').fields.properties.ready).toBe('true');
  });
});
