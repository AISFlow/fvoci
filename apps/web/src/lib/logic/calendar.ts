/**
 * The arithmetic of a calendar view: which days the grid of a month shows,
 * which property places the cards, which days each card is on, and where the
 * bar of a card goes in a week.
 *
 * Days are calendar dates (dates.ts). Nothing here reads a clock or the time
 * zone of the device: which day a moment falls on is decided by the reader's
 * time zone, passed in, and the first day of the week is an argument.
 */
import { withDay } from './date-entry.ts';
import {
  addDays,
  calendarDate,
  comparePoints,
  dateOfInstant,
  dateOfPoint,
  daysBetween,
  daysOf,
  type CalendarDate,
  type DatePoint,
  type DateValue,
} from './dates.ts';
import { propertiesThatMeetFilterGroup } from './filter.ts';
import { propertyBehavior } from './properties.ts';
import type { Weekday } from './regional.ts';
import {
  cardProperty,
  formatDateValue,
  parseDateValue,
  type Card,
  type FilterGroup,
  type PropertyTemplate,
  type PropertyValue,
} from './wire.ts';

// ------------------------------------------------------------- months

export interface Month {
  year: number;
  /** 1 to 12. */
  month: number;
}

export function monthOf(date: CalendarDate): Month {
  return { year: Number(date.slice(0, 4)), month: Number(date.slice(5, 7)) };
}

export function firstOf(month: Month): CalendarDate {
  return calendarDate(month.year, month.month, 1);
}

export function lastOf(month: Month): CalendarDate {
  // Day 0 of the next month rolls back to the last day of this one.
  return calendarDate(month.year, month.month + 1, 0);
}

export function shiftMonth(month: Month, by: number): Month {
  return monthOf(calendarDate(month.year, month.month + by, 1));
}

export function inMonth(date: CalendarDate, month: Month): boolean {
  return date >= firstOf(month) && date <= lastOf(month);
}

/** The day of the week as ISO 8601 numbers it: 1 is Monday, 7 is Sunday. */
export function weekdayOf(date: CalendarDate): Weekday {
  // 1970-01-01 was a Thursday.
  const sinceThursday = daysBetween(calendarDate(1970, 1, 1), date);
  return (((((sinceThursday + 3) % 7) + 7) % 7) + 1) as Weekday;
}

/** The seven days of the week `date` is in, for a reader whose week starts on `weekStart`. */
export function weekOf(date: CalendarDate, weekStart: Weekday): CalendarDate[] {
  const back = (weekdayOf(date) - weekStart + 7) % 7;
  const first = addDays(date, -back);
  return Array.from({ length: 7 }, (_, at) => addDays(first, at));
}

/** The weeks the grid of a month shows: every week with a day of the month in it, four to six. */
export function weeksOfMonth(month: Month, weekStart: Weekday): CalendarDate[][] {
  const weeks: CalendarDate[][] = [];
  const last = lastOf(month);
  for (let day = firstOf(month); ; day = addDays(day, 7)) {
    const week = weekOf(day, weekStart);
    if ((week[0] ?? day) > last) break;
    weeks.push(week);
  }
  return weeks;
}

// ---------------------------------------------------- what places a card

/**
 * The property a calendar view places its cards by: the one the view names,
 * if it still is a kind of date; otherwise the first date property of the
 * board; otherwise the first property that is a moment (created or updated
 * time). Undefined when the board has none of these: the cards are then
 * placed by when they were made.
 */
export function displayProperty(
  dateProperty: string | undefined,
  templates: readonly PropertyTemplate[],
): PropertyTemplate | undefined {
  const dated = (template: PropertyTemplate): boolean =>
    propertyBehavior(template.type).days !== undefined;
  const named = templates.find((template) => template.id === dateProperty);
  if (named && dated(named)) return named;
  return templates.find((template) => template.type === 'date') ?? templates.find(dated);
}

/** The properties a calendar can be placed by, in the order of the board. */
export function displayableProperties(templates: readonly PropertyTemplate[]): PropertyTemplate[] {
  return templates.filter((template) => propertyBehavior(template.type).days !== undefined);
}

