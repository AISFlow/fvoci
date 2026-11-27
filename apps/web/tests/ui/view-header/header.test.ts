/**
 * The header of a view over the real stores and the in-memory backend: what
 * a press changes in the view, that one undo takes one change back, and what
 * a reader without the right to edit gets.
 *
 * A simulated page has no layout and no style sheet: the pane counts as wide
 * (the whole row) and the page as expanded (popovers and dropdowns). The
 * folded row and the sheets are tested in the browser (e2e/view-header.spec.ts).
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { csvFileName } from '$lib/features/view-header/view-csv.ts';
import type { FilterGroup } from '$lib/logic/wire.ts';
import { BackendError, type BoardStore, type Workspace } from '$lib/state/index.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';
import type { SeedTemplate } from '$lib/state/memory/seed.ts';

import { openFirstBoard, OWNER, setup, SMALL, STATUS } from '../../state/support.ts';

import Harness from './Harness.svelte';

afterEach(() => {
  cleanup();
});

const EMPTY_FILTER: FilterGroup = { operation: 'and', filters: [] };

/** The small board with a second view, a table, and a text property. */
const TWO_VIEWS: SeedTemplate = {
  ...SMALL,
  board: {
    ...SMALL.board,
    cardProperties: [
      ...SMALL.board.cardProperties,
      { id: 'note', name: 'Note', type: 'text', options: [] },
    ],
  },
  views: [
    ...SMALL.views,
    {
      id: 'list',
      title: 'List',
      fields: {
        viewType: 'table',
        groupBy: '',
        shownProperties: [STATUS],
        visibleGroups: [],
        hiddenGroups: [],
        sort: [],
        cardOrder: ['c1', 'c2', 'c3'],
        filter: EMPTY_FILTER,
      },
    },
  ],
};

interface Mounted {
  board: BoardStore;
  workspace: Workspace;
  backend: MemoryBackend;
  onOpenView: ReturnType<typeof vi.fn<(viewId: string) => void>>;
  onAddCard: ReturnType<typeof vi.fn<() => void>>;
  /** The view the header is shown for, as the store has it now. */
  view: () => NonNullable<ReturnType<BoardStore['view']>>;
  /** Shows another view (by its title or identifier), as the board page does when the address changes. */
  show: (title: string) => Promise<void>;
  unmount: () => void;
}

async function mount(title: string, template: SeedTemplate = TWO_VIEWS): Promise<Mounted> {
  const { board, workspace, backend } = await openFirstBoard(setup([template]));
  const shown = board.views.find((view) => view.title === title);
  if (!shown) throw new Error(`no view called ${title}`);
  const onOpenView = vi.fn<(viewId: string) => void>();
  const onAddCard = vi.fn<() => void>();
  const { unmount, rerender } = render(Harness, {
    props: { board, viewId: shown.id, onOpenView, onAddCard },
  });
  return {
    board,
    workspace,
    backend,
    onOpenView,
    onAddCard,
    view: () => {
      const view = board.view(shown.id);
      if (!view) throw new Error('the view is gone');
      return view;
    },
    show: async (other) => {
      const viewId =
        board.views.find((view) => view.id === other || view.title === other)?.id ?? '';
      await rerender({ board, viewId, onOpenView, onAddCard });
    },
    unmount,
  };
}

const header = (): HTMLElement => screen.getByTestId('view-header');
const toolButton = (name: string): HTMLElement => {
  const found = header().querySelector<HTMLElement>(`[data-tool="${name}"]`);
  if (!found) throw new Error(`no tool ${name}`);
  return found;
};

/** Opens a dropdown the way a mouse does: its click counts one (Bits UI takes one of 0 for a key's). */
async function openMenu(trigger: HTMLElement): Promise<void> {
  await fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: 'mouse' });
  await fireEvent.click(trigger, { detail: 1 });
  await tick();
}

/*
 * What opens next to a button (a popover, a dropdown) is placed by measuring
 * the button. A simulated page measures nothing, the placing code concludes
 * that the button is out of sight and hides what it opened, and a query by
 * role and name then finds no name. So what is inside a popover or a menu is
 * found by its label or its text here; that it has its role and name for a
 * screen reader is checked in the browser.
 */

/** The lines of the menu that is open, not the tools at their ends. */
const menuLines = (): HTMLElement[] => [
  ...document.querySelectorAll<HTMLElement>('[role="menuitem"]:not([data-line-tool])'),
];
/** The tools at the end of the line of a view, by their names. */
const lineTools = (viewId: string): HTMLElement[] => [
  ...document.querySelectorAll<HTMLElement>(`[data-line="${viewId}"] [data-line-tool]`),
];
/** The tool `id` (`move-up`, `move-down`) at the end of the line of a view. */
function lineTool(viewId: string, id: string): HTMLElement {
  const found = lineTools(viewId).find((tool) => tool.dataset.action === id);
  if (!found) throw new Error(`no ${id} on the line of ${viewId}`);
  return found;
}
const lineTexts = (): string[] => menuLines().map((line) => line.textContent.trim());
/** The lines of the open list of views that are views: links to them. */
const viewLinks = (): HTMLAnchorElement[] => [
  ...document.querySelectorAll<HTMLAnchorElement>('a[role="menuitem"]'),
];

