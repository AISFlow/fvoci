/**
 * A view of a board shown in a body (EmbeddedView.svelte): its header and
 * its cards as the view shows them, the one line for a view the reader may
 * not see or that is gone, at most fifty cards and a link to all of them,
 * a checkbox changed in place by who may change cards; and the days of a
 * calendar's summary, of all its cards.
 */
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import EmbeddedView from '../../../src/app/embedded/EmbeddedView.svelte';
import {
  daysOf,
  EMBEDDED_CARDS,
  firstCards,
  kindName,
  summaryOf,
} from '../../../src/app/embedded/embedded-view.ts';
import { Router } from '../../../src/app/router.svelte.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import type { CalendarDate } from '$lib/logic/dates.ts';
import type { BoardView, Card, PropertyTemplate } from '$lib/logic/wire.ts';

import { loadTemplates, openFirstBoard, settle, setup } from '../../state/support.ts';

afterEach(() => {
  cleanup();
  opened.length = 0;
});

const MISSING = '01930000-0000-7000-8000-0000000b0404';

/** The cards asked to open over the page, in the order asked. */
const opened: [boardId: string, viewId: string, cardId: string][] = [];
const onOpenCard = (boardId: string, view: BoardView, cardId: string): void => {
  opened.push([boardId, view.id, cardId]);
};

async function shown() {
  const made = await openFirstBoard(setup(await loadTemplates()));
  const view = made.board.orderedViews[0];
  if (!view) throw new Error('the board has no view');
  const router = new Router('');
  return { ...made, view, router };
}

describe('a view shown in a body', () => {
  it('names its board and itself, its kind, and links to itself on the board', async () => {
    const { workspace, board, view, router } = await shown();
    render(EmbeddedView, { workspace, router, viewId: view.id, onOpenCard });
    const frame = await screen.findByTestId('embedded-view');
    expect(frame.getAttribute('aria-label')).toContain(view.title);
    expect(within(frame).getByText(board.board?.title ?? '')).toBeTruthy();
    expect(within(frame).getByTestId('embedded-view-kind').textContent.trim()).toBe(
      kindName(view.fields.viewType),
    );
    const open = within(frame).getByRole('link', { name: m.board_view_open() });
    expect(open.getAttribute('href')).toContain(`/boards/${board.boardId}/${view.id}`);
    const titles = within(frame)
      .queryAllByTestId('embedded-card')
      .map((link) => link.textContent.trim());
    expect(titles.length).toBe(Math.min(board.filteredCardsOf(view).length, EMBEDDED_CARDS));
  });

  it('opens a card pressed over the page, which is not its board', async () => {
    const { workspace, board, view, router } = await shown();
    render(EmbeddedView, { workspace, router, viewId: view.id, onOpenCard });
    const [first] = await screen.findAllByTestId('embedded-card');
    if (!first) throw new Error('no card shown');
    await fireEvent.click(first);
    const cardId = first.getAttribute('href')?.split('/').at(-1);
    expect(opened).toEqual([[board.boardId, view.id, cardId]]);
  });

  it('says only that it cannot be seen, for a view that is not there', async () => {
    const { workspace, router } = await shown();
    render(EmbeddedView, { workspace, router, viewId: MISSING, onOpenCard });
    expect((await screen.findByTestId('board-view-unseen')).textContent).toBe(
      m.board_view_unseen(),
    );
    expect(screen.queryByTestId('embedded-view')).toBeNull();
  });

  it('shows fifty cards at most, and links to all of them on the board', async () => {
    const { workspace, board, view, router, backend } = await shown();
    const many = EMBEDDED_CARDS + 3 - board.filteredCardsOf(view).length;
    for (let made = 0; made < many; made += 1) {
      await backend.createCard(board.boardId, { title: `More ${made}` });
    }
    await settle();
    render(EmbeddedView, { workspace, router, viewId: view.id, onOpenCard });
    const all = await screen.findByTestId('embedded-view-all');
    await waitFor(() => {
      expect(screen.getAllByTestId('embedded-card')).toHaveLength(EMBEDDED_CARDS);
    });
    expect(all.textContent).toContain(String(board.filteredCardsOf(view).length));
  });

  it('changes a checkbox in place for who may change cards', async () => {
    const { workspace, board, view, router } = await shown();
    const checkbox = board.board?.cardProperties.find((template) => template.type === 'checkbox');
    if (!checkbox || !view.fields.shownProperties.includes(checkbox.id)) return;
    render(EmbeddedView, { workspace, router, viewId: view.id, onOpenCard });
    const frame = await screen.findByTestId('embedded-view');
    const [box] = within(frame).getAllByRole('checkbox');
    if (!box) throw new Error('no checkbox');
    const before = box.getAttribute('aria-checked');
    await fireEvent.click(box);
    await waitFor(() => {
      expect(box.getAttribute('aria-checked')).not.toBe(before);
    });
  });
});

