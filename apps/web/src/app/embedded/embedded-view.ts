/**
 * What a board view shown in a body (EmbeddedView.svelte) shows of its view:
 * the view's cards as the board shows them (its filter and its sort), at
 * most `EMBEDDED_CARDS` of them, and for a calendar those cards by their
 * days. The rest is on the board, a link away.
 */
import type { Presentation } from '$lib/components/picker-surface/presentation.ts';
import type { PropertyUser } from '$lib/components/properties/index.ts';
import type { TableColumn } from '$lib/features/views/table/columns.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { cardDays, displayProperty } from '$lib/logic/calendar.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';
import type { Regional } from '$lib/logic/regional.ts';
import type { BoardView, Card, PropertyTemplate, ViewType } from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

/** The most cards a board view shows in a body. */
export const EMBEDDED_CARDS = 50;

/** What a kind of view is called, on the badge of a board view. */
export function kindName(viewType: ViewType): string {
  switch (viewType) {
    case 'table':
      return m.board_view_kind_table();
    case 'gallery':
      return m.board_view_kind_gallery();
    case 'calendar':
      return m.board_view_kind_calendar();
    case 'gantt':
      return m.board_view_kind_gantt();
    default:
      return m.board_view_kind_board();
  }
}

/** How a kind of view is shown in a body: a board as its columns, a calendar by days, any other as a table. */
export type Summary = 'columns' | 'days' | 'rows';

export function summaryOf(viewType: ViewType): Summary {
  if (viewType === 'board') return 'columns';
  if (viewType === 'calendar') return 'days';
  return 'rows';
}

/** The first cards of a view, and how many it has. */
export function firstCards<T>(cards: readonly T[], limit = EMBEDDED_CARDS): T[] {
  return cards.slice(0, limit);
}

/** A day of a calendar summary, with its cards. */
export interface Day {
  day: CalendarDate;
  cards: Card[];
}

/**
 * The cards of a calendar by the day each begins on: those still to come or
 * going on (`today` or later) when there are any, else all of them; at most
 * `limit` cards, days in their order and the view's order within a day.
 */
export function daysOf(
  cards: readonly Card[],
  dateProperty: string | undefined,
  templates: readonly PropertyTemplate[],
  today: CalendarDate,
  timeZone: string,
  limit = EMBEDDED_CARDS,
): Day[] {
  const template = displayProperty(dateProperty, templates);
  const dated = cards.flatMap((card) => {
    const days = cardDays(card, template, timeZone);
    return days ? [{ card, ...days }] : [];
  });
  const ahead = dated.filter((each) => each.last >= today);
  const shown = (ahead.length > 0 ? ahead : dated)
    .map((each, index) => ({ ...each, index }))
    .sort((a, b) => (a.first < b.first ? -1 : a.first > b.first ? 1 : a.index - b.index))
    .slice(0, limit);
  const days: Day[] = [];
  for (const { card, first } of shown) {
    const last = days.at(-1);
    if (last?.day === first) last.cards.push(card);
    else days.push({ day: first, cards: [card] });
  }
  return days;
}

/** A day of the calendar as its heading says it, in the reader's language. */
export function dayName(day: CalendarDate, locale: string): string {
  return new Intl.DateTimeFormat(locale, {
    month: 'short',
    day: 'numeric',
    weekday: 'short',
    timeZone: 'UTC',
  }).format(new Date(`${day}T12:00:00Z`));
}

/** What each summary of a view is given: how to show, change and open its cards. */
export interface EmbeddedHost {
  readonly store: BoardStore;
  readonly view: BoardView;
  /** The reader may change cards of the board. */
  readonly canEdit: boolean;
  readonly locale: string;
  readonly regional: Regional;
  /** Everybody a card may name, to show a name by. */
  readonly users: readonly PropertyUser[];
  /** Who a person property can be set to: the members of the board. */
  readonly people: readonly PropertyUser[];
  readonly presentation: Presentation;
  /** The properties the view shows, in its order. */
  readonly columns: readonly TableColumn[];
  cardHref: (cardId: string) => string;
  open: (cardId: string) => void;
}

/**
 * Whether a value is changed where the view is shown in a body: a checkbox,
 * and the board's status, by who may change cards. Anything else is changed
 * on the card.
 */
export function changedHere(host: EmbeddedHost, template: PropertyTemplate): boolean {
  if (!host.canEdit) return false;
  return template.type === 'checkbox' || template.id === host.store.statusProperty?.id;
}
