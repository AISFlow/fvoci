/**
 * Cards that repeat (a card's `recurrence`): every `every` days, weeks,
 * months or years, counted from the due date (`due`) or from the day the
 * card is done (`done`). Made done, a card that repeats hands its recurrence
 * to a next card the server makes, due one period on, and names it
 * (`recurredTo`). Only a board with a rule of done and a date property lets
 * a card repeat.
 *
 * The dates are the server's (`crates/model/src/recurrence.rs`), which the
 * in-memory backend follows with these functions: a month on from the 31st
 * is the last day of a shorter month, each step counted from the first date,
 * so that the 31st of January is the 30th of April three months on and the
 * 31st of May four. The next cards of one counted from the due date are a
 * chain whose steps are all counted from the due date of its first (`from`),
 * which the server hands on, so that it keeps the ends of months.
 */
import { wallTimeOf, instantOfWallTime, occurrenceOf } from './date-entry.ts';
import {
  addDays,
  calendarDate,
  dateOfInstant,
  daysBetween,
  parseCalendarDate,
  type CalendarDate,
} from './dates.ts';
import type { PropertyValue } from './wire-blocks.ts';
import type { Container, DoneRule, PropertyTemplate, StoredDateValue } from './wire.ts';

export type RecurrenceUnit = 'day' | 'week' | 'month' | 'year';
export type RecurrenceAnchor = 'due' | 'done';

export interface Recurrence {
  /** How many units apart: from 1 to `MOST_PERIODS`. */
  every: number;
  unit: RecurrenceUnit;
  anchor: RecurrenceAnchor;
  /**
   * The due date of the first card of the chain, which the steps are counted
   * from: the server's, handed to each next card and taken away when how the
   * card repeats or its due date is changed. None for the first card.
   */
  from?: CalendarDate;
}

/** The most units one step may span. */
export const MOST_PERIODS = 365;

export const RECURRENCE_UNITS: readonly RecurrenceUnit[] = ['day', 'week', 'month', 'year'];

/** A recurrence as a card's fields hold it; undefined for anything else. */
export function readRecurrence(value: unknown): Recurrence | undefined {
  if (typeof value !== 'object' || value === null) return undefined;
  const { every, unit, anchor, from } = value as Record<string, unknown>;
  if (typeof every !== 'number' || !Number.isInteger(every)) return undefined;
  if (every < 1 || every > MOST_PERIODS) return undefined;
  if (!RECURRENCE_UNITS.includes(unit as RecurrenceUnit)) return undefined;
  if (anchor !== 'due' && anchor !== 'done') return undefined;
  const first = typeof from === 'string' ? parseCalendarDate(from) : undefined;
  if (from !== undefined && !first) return undefined;
  return { every, unit: unit as RecurrenceUnit, anchor, ...(first ? { from: first } : {}) };
}

/** The same rule, as the first card of a chain has it. */
export function anew({ every, unit, anchor }: Recurrence): Recurrence {
  return { every, unit, anchor };
}

/** Whether both repeat alike, wherever their chains are counted from. */
export function sameRecurrence(first?: Recurrence, second?: Recurrence): boolean {
  return (
    first?.every === second?.every &&
    first?.unit === second?.unit &&
    first?.anchor === second?.anchor
  );
}

/** What the due date editor offers: none, every day, week, month or year, or a recurrence of one's own. */
export type RecurrenceChoice = 'none' | 'daily' | 'weekly' | 'monthly' | 'yearly' | 'custom';

export const RECURRENCE_CHOICES: readonly RecurrenceChoice[] = [
  'none',
  'daily',
  'weekly',
  'monthly',
  'yearly',
  'custom',
];

const PRESETS: Record<Exclude<RecurrenceChoice, 'none' | 'custom'>, RecurrenceUnit> = {
  daily: 'day',
  weekly: 'week',
  monthly: 'month',
  yearly: 'year',
};

/** The choice a recurrence is: one of every unit from the due date, or one's own. */
export function choiceOf(recurrence: Recurrence | undefined): RecurrenceChoice {
  if (!recurrence) return 'none';
  if (recurrence.every !== 1 || recurrence.anchor !== 'due') return 'custom';
  const preset = Object.entries(PRESETS).find(([, unit]) => unit === recurrence.unit);
  return (preset?.[0] as RecurrenceChoice | undefined) ?? 'custom';
}