describe('a calendar shown in a body', () => {
  it('lists the days still to come of all its cards, not of the first fifty alone', async () => {
    const made = setup();
    const boardId = [...made.world.blocks.values()].find((one) => one.type === 'card')?.containerId;
    if (!boardId) throw new Error('the seed has no board');
    for (let at = 0; at <= EMBEDDED_CARDS; at += 1) {
      await made.backend.createCard(boardId, { title: `Old ${String(at)}` });
    }
    // Every card began long ago, but the last one in the view's order: today.
    const old = Date.parse('2020-01-01T12:00:00Z');
    const cards = [...made.world.blocks.values()].filter(
      (block) => block.containerId === boardId && block.type === 'card',
    );
    for (const card of cards) made.world.blocks.set(card.id, { ...card, createdAt: old });
    const now = await made.backend.createCard(boardId, { title: 'Today' });
    const [template] = await made.backend.views(boardId);
    if (!template) throw new Error('the board has no view');
    const calendar = await made.backend.createView(boardId, {
      title: 'Calendar',
      fields: {
        ...template.fields,
        viewType: 'calendar',
        groupBy: undefined,
        cardOrder: [...cards.map((card) => card.id), now.id],
      },
    });
    const { workspace } = await openFirstBoard(made);
    render(EmbeddedView, { workspace, router: new Router(''), viewId: calendar.id, onOpenCard });
    const days = await screen.findByTestId('embedded-days');
    await waitFor(() => {
      expect(
        within(days)
          .getAllByTestId('embedded-card')
          .map((link) => link.textContent.trim()),
      ).toEqual(['Today']);
    });
  });
});

describe('the days of a calendar shown in a body', () => {
  const DATE: PropertyTemplate = {
    id: 'due',
    name: 'Due',
    type: 'date',
    options: [],
  } as unknown as PropertyTemplate;
  const card = (id: string, day: string | undefined): Card =>
    ({
      id,
      title: id,
      createdAt: Date.parse('2026-11-01T00:00:00Z'),
      fields: { properties: day ? { due: { from: day } } : {} },
    }) as unknown as Card;
  const today = '2026-11-28' as CalendarDate;

  it('lists the cards still to come by the day each begins on, in the view order within a day', () => {
    const cards = [
      card('late', '2026-12-05'),
      card('past', '2026-11-02'),
      card('soon', '2026-11-30'),
      card('none', undefined),
      card('same', '2026-11-30'),
    ];
    const days = daysOf(cards, 'due', [DATE], today, 'UTC');
    expect(days.map((day) => [day.day, day.cards.map((each) => each.id)])).toEqual([
      ['2026-11-30', ['soon', 'same']],
      ['2026-12-05', ['late']],
    ]);
  });

  it('lists the past when nothing is still to come, and fifty cards at most', () => {
    const past = Array.from({ length: 60 }, (_, at) => card(`p${at}`, '2026-10-01'));
    const days = daysOf(past, 'due', [DATE], today, 'UTC');
    expect(days).toHaveLength(1);
    expect(days[0]?.cards).toHaveLength(EMBEDDED_CARDS);
  });

  it('shows a board as its columns, a calendar by days and anything else as rows', () => {
    expect(
      ['board', 'calendar', 'table', 'gallery', 'gantt'].map((kind) => summaryOf(kind as never)),
    ).toEqual(['columns', 'days', 'rows', 'rows', 'rows']);
    expect(firstCards([1, 2, 3], 2)).toEqual([1, 2]);
  });
});