/** The days a card is on: the same day twice when it is not a range. */
export interface Span {
  id: string;
  first: CalendarDate;
  last: CalendarDate;
  /** The moment it starts, where its value carries a time of day. */
  time?: number;
}

/** The first and last day of a card for a reader in `timeZone`; undefined when it has no date. */
export function cardDays(
  card: Card,
  template: PropertyTemplate | undefined,
  timeZone: string,
): { first: CalendarDate; last: CalendarDate } | undefined {
  if (!template) {
    const day = dateOfInstant(card.createdAt, timeZone);
    return { first: day, last: day };
  }
  const days = propertyBehavior(template.type).days;
  return days?.(cardProperty(card, template.id), card, timeZone);
}

/** The moment a stored date value starts at, where it carries a time of day. */
function startOf(stored: unknown): number | undefined {
  return parseDateValue(stored).from?.instant;
}

/** The cards that have days, as spans in the order given, and those that have none. */
export function spansOf(
  cards: readonly Card[],
  template: PropertyTemplate | undefined,
  timeZone: string,
): { spans: Span[]; undated: string[] } {
  const spans: Span[] = [];
  const undated: string[] = [];
  for (const card of cards) {
    const days = cardDays(card, template, timeZone);
    if (!days) {
      undated.push(card.id);
      continue;
    }
    const time = template?.type === 'date' ? startOf(cardProperty(card, template.id)) : undefined;
    spans.push(time === undefined ? { id: card.id, ...days } : { id: card.id, ...days, time });
  }
  return { spans, undated };
}

/**
 * The ids of the cards on each of `days`, in the order of the spans. One pass
 * over the cards, not one per day: a month has 42 days and a board hundreds
 * of cards.
 */
export function cardsByDay(
  spans: readonly Span[],
  days: readonly CalendarDate[],
): Map<CalendarDate, string[]> {
  const result = new Map<CalendarDate, string[]>(days.map((day) => [day, []]));
  const first = days[0];
  const last = days.at(-1);
  if (!first || !last) return result;
  for (const span of spans) {
    if (span.last < first || span.first > last) continue;
    const from = span.first < first ? first : span.first;
    const to = span.last > last ? last : span.last;
    for (let day = from; day <= to; day = addDays(day, 1)) result.get(day)?.push(span.id);
  }
  return result;
}

// --------------------------------------------------------------- bars

/** The part of a card's span that falls in one week, as a bar. */
export interface Segment {
  id: string;
  /** The row of bars it is in, from 0 at the top. */
  lane: number;
  /** The first column it covers, 0 to 6, and how many. */
  column: number;
  length: number;
  /** The card began before this week, or goes on after it. */
  fromBefore: boolean;
  toAfter: boolean;
}

/**
 * The order of cards in a week and in the list of a day: the one that began
 * earlier, then the longer one, then one without a time before one with a
 * time, and the earlier time first. Cards still equal keep the order given
 * (the view's order of cards); sorts that use this are stable.
 */
export function compareSpans(a: Span, b: Span): number {
  if (a.first !== b.first) return a.first < b.first ? -1 : 1;
  const longer = daysBetween(b.first, b.last) - daysBetween(a.first, a.last);
  if (longer !== 0) return longer;
  if (a.time === undefined || b.time === undefined) {
    return Number(a.time !== undefined) - Number(b.time !== undefined);
  }
  return Math.sign(a.time - b.time);
}

/**
 * The bars of one week, in the order of `compareSpans`; each takes the highest row that is free
 * on all its days. Rows are worked out a week at a time: a card that spans
 * two weeks may sit in different rows of the two.
 */