async function menuLine(name: string): Promise<HTMLElement> {
  return waitFor(() => {
    const line = menuLines().find((each) => each.textContent.trim() === name);
    if (!line) throw new Error(`no line "${name}" among ${lineTexts().join(', ')}`);
    return line;
  });
}

async function choose(name: string): Promise<void> {
  await fireEvent.click(await menuLine(name));
  await tick();
}

/** Lets a write reach the backend and its answer come back. */
async function settled(): Promise<void> {
  for (let turn = 0; turn < 6; turn += 1) await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('the whole row', () => {
  it('has one button that names the view, the views as links behind it, and every tool', async () => {
    const { board } = await mount('List');
    expect(header()).toHaveAttribute('data-layout', 'row');
    // No tabs: the views are behind the one button, which names the one shown.
    expect(within(header()).queryAllByRole('link')).toHaveLength(0);
    expect(toolButton('views')).toHaveTextContent('List');
    // Shown whole (a simulated page cuts nothing): no tooltip of the same words.
    await fireEvent.pointerEnter(toolButton('views'));
    expect(toolButton('views')).not.toHaveAttribute('title');
    // Cut (its widths set, as a simulated page lays nothing out): whole in its tooltip.
    const name = toolButton('views').querySelector('.truncate');
    if (!name) throw new Error('no name on the button');
    Object.defineProperty(name, 'scrollWidth', { value: 300, configurable: true });
    Object.defineProperty(name, 'clientWidth', { value: 120, configurable: true });
    await fireEvent.pointerEnter(toolButton('views'));
    expect(toolButton('views')).toHaveAttribute('title', 'List');
    await fireEvent.pointerLeave(toolButton('views'));
    expect(toolButton('views')).not.toHaveAttribute('title');
    await openMenu(toolButton('views'));
    const views = viewLinks();
    expect(views.map((line) => line.textContent.trim())).toEqual(['Board', 'List']);
    expect(views[1]).toHaveAttribute('aria-current', 'page');
    expect(views[0]).not.toHaveAttribute('aria-current');
    expect(views[0]).toHaveAttribute(
      'href',
      `/view/${board.views.find((view) => view.title === 'Board')?.id ?? ''}`,
    );
    // Under them the kinds to add, then what is done to the view shown.
    expect(lineTexts().slice(2)).toEqual([
      'Add board view',
      'Add table view',
      'Add gallery view',
      'Add calendar view',
      'Add Gantt view',
      'Rename view',
      'Duplicate view',
      'Delete view',
    ]);
    for (const name of ['properties', 'group', 'filter', 'sort', 'search']) {
      expect(toolButton(name)).toBeVisible();
    }
    // Every tool of a table has its own button in the whole row: "…" has nothing to hold.
    expect(header().querySelector('[data-tool="more"]')).toBeNull();
    // A tool is an icon: its name is what a screen reader hears, and its tooltip, with
    // what it has on as a count: here the one property the cards show.
    expect(toolButton('properties')).toHaveTextContent('1');
    expect(toolButton('properties')).toHaveAccessibleName('Properties (1)');
    expect(toolButton('properties')).toHaveAttribute('title', 'Properties (1)');
    expect(toolButton('filter')).toHaveTextContent('');
    expect(toolButton('filter')).toHaveAccessibleName('Filter');
    // Grouping has no count: it says what it is now.
    expect(toolButton('group')).toHaveAccessibleName('Group by: None');
    expect(toolButton('group')).toHaveAttribute('title', 'Group by: None');
  });

  it('opens a view by its line, and leaves a click that asks for a new tab to the browser', async () => {
    const { board, onOpenView } = await mount('List');
    await openMenu(toolButton('views'));
    expect(await fireEvent.click(await menuLine('Board'), { ctrlKey: true })).toBe(true);
    expect(onOpenView).not.toHaveBeenCalled();
    await openMenu(toolButton('views'));
    // The line of the view shown changes nothing.
    await fireEvent.click(await menuLine('List'));
    expect(onOpenView).not.toHaveBeenCalled();
    await openMenu(toolButton('views'));
    expect(await fireEvent.click(await menuLine('Board'))).toBe(false);
    expect(onOpenView).toHaveBeenCalledWith(board.views.find((view) => view.title === 'Board')?.id);
  });

  it('says the view shown as it changes, in a quiet status, and not a new name', async () => {
    const { board, show } = await mount('List');
    const said = screen.getByTestId('view-shown');
    expect(said).toHaveAttribute('role', 'status');
    expect(said.textContent.trim()).toBe('Now showing: List');
    await show('Board');
    expect(said.textContent.trim()).toBe('Now showing: Board');
    expect(toolButton('views')).toHaveTextContent('Board');

    // Renamed (here or by someone else) it is the same view: the button says so, the status not.
    const shown = board.views.find((view) => view.title === 'Board')?.id ?? '';
    await board.changeView(shown, (current) => ({ ...current, title: 'Boarding' }), 'renameView');
    await settled();
    expect(toolButton('views')).toHaveTextContent('Boarding');
    expect(said.textContent.trim()).toBe('Now showing: Board');
    await show('List');
    expect(said.textContent.trim()).toBe('Now showing: List');

    // Another view of the same name: the same words, said again as a new line.
    await board.changeView(shown, (current) => ({ ...current, title: 'List' }), 'renameView');
    await settled();
    const line = said.firstElementChild;
    await show(shown);
    expect(toolButton('views')).toHaveTextContent('List');
    expect(said.textContent.trim()).toBe('Now showing: List');
    expect(said.firstElementChild).not.toBe(line);
  });

  it('"New" asks for a card', async () => {
    const { onAddCard } = await mount('List');
    await fireEvent.click(within(header()).getByRole('button', { name: 'New' }));
    expect(onAddCard).toHaveBeenCalledTimes(1);
  });

  it('opens the search field in place of its button, and gives the text to the page as it is typed', async () => {
    await mount('List');
    const search = (): Element | null => header().querySelector('[data-testid="view-search"]');
    expect(search()).toHaveClass('hidden');
    await fireEvent.click(toolButton('search'));
    await tick();
    expect(search()).not.toHaveClass('hidden');
    expect(header().querySelector('[data-tool="search"]')).toBeNull();
    const field = within(header()).getByRole('searchbox', { name: 'Search cards' });
    expect(field).toHaveFocus();
    // The button of the views stays as it was.
    expect(toolButton('views')).toHaveTextContent('List');
    await fireEvent.input(field, { target: { value: 'alp' } });
    expect(screen.getByTestId('search-text')).toHaveTextContent('alp');
    // With text it stays open when the focus leaves.
    await fireEvent.blur(field);
    expect(search()).not.toHaveClass('hidden');
    await fireEvent.keyDown(field, { key: 'Escape' });
    expect(screen.getByTestId('search-text')).toHaveTextContent('');
    expect(search()).toHaveClass('hidden');
    expect(toolButton('search')).toHaveFocus();
    // Clicking the focused search button again immediately reopens and focuses the field.
    await fireEvent.click(toolButton('search'));
    expect(search()).not.toHaveClass('hidden');
    expect(within(header()).getByRole('searchbox', { name: 'Search cards' })).toHaveFocus();
  });

  it('gives an empty search field back to its button when the focus leaves it', async () => {
    await mount('List');
    vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    await fireEvent.click(toolButton('search'));
    await tick();
    await fireEvent.blur(within(header()).getByRole('searchbox', { name: 'Search cards' }));
    await tick();
    expect(header().querySelector('[data-testid="view-search"]')).toHaveClass('hidden');
    expect(toolButton('search')).toBeVisible();
    vi.restoreAllMocks();
  });

  it('has no grouping for a gallery and no sort for a calendar', async () => {
    const template: SeedTemplate = {
      ...SMALL,
      views: [
        { id: 'g', title: 'Gallery', fields: { ...SMALL.views[0]?.fields, viewType: 'gallery' } },
        { id: 'c', title: 'Calendar', fields: { ...SMALL.views[0]?.fields, viewType: 'calendar' } },
      ],
    };
    const gallery = await mount('Gallery', template);
    expect(header().querySelector('[data-tool="group"]')).toBeNull();
    expect(header().querySelector('[data-tool="sort"]')).not.toBeNull();
    gallery.unmount();
    await mount('Calendar', template);
    expect(header().querySelector('[data-tool="group"]')).toBeNull();
    expect(header().querySelector('[data-tool="sort"]')).toBeNull();
    expect(header().querySelector('[data-tool="filter"]')).not.toBeNull();
  });
});

describe('a reader who may only look', () => {
  it('gets the views and the search, and no tool that changes the view', async () => {
    // The seed makes the reader a viewer of the board with this key.
    await mount('List', { ...TWO_VIEWS, key: 'study-notes' });
    expect(toolButton('search')).toBeVisible();
    // The list of the views switches, and offers nothing else.
    await openMenu(toolButton('views'));
    expect(lineTexts()).toEqual(['Board', 'List']);
    expect(viewLinks()).toHaveLength(2);
    // Nor are the views moved.
    expect(document.querySelectorAll('[data-line-tool]')).toHaveLength(0);
    for (const name of ['properties', 'group', 'filter', 'sort', 'more', 'rename']) {
      expect(header().querySelector(`[data-tool="${name}"]`)).toBeNull();
    }
    expect(screen.queryByTestId('new-card')).toBeNull();
  });

  it('as a commenter, gets the views and nothing that moves them', async () => {
    const { board, workspace } = await mount('List');
    const me = workspace.me?.id;
    board.members = board.members.map((member) =>
      member.userId === me ? { ...member, role: 'commenter' } : member,
    );
    expect(board.role).toBe('commenter');
    await openMenu(toolButton('views'));
    expect(lineTexts()).toEqual(['Board', 'List']);
    expect(document.querySelectorAll('[data-line-tool]')).toHaveLength(0);
    expect(document.querySelector('[aria-keyshortcuts]')).toBeNull();
  });
});

describe('views', () => {
  it('adds a view of the chosen kind and shows it', async () => {
    const { board, onOpenView } = await mount('List');
    await openMenu(toolButton('views'));
    await choose('Add gallery view');
    await waitFor(() => {
      expect(onOpenView).toHaveBeenCalledTimes(1);
    });
    const made = board.views.find((view) => view.title === 'Gallery view');
    expect(made?.fields.viewType).toBe('gallery');
    expect(made?.fields.shownProperties).toEqual(['__title']);
    expect(onOpenView).toHaveBeenCalledWith(made?.id);
  });

  it('moves a view down and up with Alt and the arrows, the list open and the focus on its line', async () => {
    const { board, workspace } = await mount('List');
    await openMenu(toolButton('views'));
    const boardId = board.views.find((view) => view.title === 'Board')?.id ?? '';
    const line = (viewId: string): HTMLElement => {
      const found = document.querySelector<HTMLElement>(
        `[data-line="${viewId}"] [data-action="${viewId}"]`,
      );
      if (!found) throw new Error(`no line for ${viewId}`);
      return found;
    };
    // The keys are the line's, and named there: those that do something, at the top down alone.
    expect(line(boardId).getAttribute('aria-keyshortcuts')).toBe('Alt+ArrowDown');

    line(boardId).focus();
    await fireEvent.keyDown(line(boardId), { key: 'ArrowDown', altKey: true });
    await tick();
    expect(lineTexts().slice(0, 2)).toEqual(['List', 'Board']);
    // Still open, the focus on Board's line where it went, and its place said.
    expect(document.activeElement).toBe(line(boardId));
    expect(screen.getByText('Board is now 2 of 2')).toBeInTheDocument();
    expect(line(boardId).getAttribute('aria-keyshortcuts')).toBe('Alt+ArrowUp');
    await settled();
    expect(board.orderedViews.map((view) => view.title)).toEqual(['List', 'Board']);
    // Past the end, nothing.
    await fireEvent.keyDown(line(boardId), { key: 'ArrowDown', altKey: true });
    await settled();
    expect(board.orderedViews.map((view) => view.title)).toEqual(['List', 'Board']);

    await fireEvent.keyDown(line(boardId), { key: 'ArrowUp', altKey: true });
    await tick();
    expect(lineTexts().slice(0, 2)).toEqual(['Board', 'List']);
    expect(document.activeElement).toBe(line(boardId));
    await settled();
    await workspace.undoLast();
    await tick();
    expect(lineTexts().slice(0, 2)).toEqual(['List', 'Board']);
  });

  it('says where a view moved and refused is, back where it is stored, said once', async () => {
    const { board, workspace, backend } = await mount('List');
    vi.spyOn(backend, 'moveView').mockRejectedValue(new BackendError(403, { detail: 'no' }));
    await openMenu(toolButton('views'));
    const boardId = board.views.find((view) => view.title === 'Board')?.id ?? '';
    await fireEvent.click(lineTool(boardId, 'move-down'));
    await tick();
    expect(screen.getByText('Board is now 2 of 2')).toBeInTheDocument();
    await settled();
    expect(lineTexts().slice(0, 2)).toEqual(['Board', 'List']);
    expect(screen.getByText('Board is now 1 of 2')).toBeInTheDocument();
    expect(workspace.problems).toHaveLength(1);
  });

  it('does not treat a rapid second move of the same view as a refusal of the first', async () => {
    const first = TWO_VIEWS.views[0];
    if (!first) throw new Error('missing view');
    const three: SeedTemplate = {
      ...TWO_VIEWS,
      views: [...TWO_VIEWS.views, { ...first, id: 'third', title: 'Third' }],
    };
    const { board, workspace, backend } = await mount('List', three);
    let release = (): void => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const send = backend.moveView.bind(backend);
    let holdFirst = true;
    vi.spyOn(backend, 'moveView').mockImplementation(async (boardId, viewId, after) => {
      if (holdFirst) {
        holdFirst = false;
        await gate;
      }
      return send(boardId, viewId, after);
    });
    await openMenu(toolButton('views'));
    const announcements: string[] = [];
    const statusRegion = document.querySelector('span[aria-live="polite"]');
    if (!statusRegion) throw new Error('missing menu status region');
    const observer = new MutationObserver(() => {
      const text = statusRegion.textContent.trim();
      if (text) announcements.push(text);
    });
    observer.observe(statusRegion, { childList: true, characterData: true, subtree: true });
    try {
      const boardId = board.views.find((view) => view.title === 'Board')?.id ?? '';
      await fireEvent.click(lineTool(boardId, 'move-down'));
      await tick();
      expect(screen.getByText('Board is now 2 of 3')).toBeInTheDocument();
      await fireEvent.click(lineTool(boardId, 'move-down'));
      await tick();
      expect(screen.getByText('Board is now 3 of 3')).toBeInTheDocument();
      release();
      await settled();
      expect(lineTexts().slice(0, 3)).toEqual(['List', 'Third', 'Board']);
      expect(announcements).toEqual(['Board is now 2 of 3', 'Board is now 3 of 3']);
      expect(workspace.problems).toEqual([]);
    } finally {
      observer.disconnect();
    }
  });

  it('announces a move once when the server updates companion views atomically', async () => {
    const { board, workspace } = await mount('List');
    const boardId = board.views.find((view) => view.title === 'Board')?.id ?? '';
    await openMenu(toolButton('views'));
    const announcements: string[] = [];
    const statusRegion = document.querySelector('span[aria-live="polite"]');
    if (!statusRegion) throw new Error('missing menu status region');
    const observer = new MutationObserver(() => {
      const text = statusRegion.textContent.trim();
      if (text) announcements.push(text);
    });
    observer.observe(statusRegion, { childList: true, characterData: true, subtree: true });
    try {
      await fireEvent.click(lineTool(boardId, 'move-down'));
      await tick();
      await settled();
      expect(lineTexts().slice(0, 2)).toEqual(['List', 'Board']);
      expect(announcements).toEqual(['Board is now 2 of 2']);
      expect(workspace.problems).toEqual([]);
    } finally {
      observer.disconnect();
    }
  });

  it('has buttons at the end of each line for a pointer, closed at the ends, which keep the list open', async () => {
    const { board } = await mount('List');
    await openMenu(toolButton('views'));
    const idOf = (title: string): string =>
      board.views.find((view) => view.title === title)?.id ?? '';
    const [boardId, listId] = [idOf('Board'), idOf('List')];
    const titled = (viewId: string): (string | null)[] =>
      lineTools(viewId).map((tool) => tool.getAttribute('title'));
    expect(titled(boardId)).toEqual(['Move Board up (Alt+↑)', 'Move Board down (Alt+↓)']);
    // Items of the menu, named, for a screen reader read by touch; not Bits UI's, which its
    // arrows go to: those go from line to line.
    for (const tool of lineTools(boardId)) {
      expect(tool.getAttribute('role')).toBe('menuitem');
      expect(tool.getAttribute('aria-label')).toMatch(/^Move Board (up|down)$/);
      expect(tool.hasAttribute('data-dropdown-menu-item')).toBe(false);
      expect(tool.tabIndex).toBe(-1);
    }
    // A line is said as one of the lines the arrows go to, the buttons not counted.
    const lines = menuLines();
    expect(lines.map((line) => line.getAttribute('aria-posinset'))).toEqual(
      lines.map((_, index) => String(index + 1)),
    );
    expect(lines.every((line) => line.getAttribute('aria-setsize') === String(lines.length))).toBe(
      true,
    );
    const disabled = (viewId: string): boolean[] =>
      lineTools(viewId).map((tool) => (tool as HTMLButtonElement).disabled);
    expect(disabled(boardId)).toEqual([true, false]);
    expect(disabled(listId)).toEqual([false, true]);

    await fireEvent.click(lineTool(boardId, 'move-down'));
    await tick();
    expect(lineTexts().slice(0, 2)).toEqual(['List', 'Board']);
    expect(document.activeElement?.getAttribute('data-action')).toBe(boardId);
    expect(screen.getByText('Board is now 2 of 2')).toBeInTheDocument();
    expect(disabled(boardId)).toEqual([false, true]);
  });

  it('adds a view at the end of a list somebody put in order', async () => {
    const { board } = await mount('List');
    await openMenu(toolButton('views'));
    const listId = board.views.find((view) => view.title === 'List')?.id ?? '';
    await fireEvent.click(lineTool(listId, 'move-up'));
    await settled();
    expect(board.orderedViews.map((view) => view.title)).toEqual(['List', 'Board']);
    await choose('Add calendar view');
    await waitFor(() => {
      expect(board.orderedViews.map((view) => view.title)).toEqual([
        'List',
        'Board',
        'Calendar view',
      ]);
    });
  });

  it('renames a view with Enter, and not at all after Escape', async () => {
    const { view } = await mount('List');
    await openMenu(toolButton('views'));
    await choose('Rename view');
    const name = await screen.findByLabelText('View name');
    expect(name).toHaveValue('List');
    await fireEvent.input(name, { target: { value: '  Everything ' } });
    await fireEvent.keyDown(name, { key: 'Enter' });
    await settled();
    expect(view().title).toBe('Everything');
    await waitFor(() => {
      expect(screen.queryByLabelText('View name')).toBeNull();
    });

    await openMenu(toolButton('views'));
    await choose('Rename view');
    const again = await screen.findByLabelText('View name');
    await fireEvent.input(again, { target: { value: 'Not this' } });
    await fireEvent.keyDown(again, { key: 'Escape' });
    await settled();
    await waitFor(() => {
      expect(screen.queryByLabelText('View name')).toBeNull();
    });
    expect(view().title).toBe('Everything');
  });

  it('renames the view it was opened for, also when another is shown before it closes', async () => {
    const { board, show } = await mount('List');
    await openMenu(toolButton('views'));
    await choose('Rename view');
    const name = await screen.findByLabelText('View name');
    await fireEvent.input(name, { target: { value: 'Mine' } });
    // The address goes back to another view (the browser's back) while the field is open.
    await show('Board');
    await fireEvent.keyDown(name, { key: 'Enter' });
    await settled();
    expect(board.views.map((view) => view.title).sort()).toEqual(['Board', 'Mine']);
  });

  it('does not rename while an input method is still composing the name', async () => {
    const { view } = await mount('List');
    await openMenu(toolButton('views'));
    await choose('Rename view');
    const name = await screen.findByLabelText('View name');
    await fireEvent.input(name, { target: { value: '목록' } });
    await fireEvent.keyDown(name, { key: 'Enter', isComposing: true });
    await settled();
    expect(view().title).toBe('List');
    expect(screen.getByLabelText('View name')).toBeInTheDocument();
  });

  it('copies a view under a name that says so, and shows the copy', async () => {
    const { board, onOpenView } = await mount('List');
    await openMenu(toolButton('views'));
    await choose('Duplicate view');
    await waitFor(() => {
      expect(onOpenView).toHaveBeenCalledTimes(1);
    });
    const copy = board.views.find((view) => view.title === 'List copy');
    expect(copy?.fields.viewType).toBe('table');
    expect(copy?.fields.shownProperties).toEqual([STATUS]);
    expect(onOpenView).toHaveBeenCalledWith(copy?.id);
  });

  it('deletes a view only after asking, and undo brings it back', async () => {
    const { board, workspace } = await mount('List');
    await openMenu(toolButton('views'));
    await choose('Delete view');
    const dialog = await screen.findByTestId('confirm-dialog');
    expect(dialog).toHaveTextContent('Delete “List”?');
    expect(board.views).toHaveLength(2);
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
    await settled();
    expect(board.views.map((view) => view.title)).toEqual(['Board']);
    await workspace.undoLast();
    expect(board.views.map((view) => view.title).sort()).toEqual(['Board', 'List']);
  });

  it('deletes the view it asked about, also when another is shown before the answer', async () => {
    const { board, show } = await mount('List');
    await openMenu(toolButton('views'));
    await choose('Delete view');
    const dialog = await screen.findByTestId('confirm-dialog');
    // The address goes back to another view (the browser's back) while the question is open.
    await show('Board');
    expect(dialog).toHaveTextContent('Delete “List”?');
    await fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
    await settled();
    expect(board.views.map((view) => view.title)).toEqual(['Board']);
  });

  it('cannot delete the last view of a board', async () => {
    await mount('Board', SMALL);
    await openMenu(toolButton('views'));
    expect(await menuLine('Delete view')).toHaveAttribute('data-disabled');
    expect(await menuLine('Duplicate view')).not.toHaveAttribute('data-disabled');
  });
});

describe('grouping', () => {
  it('by another property refills the columns in one change that one undo takes back', async () => {
    const { board, view, workspace } = await mount('Board');
    await board.setViewFields(view().id, {
      visibleGroups: ['todo', 'done', ''],
      hiddenGroups: ['doing'],
      collapsedGroups: ['done'],
    });
    workspace.undo.clear();
    expect(toolButton('group')).toHaveAccessibleName('Group by: Status');

    await openMenu(toolButton('group'));
    // A board is always grouped by something.
    await menuLine('Status');
    expect(lineTexts()).toEqual(['Status', 'Owner']);
    await choose('Owner');
    await settled();
    expect(view().fields).toMatchObject({
      groupBy: OWNER,
      visibleGroups: [''],
      hiddenGroups: [],
      collapsedGroups: [],
    });
    expect(toolButton('group')).toHaveAccessibleName('Group by: Owner');

    await workspace.undoLast();
    expect(view().fields).toMatchObject({
      groupBy: STATUS,
      visibleGroups: ['todo', 'done', ''],
      hiddenGroups: ['doing'],
      collapsedGroups: ['done'],
    });
    expect(workspace.canUndo).toBe(false);
  });

  it('of a table can be taken off', async () => {
    const { view } = await mount('List');
    expect(toolButton('group')).toHaveAccessibleName('Group by: None');
    await openMenu(toolButton('group'));
    await choose('Status');
    await settled();
    expect(view().fields.groupBy).toBe(STATUS);
    expect(view().fields.visibleGroups).toEqual(['', 'todo', 'doing', 'done']);
    await openMenu(toolButton('group'));
    await choose('Ungroup');
    await settled();
    expect(view().fields.groupBy).toBe('');
    expect(view().fields.visibleGroups).toEqual([]);
  });

  it('by the property it already is changes nothing', async () => {
    const { workspace } = await mount('Board');
    await openMenu(toolButton('group'));
    await choose('Status');
    await settled();
    expect(workspace.canUndo).toBe(false);
  });
});

describe('the properties a view shows', () => {
  it('are switched one at a time, each a change of its own', async () => {
    const { view, workspace } = await mount('List');
    await fireEvent.click(toolButton('properties'));
    const panel = await screen.findByTestId('properties-panel');
    const names = [...panel.querySelectorAll('[role="switch"]')].map((each) =>
      each.getAttribute('aria-label'),
    );
    // A table: its properties, no switch for the title and none for the marks.
    expect(names).toEqual(['Status', 'Owner', 'Note']);
    expect(within(panel).getByLabelText('Status')).toBeChecked();
    expect(within(panel).getByLabelText('Note')).not.toBeChecked();

    await fireEvent.click(within(panel).getByLabelText('Note'));
    await fireEvent.click(within(panel).getByLabelText('Status'));
    await settled();
    expect(view().fields.shownProperties).toEqual(['note']);
    expect(toolButton('properties')).toHaveAccessibleName('Properties (1)');
    await workspace.undoLast();
    expect(view().fields.shownProperties).toEqual([STATUS, 'note']);
    expect(toolButton('properties')).toHaveAccessibleName('Properties (2)');
    await workspace.undoLast();
    expect(view().fields.shownProperties).toEqual([STATUS]);
  });
});

describe('the filter', () => {
  const nested: FilterGroup = {
    operation: 'or',
    filters: [
      { propertyId: STATUS, condition: 'includes', values: ['todo'] },
      { propertyId: 'title', condition: 'contains', values: ['mm'] },
    ],
  };

  async function openFilter(): Promise<HTMLElement> {
    await fireEvent.click(toolButton('filter'));
    return screen.findByTestId('filter-editor');
  }
  const rows = (editor: HTMLElement): HTMLElement[] => [
    ...editor.querySelectorAll<HTMLElement>('[data-filter-row]'),
  ];
  const add = async (editor: HTMLElement): Promise<void> => {
    const button = editor.querySelector<HTMLElement>('[data-tool="add-filter"]');
    if (!button) throw new Error('no button to add a condition');
    await fireEvent.click(button);
    await settled();
  };
  const slot = (row: HTMLElement | undefined, name: string): HTMLElement => {
    const found = row?.querySelector<HTMLElement>(`[data-slot="${name}"]`);
    if (!found) throw new Error(`no ${name}`);
    return found;
  };

  it('gets a condition, changes its property and condition, and saves typed text on Enter', async () => {
    const { view, board } = await mount('List');
    const editor = await openFilter();
    expect(editor).toHaveTextContent('No conditions');
    await add(editor);
    expect(view().fields.filter.filters).toEqual([
      { propertyId: STATUS, condition: 'includes', values: [] },
    ]);
    expect(toolButton('filter')).toHaveAccessibleName('Filter (1)');

    await openMenu(slot(rows(editor)[0], 'filter-property'));
    // What cannot be filtered by is not offered; the title is.
    await menuLine('Title');
    expect(lineTexts()).toEqual(['Title', 'Status', 'Owner', 'Note']);
    await choose('Title');
    await settled();
    expect(view().fields.filter.filters).toEqual([
      { propertyId: 'title', condition: 'is', values: [] },
    ]);

    await openMenu(slot(rows(editor)[0], 'filter-condition'));
    await choose('contains');
    await settled();
    const text = within(editor).getByLabelText('Value for Title');
    await fireEvent.input(text, { target: { value: 'mm' } });
    // Typing alone saves nothing.
    await settled();
    expect(view().fields.filter.filters).toEqual([
      { propertyId: 'title', condition: 'contains', values: [] },
    ]);
    await fireEvent.keyDown(text, { key: 'Enter' });
    await settled();
    expect(view().fields.filter.filters).toEqual([
      { propertyId: 'title', condition: 'contains', values: ['mm'] },
    ]);
    expect(board.cardsOf(view()).map((card) => card.title)).toEqual(['Gamma']);
  });

  it('saves text that was typed and not confirmed when the field goes away', async () => {
    const { view, board, unmount } = await mount('List');
    await board.setViewFields(view().id, {
      filter: {
        operation: 'and',
        filters: [{ propertyId: 'note', condition: 'startsWith', values: ['a'] }],
      },
    });
    const editor = await openFilter();
    const text = within(editor).getByLabelText('Value for Note');
    expect(text).toHaveValue('a');
    await fireEvent.input(text, { target: { value: 'ab' } });
    unmount();
    await settled();
    expect(view().fields.filter.filters).toEqual([
      { propertyId: 'note', condition: 'startsWith', values: ['ab'] },
    ]);
  });

  it('follows a change somebody else made while the editor is open', async () => {
    const { view, board } = await mount('List');
    const editor = await openFilter();
    await add(editor);
    await board.setViewFields(view().id, {
      filter: {
        operation: 'and',
        filters: [{ propertyId: 'note', condition: 'endsWith', values: ['z'] }],
      },
    });
    await tick();
    expect(slot(rows(editor)[0], 'filter-property')).toHaveTextContent('Note');
    expect(slot(rows(editor)[0], 'filter-condition')).toHaveTextContent('ends with');
    expect(within(editor).getByLabelText('Value for Note')).toHaveValue('z');
  });

  it('keeps a group of conditions inside it through adding, changing and removing', async () => {
    const { view, board, workspace } = await mount('List');
    await board.setViewFields(view().id, { filter: { operation: 'and', filters: [nested] } });
    workspace.undo.clear();
    const stored = view().fields.filter.filters[0];
    expect(toolButton('filter')).toHaveAccessibleName('Filter (2)');

    const editor = await openFilter();
    expect(within(editor).getByTestId('filter-nested')).toHaveTextContent(
      'Nested groups of conditions: 1.',
    );
    expect(rows(editor)).toHaveLength(0);

    await add(editor);
    expect(view().fields.filter.filters).toEqual([
      nested,
      // The group's conditions are not lines: the first free property is still the first one.
      { propertyId: STATUS, condition: 'includes', values: [] },
    ]);
    expect(view().fields.filter.filters[0]).toEqual(stored);

    // With two parts there is a choice between all and any.
    await fireEvent.click(within(editor).getByText('Any condition'));
    await settled();
    expect(view().fields.filter).toEqual({
      operation: 'or',
      filters: [nested, { propertyId: STATUS, condition: 'includes', values: [] }],
    });

    await openMenu(slot(rows(editor)[0], 'filter-condition'));
    await choose('is empty');
    await settled();
    expect(view().fields.filter.filters[0]).toEqual(nested);
    expect(rows(editor)[0]?.querySelector('[data-slot="property-select"]')).toBeNull();

    const remove = rows(editor)[0]?.querySelector<HTMLElement>('[data-remove]');
    if (!remove) throw new Error('no button to remove the condition');
    await fireEvent.click(remove);
    await settled();
    expect(view().fields.filter).toEqual({ operation: 'or', filters: [nested] });
    // The focus is not left on the button that went away with its line.
    expect(editor.querySelector('[data-tool="add-filter"]')).toHaveFocus();

    // Undo, step by step, back to the filter as it was, the group whole at every step.
    for (let step = 0; step < 4; step += 1) {
      await workspace.undoLast();
      expect(view().fields.filter.filters[0]).toEqual(nested);
    }
    expect(view().fields.filter).toEqual({ operation: 'and', filters: [nested] });
    expect(workspace.canUndo).toBe(false);
  });
});

describe('the sort', () => {
  it('adds, reverses, moves and removes lines, and says what a sort turns off', async () => {
    const { view } = await mount('List');
    await fireEvent.click(toolButton('sort'));
    const editor = await screen.findByTestId('sort-editor');
    expect(editor).toHaveTextContent('No sort');
    expect(within(editor).queryByTestId('sort-notice')).toBeNull();

    const add = editor.querySelector<HTMLElement>('[data-tool="add-sort"]');
    if (!add) throw new Error('no button to add a sort');
    await openMenu(add);
    await menuLine('Title');
    expect(lineTexts()).toEqual(['Title', 'Status', 'Owner', 'Note']);
    await choose('Status');
    await settled();
    await openMenu(add);
    // A property is sorted by once.
    await menuLine('Title');
    expect(lineTexts()).toEqual(['Title', 'Owner', 'Note']);
    await choose('Title');
    await settled();
    expect(view().fields.sort).toEqual([
      { propertyId: STATUS, reversed: false },
      { propertyId: '__title', reversed: false },
    ]);
    expect(within(editor).getByTestId('sort-notice')).toHaveTextContent('cannot be dragged');
    expect(toolButton('sort')).toHaveAccessibleName('Sort (2)');

    await fireEvent.click(within(editor).getByLabelText('Title: Ascending. Press to reverse.'));
    await settled();
    expect(view().fields.sort[1]).toEqual({ propertyId: '__title', reversed: true });
    expect(within(editor).getByLabelText('Title: Descending. Press to reverse.')).toHaveTextContent(
      'Descending',
    );

    expect(within(editor).getByLabelText('Move Status up')).toBeDisabled();
    expect(within(editor).getByLabelText('Move Title down')).toBeDisabled();
    await fireEvent.click(within(editor).getByLabelText('Move Title up'));
    await settled();
    expect(view().fields.sort.map((option) => option.propertyId)).toEqual(['__title', STATUS]);
    expect(editor.querySelector('[role="status"]')).toHaveTextContent(
      'Title is now number 1 of 2.',
    );
    // The button that was pressed is off now; the focus is on the line's other one.
    expect(within(editor).getByLabelText('Move Title down')).toHaveFocus();

    await fireEvent.click(within(editor).getByLabelText('Remove the sort by Title'));
    await settled();
    expect(view().fields.sort).toEqual([{ propertyId: STATUS, reversed: false }]);
    expect(add).toHaveFocus();
  });
});

describe('the file of a view', () => {
  it('is named by the board and the view, without what a system refuses or what turns the name', () => {
    expect(csvFileName('Plan', 'Week 1')).toBe('Plan - Week 1.csv');
    expect(csvFileName('a/b: c?', 'd')).toBe('a_b_ c_ - d.csv');
    // U+202E would show the name backwards; U+200B is not seen.
    expect(csvFileName('Report\u202eplh', 'x\u200by')).toBe('Reportplh - xy.csv');
  });
});