/** The recurrence a choice makes; `custom` keeps the one there is, or starts every two weeks. */
export function recurrenceOfChoice(
  choice: RecurrenceChoice,
  current?: Recurrence,
): Recurrence | undefined {
  if (choice === 'none') return undefined;
  if (choice === 'custom') return current ?? { every: 2, unit: 'week', anchor: 'due' };
  return { every: 1, unit: PRESETS[choice], anchor: 'due' };
}

/** The date property a board's cards are due by: the one it names, or its first. */
export function duePropertyOf(board: Container): PropertyTemplate | undefined {
  const dates = board.cardProperties.filter((prop) => prop.type === 'date' && prop.id !== '');
  return dates.find((prop) => prop.id === board.dueDatePropertyId) ?? dates[0];
}

/**
 * Whether a card of `board` may repeat: it has a rule of done of a property
 * it has, and names one of its date properties as its due date (the first
 * one it falls back to without a name is not enough).
 */
export function boardLetsCardsRepeat(board: Container): boolean {
  const rule = board.doneWhen;
  return (
    board.cardProperties.some(
      (prop) => prop.type === 'date' && prop.id !== '' && prop.id === board.dueDatePropertyId,
    ) &&
    rule !== null &&
    rule !== undefined &&
    board.cardProperties.some((prop) => prop.id === rule.propertyId)
  );
}

/** Whether `value` of the rule's property makes a card done. */
export function isDoneValue(rule: DoneRule, value: PropertyValue | undefined): boolean {
  if (rule.kind === 'checkbox') return value === 'true';
  const ids = Array.isArray(value) ? value : typeof value === 'string' ? [value] : [];
  return ids.some((id) => typeof id === 'string' && rule.optionIds.includes(id));
}

/**
 * The value of the rule's property the next card starts with, not done, from
 * the completed card's (`current`) and the one it had before it was done
 * (`before`): of a choice of one, the option it was in if that is one and not
 * done, else the first that is not done; of several, those chosen but the
 * done ones; of a checkbox, none.
 */
export function notDoneValue(
  property: PropertyTemplate,
  rule: DoneRule,
  current: PropertyValue | undefined,
  before?: PropertyValue,
): PropertyValue | undefined {
  const done = (id: string): boolean => rule.kind === 'option' && rule.optionIds.includes(id);
  if (property.type === 'select') {
    const was = property.options.find((opt) => opt.id === before && !done(opt.id));
    return (was ?? property.options.find((opt) => !done(opt.id)))?.id;
  }
  if (property.type === 'multiSelect') {
    const kept = (Array.isArray(current) ? current : []).filter(
      (id): id is string => typeof id === 'string' && !done(id),
    );
    return kept.length > 0 ? kept : undefined;
  }
  if (property.type === 'checkbox') return undefined;
  return current;
}

const MONTH_DAYS = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

function daysInMonth(year: number, month: number): number {
  const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  return month === 2 && leap ? 29 : (MONTH_DAYS[month - 1] ?? 31);
}

/** `count` steps of the recurrence after `date`; undefined past the year 9999. */
function stepsAfter(
  date: CalendarDate,
  recurrence: Recurrence,
  count: number,
): CalendarDate | undefined {
  const units = recurrence.every * count;
  if (recurrence.unit === 'day' || recurrence.unit === 'week') {
    const moved = addDays(date, recurrence.unit === 'day' ? units : units * 7);
    return parseCalendarDate(moved);
  }
  const months = recurrence.unit === 'month' ? units : units * 12;
  const total = Number(date.slice(0, 4)) * 12 + Number(date.slice(5, 7)) - 1 + months;
  const year = Math.floor(total / 12);
  const month = (total % 12) + 1;
  if (year > 9999) return undefined;
  const day = Math.min(Number(date.slice(8, 10)), daysInMonth(year, month));
  return calendarDate(year, month, day);
}