export function layWeek(week: readonly CalendarDate[], spans: readonly Span[]): Segment[] {
  const start = week[0];
  const end = week[6];
  if (!start || !end) return [];
  const here = spans
    .map((span, order) => ({ span, order }))
    .filter(({ span }) => span.first <= end && span.last >= start)
    .sort((a, b) => compareSpans(a.span, b.span) || a.order - b.order);
  /** For each row, the columns taken. */
  const taken: boolean[][] = [];
  return here.map(({ span }) => {
    const column = Math.max(0, daysBetween(start, span.first));
    const until = Math.min(6, daysBetween(start, span.last));
    let lane = 0;
    const free = (row: boolean[] | undefined): boolean =>
      !row || row.slice(column, until + 1).every((cell) => !cell);
    while (!free(taken[lane])) lane += 1;
    const row = (taken[lane] ??= Array.from({ length: 7 }, () => false));
    for (let at = column; at <= until; at += 1) row[at] = true;
    return {
      id: span.id,
      lane,
      column,
      length: until - column + 1,
      fromBefore: span.first < start,
      toAfter: span.last > end,
    };
  });
}

export interface Folded {
  shown: Segment[];
  /** For each column, how many cards of that day have no bar. */
  more: number[];
}

const covers = (segment: Segment, column: number): boolean =>
  column >= segment.column && column < segment.column + segment.length;

/**
 * What of a week's bars is drawn when a cell has room for `lanes` rows (at
 * least one). When a day has more cards than rows, its last row says "+N"
 * instead of a bar; a bar in the last row stays only if none of its days
 * needs that row for "+N". A bar that does not fit is hidden in the whole
 * week, not cut at the day that is full.
 */
export function foldWeek(segments: readonly Segment[], lanes: number): Folded {
  const rows = Math.max(1, lanes);
  const depth = Array.from({ length: 7 }, () => 0);
  for (const segment of segments) {
    for (let column = segment.column; column < segment.column + segment.length; column += 1) {
      depth[column] = Math.max(depth[column] ?? 0, segment.lane + 1);
    }
  }
  const fits = (segment: Segment): boolean => {
    if (segment.lane < rows - 1) return true;
    if (segment.lane > rows - 1) return false;
    for (let column = segment.column; column < segment.column + segment.length; column += 1) {
      if ((depth[column] ?? 0) > rows) return false;
    }
    return true;
  };
  const shown: Segment[] = [];
  const more = Array.from({ length: 7 }, () => 0);
  for (const segment of segments) {
    if (fits(segment)) {
      shown.push(segment);
      continue;
    }
    for (let column = 0; column < 7; column += 1) {
      if (covers(segment, column)) more[column] = (more[column] ?? 0) + 1;
    }
  }
  return { shown, more };
}

// ------------------------------------------------------- changing a date

/**
 * Whether the dates of a calendar can be changed on it: only when its cards
 * are placed by a date property. When they were made or last changed is
 * written by the server, and a board with no date at all places its cards
 * by when they were made.
 */
export function datesWritable(template: PropertyTemplate | undefined): boolean {
  return template?.type === 'date';
}

/**
 * A date value moved whole so that its first day is `first`, for a reader in
 * `timeZone`. Both ends move by the same number of days, so a
 * range keeps its length in days; a moment keeps its time of day on the
 * reader's clock (`withDay`). A range stored backwards comes out in order. An
 * empty value becomes that day.
 *
 * On a day the clocks skip, a time in the skipped hour lands later
 * (date-entry.ts); an end that would then come before the start keeps the
 * range's length in time instead.
 */
export function movedValue(value: DateValue, first: CalendarDate, timeZone: string): DateValue {
  const days = daysOf(value, timeZone);
  if (!value.from || !days) return { from: { date: first } };
  const by = daysBetween(days.first, first);
  const shift = (point: DatePoint): DatePoint =>
    withDay(point, addDays(dateOfPoint(point, timeZone), by), timeZone);
  const backwards = value.to !== undefined && comparePoints(value.from, value.to, timeZone) > 0;
  const start = backwards && value.to ? value.to : value.from;
  const end = backwards ? value.from : value.to;
  const from = shift(start);
  if (!end) return { from };
  const to = shift(end);
  if (
    from.instant !== undefined &&
    to.instant !== undefined &&
    start.instant !== undefined &&
    end.instant !== undefined &&
    to.instant < from.instant
  ) {
    return { from, to: { instant: from.instant + (end.instant - start.instant) } };
  }
  return { from, to };
}

