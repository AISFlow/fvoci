/** The summary row of a table and the summary of a board column. */
import { cardUpdatedAt } from './card-time.ts';
import { addDays, daysBetween, daysOf, type CalendarDate } from './dates.ts';
import {
  displayCalendarDate,
  displayDateTime,
  humanizeDuration,
  roundTo,
  type DisplayContext,
} from './format.ts';
import {
  cardProperty,
  parseDateValue,
  plainPropertyValue,
  TITLE_COLUMN_ID,
  type Card,
  type PropertyTemplate,
} from './wire.ts';

const DECIMALS = 2;

export const CALCULATION_IDS = [
  'count',
  'countEmpty',
  'countNotEmpty',
  'percentEmpty',
  'percentNotEmpty',
  'countValue',
  'countUniqueValue',
  'countChecked',
  'countUnchecked',
  'percentChecked',
  'percentUnchecked',
  'sum',
  'average',
  'median',
  'min',
  'max',
  'range',
  'earliest',
  'latest',
  'dateRange',
] as const;

export type CalculationId = (typeof CALCULATION_IDS)[number];

export type Calculation = (
  cards: readonly Card[],
  property: PropertyTemplate,
  context: DisplayContext,
) => string;

/** Times that are shown to the minute count as equal within the minute. */
const toTheMinute = (timestamp: number): number => timestamp - (timestamp % 60000);

function valueOf(card: Card, property: PropertyTemplate): string | string[] | number | undefined {
  if (property.id === TITLE_COLUMN_ID) return card.title;
  switch (property.type) {
    case 'createdBy':
      return card.createdBy;
    case 'createdTime':
      return toTheMinute(card.createdAt);
    case 'updatedBy':
      return card.modifiedBy;
    case 'updatedTime':
      return toTheMinute(cardUpdatedAt(card));
    default:
      return plainPropertyValue(cardProperty(card, property.id));
  }
}

/** The numeric reading of a card's value; NaN when it is not a number. */
const numberOf = (card: Card, property: PropertyTemplate): number =>
  Number.parseFloat(String(valueOf(card, property)));

const withValue = (cards: readonly Card[], property: PropertyTemplate): Card[] =>
  cards.filter((card) => Boolean(valueOf(card, property)));

const percent = (part: number, whole: number): string => `${((part / whole) * 100).toFixed(0)}%`;

/** How many values there are: every item of a multi-select counts. */
function valueCount(cards: readonly Card[], property: PropertyTemplate): number {
  const filled = withValue(cards, property);
  if (property.type !== 'multiSelect') return filled.length;
  return filled.reduce((sum, card) => sum + (valueOf(card, property) as string[]).length, 0);
}

function extremes(
  cards: readonly Card[],
  property: PropertyTemplate,
): { low: number; high: number } {
  let low = Number.POSITIVE_INFINITY;
  let high = Number.NEGATIVE_INFINITY;
  for (const card of withValue(cards, property)) {
    const value = numberOf(card, property);
    low = Math.min(low, value);
    high = Math.max(high, value);
  }
  return { low, high };
}

/** The text for a minimum or maximum; `0` when there was no value at all (`none`). */
const rounded = (value: number, none: number): string =>
  value === none ? '0' : String(roundTo(value, DECIMALS));
const lowest = (value: number): string => rounded(value, Number.POSITIVE_INFINITY);
const highest = (value: number): string => rounded(value, Number.NEGATIVE_INFINITY);

const DAY = 24 * 60 * 60 * 1000;

/** The first and last day over the date values of the cards, as the reader's calendar has them. */
function daySpan(
  cards: readonly Card[],
  property: PropertyTemplate,
  timeZone: string,
): { first: CalendarDate; last: CalendarDate } | undefined {
  let span: { first: CalendarDate; last: CalendarDate } | undefined;
  for (const card of cards) {
    const stored = cardProperty(card, property.id);
    const days = daysOf(parseDateValue(stored), timeZone);
    if (!days) continue;
    span = span
      ? {
          first: days.first < span.first ? days.first : span.first,
          last: days.last > span.last ? days.last : span.last,
        }
      : days;
  }
  return span;
}

/** The earliest and latest of the moments the cards themselves carry (created, updated). */
function momentSpan(
  cards: readonly Card[],
  property: PropertyTemplate,
): { first: number; last: number } {
  let first = Number.POSITIVE_INFINITY;
  let last = Number.NEGATIVE_INFINITY;
  for (const card of cards) {
    const value = valueOf(card, property);
    if (typeof value !== 'number') continue;
    first = Math.min(first, value);
    last = Math.max(last, value);
  }
  return { first, last };
}