/** Whole units from `anchor` to `today`, at most as many as there are. */
function unitsBetween(anchor: CalendarDate, today: CalendarDate, unit: RecurrenceUnit): number {
  if (unit === 'day') return daysBetween(anchor, today);
  if (unit === 'week') return Math.trunc(daysBetween(anchor, today) / 7);
  const part = (date: CalendarDate, from: number, to: number): number =>
    Number(date.slice(from, to));
  const months =
    (part(today, 0, 4) - part(anchor, 0, 4)) * 12 +
    (part(today, 5, 7) - part(anchor, 5, 7)) -
    (part(today, 8, 10) < part(anchor, 8, 10) ? 1 : 0);
  return unit === 'month' ? months : Math.trunc(months / 12);
}

/** The first date after `today` a whole number of steps (one at least) after `anchor`. */
function firstAfter(
  recurrence: Recurrence,
  anchor: CalendarDate,
  today: CalendarDate,
): CalendarDate | undefined {
  const whole = unitsBetween(anchor, today, recurrence.unit);
  let count = Math.max(1, Math.trunc(whole / recurrence.every) - 1);
  for (let tries = 0; tries < 4; tries += 1) {
    const date = stepsAfter(anchor, recurrence, count);
    if (date === undefined) return undefined;
    if (date > today) return date;
    count += 1;
  }
  return undefined;
}

function storedDue(due: PropertyValue | undefined): Partial<StoredDateValue> {
  return typeof due === 'object' && !Array.isArray(due) ? due : {};
}

/** The day a due value begins on in `timeZone`: its first date, or the day of its time. */
function dayOf(due: PropertyValue | undefined, timeZone: string): CalendarDate | undefined {
  const { from } = storedDue(due);
  if (typeof from === 'string') return parseCalendarDate(from);
  return typeof from === 'number' ? wallTimeOf(from, timeZone).date : undefined;
}

/** What the steps after a card due `old` count from: its chain's first, unless that is after it. */
function originOf(recurrence: Recurrence, old: CalendarDate): CalendarDate {
  return recurrence.from !== undefined && recurrence.from <= old ? recurrence.from : old;
}

/**
 * How the next card repeats, after a card due `due` (in `timeZone`'s days),
 * as the server hands it on: counted from the due date, from the first date
 * of the chain, `due`'s own for its first card; from the day it is done, from
 * nothing.
 */
export function handedOn(
  recurrence: Recurrence,
  due: PropertyValue | undefined,
  timeZone: string,
): Recurrence {
  const old = recurrence.anchor === 'due' ? dayOf(due, timeZone) : undefined;
  return { ...anew(recurrence), ...(old ? { from: originOf(recurrence, old) } : {}) };
}

/**
 * The due value of the next card, as the server gives it: of the kind the
 * completed card's is (a date, a range, a time), moved on as the anchor says;
 * a range keeps its length, a time its time of day in `timeZone`. Counted from
 * the due date, it is after the completed card's too, a whole number of steps
 * after the first date of the chain. Without a due date, one step after
 * today. Undefined past the year 9999.
 */
export function nextDueValue(
  recurrence: Recurrence,
  due: PropertyValue | undefined,
  now: number,
  timeZone: string,
): StoredDateValue | undefined {
  const today = dateOfInstant(now, timeZone);
  const nextDate = (old: CalendarDate): CalendarDate | undefined =>
    recurrence.anchor === 'due'
      ? firstAfter(recurrence, originOf(recurrence, old), today > old ? today : old)
      : stepsAfter(today, recurrence, 1);
  const stored = storedDue(due);
  if (typeof stored.from === 'string') {
    const old = parseCalendarDate(stored.from);
    const next = old && nextDate(old);
    if (!old || !next) return undefined;
    if (typeof stored.to !== 'string') return { from: next };
    const end = parseCalendarDate(stored.to);
    const moved = end && parseCalendarDate(addDays(end, daysBetween(old, next)));
    return moved ? { from: next, to: moved } : undefined;
  }
  if (typeof stored.from === 'number') {
    const wall = wallTimeOf(stored.from, timeZone);
    const next = nextDate(wall.date);
    if (!next) return undefined;
    // The clocks read to the minute; the seconds of the time stay as they were.
    const seconds = ((stored.from % 60_000) + 60_000) % 60_000;
    const from =
      instantOfWallTime({ ...wall, date: next }, timeZone, occurrenceOf(stored.from, timeZone)) +
      seconds;
    if (typeof stored.to !== 'number') return { from };
    return { from, to: stored.to + (from - stored.from) };
  }
  const first = stepsAfter(today, recurrence, 1);
  return first ? { from: first } : undefined;
}