/** Which end of a date value a drag takes: the first day, or the last. */
export type ValueEnd = 'start' | 'end';

/**
 * A date value with one end moved to `day`, for a reader in `timeZone`: the
 * first day (`start`) or the last (`end`); the other end stays as it is. A
 * moment keeps its time of day on the reader's clock (`withDay`). An end
 * never passes the other: one moved beyond it stops on the other's day, and
 * on a time before the other's it takes the other's time. A value of one
 * day becomes a range when an end is moved off its day; a range whose ends
 * come to the same day without a time, or to the same moment, becomes one
 * day. A range stored backwards comes out in order. An empty value becomes
 * that day.
 */
export function resizedValue(
  value: DateValue,
  end: ValueEnd,
  day: CalendarDate,
  timeZone: string,
): DateValue {
  if (!value.from) return { from: { date: day } };
  const backwards = value.to !== undefined && comparePoints(value.from, value.to, timeZone) > 0;
  let start = backwards && value.to ? value.to : value.from;
  let finish = (backwards ? value.from : value.to) ?? value.from;
  if (end === 'start') {
    const last = dateOfPoint(finish, timeZone);
    start = withDay(start, day < last ? day : last, timeZone);
    if (comparePoints(start, finish, timeZone) > 0) start = finish;
  } else {
    const first = dateOfPoint(start, timeZone);
    finish = withDay(finish, day > first ? day : first, timeZone);
    if (comparePoints(start, finish, timeZone) > 0) finish = start;
  }
  const one =
    start.instant === undefined
      ? finish.instant === undefined && start.date === finish.date
      : start.instant === finish.instant;
  return one ? { from: start } : { from: start, to: finish };
}

/** What of a card's days a drag takes: all of them, or one end. */
export type BarGrip = 'move' | ValueEnd;

const FIRST_DAY = calendarDate(1, 1, 1);
const LAST_DAY = calendarDate(9999, 12, 31);

/**
 * The days of a card dragged `by` days at `grip`: all of them moved, or the
 * first or the last day moved and stopped at the other. Kept among the days
 * that can be written (the years 1 to 9999).
 */
export function draggedDays(
  days: { first: CalendarDate; last: CalendarDate },
  grip: BarGrip,
  by: number,
): { first: CalendarDate; last: CalendarDate } {
  if (grip === 'start') {
    const most = daysBetween(days.first, days.last);
    const least = -daysBetween(FIRST_DAY, days.first);
    return { first: addDays(days.first, Math.min(most, Math.max(least, by))), last: days.last };
  }
  if (grip === 'end') {
    const least = -daysBetween(days.first, days.last);
    const most = daysBetween(days.last, LAST_DAY);
    return { first: days.first, last: addDays(days.last, Math.min(most, Math.max(least, by))) };
  }
  const least = -daysBetween(FIRST_DAY, days.first);
  const most = daysBetween(days.last, LAST_DAY);
  const shift = Math.min(most, Math.max(least, by));
  return { first: addDays(days.first, shift), last: addDays(days.last, shift) };
}

/**
 * The values of a card made on a day of the calendar: the
 * values the view's filter asks for, as a new card on a board or in a table
 * gets them, and that day for the date the calendar is placed by. The day
 * wins over the filter: where the filter asks for another date, the card is
 * made on the day it was asked on and the filter hides it, as it would hide
 * any card that does not meet it.
 */
export function newCardProperties(
  filter: FilterGroup | undefined,
  templates: readonly PropertyTemplate[],
  template: PropertyTemplate | undefined,
  day: CalendarDate,
): Record<string, PropertyValue> {
  const properties: Record<string, PropertyValue> = propertiesThatMeetFilterGroup(
    filter,
    templates,
  );
  const value = formatDateValue({ from: { date: day } });
  if (template && datesWritable(template) && value) properties[template.id] = value;
  return properties;
}
