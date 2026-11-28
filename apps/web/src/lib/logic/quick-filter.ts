/**
 * The quick filters of a board: chips a reader turns on to narrow the cards
 * of whatever view they look at, for themselves alone. They are not the
 * view's filter, which every member of the board sees: the view is not
 * changed, and nothing of them goes to the server.
 *
 * - `mine`: a person property of the card names the reader.
 * - `incomplete`: the card is not done by the board's rule (`doneWhen`).
 * - `overdue`: the card is due before the reader's today and not done.
 * - `dueThisWeek`: the card is due in the reader's week (`regional.weekStart`).
 *
 * A card is due on the last day of its due date: a range is due when it
 * ends. The due date is the board's `dueDatePropertyId` when that is a date
 * property, else ("automatic") the board's first date property, as the
 * server and the Today hub have it (`effective_due_date_property_id`).
 * Chips that are on all have to hold.
 */
import { weekOf } from './calendar.ts';
import { dateOfInstant, daysOf, type CalendarDate } from './dates.ts';
import { doneRuleOf, isCardDone } from './filter-done.ts';
import type { Weekday } from './regional.ts';
import { cardProperty, parseDateValue, type Card, type Container, type DoneRule } from './wire.ts';

export const QUICK_FILTERS = ['mine', 'incomplete', 'overdue', 'dueThisWeek'] as const;

export type QuickFilterId = (typeof QUICK_FILTERS)[number];

export function isQuickFilterId(value: unknown): value is QuickFilterId {
  return QUICK_FILTERS.some((id) => id === value);
}

/** What the chips read of a board. */
export type QuickFilterBoard = Pick<Container, 'cardProperties' | 'doneWhen' | 'dueDatePropertyId'>;

/** What the chips read of the reader and the moment. */
export interface QuickFilterContext {
  /** The reader; none on a public link, where nobody is "me". */
  meId?: string | undefined;
  timeZone: string;
  weekStart: Weekday;
  /** The current moment in milliseconds; the clock's when not given. */
  now?: number | undefined;
  /** The reader's today, when the caller knows it already; else worked out from `now`. */
  today?: CalendarDate | undefined;
}

/** The person properties of a board: those a card names its people in. */
function personPropertyIds(board: QuickFilterBoard): string[] {
  return board.cardProperties
    .filter((template) => template.type === 'person' || template.type === 'multiPerson')
    .map((template) => template.id);
}

/** The board's due date property: the one it names, else its first date property. */
function duePropertyId(board: QuickFilterBoard): string | undefined {
  const dates = board.cardProperties.filter((template) => template.type === 'date');
  return (dates.find((template) => template.id === board.dueDatePropertyId) ?? dates[0])?.id;
}

/**
 * The chips a board offers its reader, in the order they are shown: "mine"
 * to a reader who is signed in, on a board with a person property; the
 * others where the board has what they read (a rule of done, a due date).
 */
export function offeredQuickFilters(
  board: QuickFilterBoard | undefined,
  meId: string | undefined,
): QuickFilterId[] {
  if (!board) return [];
  const hasDone = doneRuleOf(board.doneWhen, board.cardProperties) !== undefined;
  const hasDue = duePropertyId(board) !== undefined;
  const offered: Record<QuickFilterId, boolean> = {
    mine: meId !== undefined && personPropertyIds(board).length > 0,
    incomplete: hasDone,
    overdue: hasDue,
    dueThisWeek: hasDue,
  };
  return QUICK_FILTERS.filter((id) => offered[id]);
}

/** What the chips read of the board and the day, worked out once for all the cards. */
interface Prepared {
  readonly meId: string | undefined;
  readonly people: readonly string[];
  readonly rule: DoneRule | undefined;
  readonly dueId: string | undefined;
  readonly timeZone: string;
  readonly today: CalendarDate;
  /** The first and the last day of the reader's week. */
  readonly week: readonly [CalendarDate, CalendarDate];
}

function prepare(board: QuickFilterBoard, context: QuickFilterContext): Prepared {
  const today = context.today ?? dateOfInstant(context.now ?? Date.now(), context.timeZone);
  const week = weekOf(today, context.weekStart);
  return {
    meId: context.meId,
    people: personPropertyIds(board),
    rule: doneRuleOf(board.doneWhen, board.cardProperties),
    dueId: duePropertyId(board),
    timeZone: context.timeZone,
    today,
    week: [week[0] ?? today, week[6] ?? today],
  };
}

/** Whether a property value names `userId`: a person, or one of several. */
function names(value: unknown, userId: string): boolean {
  return Array.isArray(value) ? value.includes(userId) : value === userId;
}

function meets(id: QuickFilterId, prepared: Prepared, card: Card): boolean {
  const { meId, rule, dueId } = prepared;
  const done = (): boolean => rule !== undefined && isCardDone(rule, card);
  // The day a card is due: the last day of its due date.
  const due = (): CalendarDate | undefined =>
    dueId === undefined
      ? undefined
      : daysOf(parseDateValue(cardProperty(card, dueId)), prepared.timeZone)?.last;
  switch (id) {
    case 'mine':
      if (meId === undefined) return true;
      return prepared.people.some((propertyId) => names(cardProperty(card, propertyId), meId));
    case 'incomplete':
      return !done();
    case 'overdue': {
      const day = due();
      return day !== undefined && day < prepared.today && !done();
    }
    case 'dueThisWeek': {
      const day = due();
      return day !== undefined && prepared.week[0] <= day && day <= prepared.week[1];
    }
  }
}

/** Whether `card` meets the chip `id`. A chip the board cannot evaluate lets every card through. */
export function isQuickFilterMet(
  id: QuickFilterId,
  board: QuickFilterBoard,
  card: Card,
  context: QuickFilterContext,
): boolean {
  return meets(id, prepare(board, context), card);
}

/**
 * The cards that meet every chip of `chosen` the board offers, in their
 * order. A chip it does not offer (one turned on for it before, on a board
 * that has no due date now) is left out: it is not shown, so it cannot be
 * turned off. A card `kept` is let through whatever the chips are: one the
 * reader made on this visit does not vanish as it is made.
 */
export function applyQuickFilters(
  cards: readonly Card[],
  chosen: readonly QuickFilterId[],
  board: QuickFilterBoard | undefined,
  context: QuickFilterContext,
  kept: (card: Card) => boolean = () => false,
): Card[] {
  if (!board || chosen.length === 0) return [...cards];
  const offered = offeredQuickFilters(board, context.meId);
  const applied = chosen.filter((id) => offered.includes(id));
  if (applied.length === 0) return [...cards];
  const prepared = prepare(board, context);
  return cards.filter((card) => kept(card) || applied.every((id) => meets(id, prepared, card)));
}