/** The text for the earliest (`end` "first") or latest day or moment of a property. */
function showEnd(
  cards: readonly Card[],
  property: PropertyTemplate,
  context: DisplayContext,
  end: 'first' | 'last',
): string {
  if (property.type === 'date') {
    const span = daySpan(cards, property, context.timeZone);
    return span ? displayCalendarDate(span[end], context) : '';
  }
  const moment = momentSpan(cards, property)[end];
  return Number.isFinite(moment) ? displayDateTime(moment, context) : '';
}

/** Whether a checkbox value is ticked. The editor writes `true` for a tick and nothing for none. */
const isChecked = (card: Card, property: PropertyTemplate): boolean =>
  valueOf(card, property) === 'true';

const checkedCount = (cards: readonly Card[], property: PropertyTemplate): number =>
  cards.filter((card) => isChecked(card, property)).length;

function sumOf(cards: readonly Card[], property: PropertyTemplate): number {
  return withValue(cards, property).reduce((sum, card) => sum + numberOf(card, property), 0);
}

function median(cards: readonly Card[], property: PropertyTemplate): string {
  const values = withValue(cards, property)
    .map((card) => numberOf(card, property))
    .sort((a, b) => a - b);
  if (values.length === 0) return '0';
  const middle = Math.floor(values.length / 2);
  const upper = values[middle] ?? 0;
  const result = values.length % 2 === 0 ? (upper + (values[middle - 1] ?? 0)) / 2 : upper;
  return String(roundTo(result, DECIMALS));
}

/** Every calculation by its identifier. Each returns the text to show. */
export const CALCULATIONS: Record<CalculationId, Calculation> = {
  count: (cards) => String(cards.length),
  countEmpty: (cards, property) => String(cards.length - withValue(cards, property).length),
  countNotEmpty: (cards, property) => String(withValue(cards, property).length),
  percentEmpty: (cards, property) =>
    cards.length === 0
      ? ''
      : percent(cards.length - withValue(cards, property).length, cards.length),
  percentNotEmpty: (cards, property) =>
    cards.length === 0 ? '' : percent(withValue(cards, property).length, cards.length),
  countValue: (cards, property) => String(valueCount(cards, property)),
  countUniqueValue: (cards, property) => {
    const unique = new Set<string>();
    for (const card of cards) {
      const value = valueOf(card, property);
      if (!value) continue;
      if (property.type === 'multiSelect') for (const item of value as string[]) unique.add(item);
      else unique.add(String(value));
    }
    return String(unique.size);
  },
  countChecked: (cards, property) => String(checkedCount(cards, property)),
  countUnchecked: (cards, property) => String(cards.length - checkedCount(cards, property)),
  percentChecked: (cards, property) =>
    cards.length === 0 ? '' : percent(checkedCount(cards, property), cards.length),
  percentUnchecked: (cards, property) =>
    cards.length === 0 ? '' : percent(cards.length - checkedCount(cards, property), cards.length),
  sum: (cards, property) => String(roundTo(sumOf(cards, property), DECIMALS)),
  average: (cards, property) => {
    const count = withValue(cards, property).length;
    if (count === 0) return '0';
    return String(roundTo(roundTo(sumOf(cards, property), DECIMALS) / count, DECIMALS));
  },
  median,
  min: (cards, property) => lowest(extremes(cards, property).low),
  max: (cards, property) => highest(extremes(cards, property).high),
  range: (cards, property) => {
    const { low, high } = extremes(cards, property);
    return `${lowest(low)} - ${highest(high)}`;
  },
  earliest: (cards, property, context) => showEnd(cards, property, context, 'first'),
  latest: (cards, property, context) => showEnd(cards, property, context, 'last'),
  dateRange: (cards, property, context) => {
    if (property.type === 'date') {
      // Both ends count: a single day is a range of one day.
      const span = daySpan(cards, property, context.timeZone);
      const days = span ? daysBetween(span.first, addDays(span.last, 1)) : 0;
      return span ? humanizeDuration(days * DAY, context.locale) : '';
    }
    const { first, last } = momentSpan(cards, property);
    return Number.isFinite(first) ? humanizeDuration(last - first, context.locale) : '';
  },
};
