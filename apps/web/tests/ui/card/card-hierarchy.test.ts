/**
 * UI tests for card hierarchy components and view integration:
 * - CardFace sub-card progress badge and parent clue
 * - CardParentBar breadcrumb, parent picker, clear, and undo
 * - SubcardsSection inline create, attach existing, detach, depth-3 limit, progress bar, and undo
 * - Problems.svelte rendering `card_too_deep` message
 * - Table parent-child grouping, expand/collapse, and filtered-parent retention
 * - GanttChart parent-child grouping and collapse
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { flushSync, tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import Problems from '../../../src/app/Problems.svelte';
import CardFace from '$lib/components/card-face/CardFace.svelte';
import CardParentBar from '$lib/features/card/CardParentBar.svelte';
import SubcardsSection from '$lib/features/card/SubcardsSection.svelte';
import { GanttChart } from '$lib/features/views/gantt/chart.svelte.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';
import type { Regional } from '$lib/logic/regional.ts';
import { createCard, type Card, type PropertyTemplate } from '$lib/logic/wire.ts';
import { BackendError, type BoardStore, type CardSummary } from '$lib/state/index.ts';
import type { SeedTemplate } from '$lib/state/memory/seed.ts';

import { openFirstBoard, setup, settle } from '../../state/support.ts';
import TableHarness from '../table/TableHarness.svelte';

afterEach(() => {
  cleanup();
});

const SEOUL: Regional = { timeZone: 'Asia/Seoul', weekStart: 7, hourCycle: 'h12' };

function cardByTitle(board: BoardStore, title: string): CardSummary {
  const found = board.cards.find((c) => c.title === title);
  if (!found) throw new Error(`missing card "${title}"`);
  return found;
}

function mustQuery(root: ParentNode, selector: string): HTMLElement {
  const found = root.querySelector<HTMLElement>(selector);
  if (!found) throw new Error(`missing element "${selector}"`);
  return found;
}

function hierarchySeed(
  viewOverrides: Record<string, unknown> = {},
  viewType: 'table' | 'gantt' = 'table',
): SeedTemplate {
  return {
    key: 'hier-test',
    onboarding: false,
    board: {
      id: 'b',
      title: 'Hierarchy Board',
      description: '',
      icon: '',
      showDescription: false,
      cardProperties: [
        {
          id: 'status',
          name: 'Status',
          type: 'select',
          options: [
            { id: 'todo', label: 'To Do', color: 'blue' },
            { id: 'done', label: 'Done', color: 'green' },
          ],
        },
        { id: 'due', name: 'Due Date', type: 'date', options: [] },
      ],
    },
    views: [
      {
        id: 'v',
        title: 'Main',
        fields: {
          viewType,
          shownProperties: ['status', 'due'],
          visibleGroups: [],
          hiddenGroups: [],
          collapsedGroups: [],
          sort: [],
          cardOrder: [],
          columnWidths: {},
          columnCalculations: {},
          dateProperty: 'due',
          filter: { operation: 'and', filters: [] },
          ...viewOverrides,
        },
      },
    ],
    cards: [
      {
        id: 'p1',
        title: 'Epic',
        fields: {
          icon: '',
          isTemplate: false,
          properties: { status: 'todo', due: { from: '2026-11-10', to: '2026-11-15' } },
        },
        body: null,
      },
      {
        id: 'c1',
        title: 'Task A',
        fields: {
          icon: '',
          isTemplate: false,
          properties: { status: 'done', due: { from: '2026-11-11', to: '2026-11-12' } },
        },
        body: null,
      },
      {
        id: 'c2',
        title: 'Task B',
        fields: {
          icon: '',
          isTemplate: false,
          properties: { status: 'todo', due: { from: '2026-11-13', to: '2026-11-14' } },
        },
        body: null,
      },
      {
        id: 'other',
        title: 'Standalone',
        fields: {
          icon: '',
          isTemplate: false,
          properties: { status: 'todo', due: { from: '2026-11-16', to: '2026-11-17' } },
        },
        body: null,
      },
    ],
    files: [],
  };
}

describe('CardFace hierarchy cues', () => {
  it('renders sub-card progress badge and parent clue when provided', () => {
    const card: Card = createCard();
    card.id = 'x';
    card.title = 'Child card';
    card.number = 4;
    const templates: PropertyTemplate[] = [];
    const { container } = render(CardFace, {
      card,
      templates,
      shownProperties: [],
      subcardsTotal: 3,
      subcardsDone: 1,
      parentHint: { number: 2, title: 'Parent card' },
      locale: 'en-US',
      regional: SEOUL,
    });
    expect(container.querySelector('[data-slot="card-subcards"]')).toHaveTextContent('1/3');
    expect(container.querySelector('[data-slot="card-parent-hint"]')).toHaveTextContent(
      /#2.*Parent card/,
    );
  });

  it('omits sub-card badge and parent clue when absent', () => {
    const card: Card = createCard();
    card.id = 'x';
    card.title = 'Plain card';
    const { container } = render(CardFace, {
      card,
      templates: [],
      shownProperties: [],
      locale: 'en-US',
      regional: SEOUL,
    });
    expect(container.querySelector('[data-slot="card-subcards"]')).toBeNull();
    expect(container.querySelector('[data-slot="card-parent-hint"]')).toBeNull();
  });
});

describe('SubcardsSection and CardParentBar with BoardStore', () => {
  it('creates, attaches, detaches sub-cards with progress and supports undo', async () => {
    const { board, workspace } = await openFirstBoard(setup([hierarchySeed()]));
    const parent = cardByTitle(board, 'Epic');
    const taskA = cardByTitle(board, 'Task A');
    const onOpenCard = vi.fn<(cardId: string) => void>();

    render(SubcardsSection, { board, card: parent, onOpenCard });

    // Create a new sub-card inline with Enter.
    const input = screen.getByTestId('subcard-new-input');
    await fireEvent.input(input, { target: { value: 'Fresh subtask' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    await settle();

    await waitFor(() => {
      expect(screen.getByTestId('subcards-list')).toHaveTextContent('Fresh subtask');
    });
    expect(screen.getByTestId('subcards-progress')).toHaveTextContent('0/1 (0%)');

    // Attach existing completed card `Task A` via picker.
    await fireEvent.click(screen.getByTestId('subcard-link-trigger'));
    await tick();
    const option = await screen.findByRole('option', { name: /Task A/ });
    await fireEvent.click(option);
    await settle();

    await waitFor(() => {
      expect(screen.getByTestId('subcards-progress')).toHaveTextContent('1/2 (50%)');
    });
    expect(board.card(taskA.id)?.parentId).toBe(parent.id);

    // Clicking a sub-card navigates to it.
    await fireEvent.click(
      mustQuery(screen.getByTestId('subcards-list'), `[data-subcard-open="${taskA.id}"]`),
    );
    expect(onOpenCard).toHaveBeenCalledWith(taskA.id);

    // Detaching moves it back to top-level, and undo puts it back under parent.
    await fireEvent.click(
      mustQuery(screen.getByTestId('subcards-list'), `[data-subcard-detach="${taskA.id}"]`),
    );
    await settle();
    expect(board.card(taskA.id)?.parentId).toBe('');
    expect(screen.getByTestId('subcards-progress')).toHaveTextContent('0/1 (0%)');

    await workspace.undoLast();
    await settle();
    expect(board.card(taskA.id)?.parentId).toBe(parent.id);
    expect(screen.getByTestId('subcards-progress')).toHaveTextContent('1/2 (50%)');
  });

  it('shows ancestor breadcrumb, hides add on depth-3 card, and supports parent clear/undo', async () => {
    const { board, workspace } = await openFirstBoard(setup([hierarchySeed()]));
    const epic = cardByTitle(board, 'Epic');
    const taskA = cardByTitle(board, 'Task A');
    const taskB = cardByTitle(board, 'Task B');
    await board.changeCard(taskA.id, (c) => ({ ...c, parentId: epic.id }));
    await board.changeCard(taskB.id, (c) => ({ ...c, parentId: taskA.id }));
    await settle();

    const grand = board.card(taskB.id);
    if (!grand) throw new Error('missing Task B');
    const onOpenCard = vi.fn<(cardId: string) => void>();

    render(CardParentBar, { board, card: grand, onOpenCard });
    const breadcrumb = screen.getByTestId('card-parent-breadcrumb');
    expect(breadcrumb).toHaveTextContent('Epic');
    expect(breadcrumb).toHaveTextContent('Task A');

    await fireEvent.click(mustQuery(breadcrumb, `[data-parent-crumb="${epic.id}"]`));
    expect(onOpenCard).toHaveBeenCalledWith(epic.id);

    // Depth-3 card has no add sub-card section.
    render(SubcardsSection, { board, card: grand, onOpenCard });
    expect(screen.queryByTestId('card-subcards')).toBeNull();

    // Clear parent via button and undo.
    await fireEvent.click(screen.getByTestId('card-parent-clear'));
    await settle();
    expect(board.card(taskB.id)?.parentId).toBe('');

    await workspace.undoLast();
    await settle();
    expect(board.card(taskB.id)?.parentId).toBe(taskA.id);
  });

  it('renders a clear message in Problems when backend returns card_too_deep', async () => {
    const { workspace } = await openFirstBoard(setup([hierarchySeed()]));
    render(Problems, { workspace });
    workspace.report(
      'save',
      new BackendError(400, {
        reason: 'card_too_deep',
        limit: 3,
        detail: 'too deep',
      }),
    );
    await tick();
    expect(screen.getByTestId('problems')).toHaveTextContent(/3/);
  });
});

describe('Table and Gantt hierarchical grouping', () => {
  it('groups children under parents in Table, collapses/expands, and keeps filtered-parent children', async () => {
    const { board } = await openFirstBoard(setup([hierarchySeed()]));
    const epic = cardByTitle(board, 'Epic');
    const taskA = cardByTitle(board, 'Task A');
    const taskB = cardByTitle(board, 'Task B');
    const standalone = cardByTitle(board, 'Standalone');
    const viewId = board.orderedViews[0]?.id ?? '';
    await board.changeCard(taskA.id, (c) => ({ ...c, parentId: epic.id }));
    await board.changeCard(taskB.id, (c) => ({ ...c, parentId: taskA.id }));
    await settle();

    const { container } = render(TableHarness, {
      board,
      viewId,
      onOpenCard: vi.fn(),
    });
    await tick();

    const rowIds = () =>
      [...container.querySelectorAll<HTMLElement>('[role="row"][data-card]')].map((el) => ({
        id: el.dataset.card,
        level: el.dataset.level,
      }));

    expect(rowIds()).toEqual([
      { id: epic.id, level: '0' },
      { id: taskA.id, level: '1' },
      { id: taskB.id, level: '2' },
      { id: standalone.id, level: '0' },
    ]);

    // Collapse parent Epic -> hides Task A and Task B.
    await fireEvent.click(mustQuery(container, `[data-hierarchy-toggle="${epic.id}"]`));
    await tick();
    expect(rowIds()).toEqual([
      { id: epic.id, level: '0' },
      { id: standalone.id, level: '0' },
    ]);

    // Expand again -> Task A and Task B return.
    await fireEvent.click(mustQuery(container, `[data-hierarchy-toggle="${epic.id}"]`));
    await tick();
    expect(rowIds().map((r) => r.id)).toEqual([epic.id, taskA.id, taskB.id, standalone.id]);

    // Filter to Status = Done (hides parent Epic, keeps child Task A visible at level 0!).
    await board.setViewFields(viewId, {
      filter: {
        operation: 'and',
        filters: [{ propertyId: 'status', condition: 'includes', values: ['done'] }],
      },
    });
    await settle();
    await tick();
    expect(rowIds()).toEqual([{ id: taskA.id, level: undefined }]);
  });

  it('groups children under parents in GanttChart and updates rows/bars on collapse', async () => {
    const { board } = await openFirstBoard(setup([hierarchySeed({}, 'gantt')]));
    const epic = cardByTitle(board, 'Epic');
    const taskA = cardByTitle(board, 'Task A');
    const taskB = cardByTitle(board, 'Task B');
    const standalone = cardByTitle(board, 'Standalone');
    await board.changeCard(taskA.id, (c) => ({ ...c, parentId: epic.id }));
    await board.changeCard(taskB.id, (c) => ({ ...c, parentId: taskA.id }));
    await settle();

    const view = board.orderedViews[0];
    if (!view) throw new Error('missing view');

    const chart = new GanttChart({
      board: () => board,
      view: () => view,
      cards: () => board.cardsOf(view),
      today: () => '2026-11-10' as CalendarDate,
      zoom: () => 'week',
      minDays: () => 28,
    });

    expect(
      chart.cardLines.map((l) => ({ id: l.id, level: l.level, hasChildren: l.hasChildren })),
    ).toEqual([
      { id: epic.id, level: 0, hasChildren: true },
      { id: taskA.id, level: 1, hasChildren: true },
      { id: taskB.id, level: 2, hasChildren: false },
      { id: standalone.id, level: 0, hasChildren: false },
    ]);

    flushSync(() => {
      chart.toggleParent(epic.id);
    });
    expect(chart.cardLines.map((l) => ({ id: l.id, collapsed: l.collapsed }))).toEqual([
      { id: epic.id, collapsed: true },
      { id: standalone.id, collapsed: false },
    ]);
    expect(chart.bars.has(taskA.id)).toBe(false);

    flushSync(() => {
      chart.toggleParent(epic.id);
    });
    expect(chart.cardLines.map((l) => l.id)).toEqual([epic.id, taskA.id, taskB.id, standalone.id]);
    expect(chart.bars.has(taskA.id)).toBe(true);
  });
});
