/**
 * What each kind of card property can do.
 *
 * A property template names its kind in `type`; this table says, per kind,
 * whether it can group and filter, which calculations apply, and how a value
 * of it is shown and exported. Code that needs to know about a kind looks it
 * up here instead of switching on the name.
 */
import type { CalculationId } from './calculations.ts';
import { cardUpdatedAt } from './card-time.ts';
import { dateOfInstant, daysOf, type CalendarDate, type DatePoint } from './dates.ts';
import { diagnostics } from './diagnostics.ts';
import { displayCalendarDate, displayDateTime, type DisplayContext } from './format.ts';
import {
  parseDateValue,
  plainPropertyValue,
  relationCardIds,
  type Card,
  type PropertyTemplate,
  type PropertyType,
  type PropertyValue,
} from './wire.ts';

/** Which kind of control edits the value of a filter on this property. */
export type FilterValueType =
  | 'none'
  | 'options'
  | 'boolean'
  | 'text'
  | 'date'
  | 'person'
  /** Only whether there is a value: "is not empty", "is empty". */
  | 'presence';

export type CalculationOption = 'none' | CalculationId;

/** A value as one cell of a CSV file: a text, or a number written as it is (csv.ts writes the file). */
export type ExportCell = string | number;

type Display = (
  value: PropertyValue | undefined,
  card: Card,
  template: PropertyTemplate,
  context: DisplayContext,
) => string | string[] | undefined;

export interface PropertyBehavior {
  canGroup: boolean;
  canFilter: boolean;
  filterValueType: FilterValueType;
  /** The value comes from the card itself and cannot be edited. */
  isReadOnly: boolean;
  calculationOptions: readonly CalculationOption[];
  /** The text shown for a value; a list for several options. */
  displayValue: Display;
  /** The value as one cell of a CSV file: what is shown, several values between commas. */
  exportValue: (
    value: PropertyValue | undefined,
    card: Card,
    template: PropertyTemplate,
    context: DisplayContext,
  ) => ExportCell;
  /**
   * For the date kinds: the first and last day a card occupies in a calendar,
   * as the reader's calendar has them.
   */
  days?: (
    value: PropertyValue | undefined,
    card: Card,
    timeZone: string,
  ) => { first: CalendarDate; last: CalendarDate } | undefined;
}

const COMMON_CALCULATIONS: readonly CalculationOption[] = [
  'none',
  'count',
  'countEmpty',
  'countNotEmpty',
  'percentEmpty',
  'percentNotEmpty',
  'countValue',
  'countUniqueValue',
];
const NUMBER_CALCULATIONS = [
  ...COMMON_CALCULATIONS,
  'sum',
  'average',
  'median',
  'min',
  'max',
  'range',
] as const;
const TIME_CALCULATIONS = [...COMMON_CALCULATIONS, 'earliest', 'latest', 'dateRange'] as const;

/** Between the values of one cell that holds several. */
const EXPORT_SEPARATOR = ', ';

function exportDisplayed(display: Display): PropertyBehavior['exportValue'] {
  return (value, card, template, context) => {
    const shown = display(value, card, template, context);
    return Array.isArray(shown) ? shown.join(EXPORT_SEPARATOR) : (shown ?? '');
  };
}

/** The identifiers of the people a stored value names. */
function storedPeople(value: PropertyValue | undefined): string[] {
  const plain = plainPropertyValue(value);
  if (Array.isArray(plain)) return plain;
  return plain ? [plain] : [];
}

/**
 * The people of a value by the names the reader knows them by
 * (`personName`), never by their identifiers where there is a way to look
 * names up.
 */
function exportPeople(
  people: (value: PropertyValue | undefined, card: Card) => readonly string[],
): PropertyBehavior['exportValue'] {
  return (value, card, _template, context) => {
    const named = context.personName ?? ((id: string) => id);
    return people(value, card).map(named).join(EXPORT_SEPARATOR);
  };
}

/** A number as a spreadsheet reads one; a value that is no number, as it is written. */
function exportNumber(value: PropertyValue | undefined): ExportCell {
  const plain = plainPropertyValue(value);
  if (typeof plain !== 'string' || plain.trim() === '') return '';
  const number = Number(plain);
  return Number.isFinite(number) ? number : plain;
}

function behavior(overrides: Partial<PropertyBehavior>): PropertyBehavior {
  const displayValue: Display = overrides.displayValue ?? plainPropertyValue;
  return {
    canGroup: false,
    canFilter: false,
    filterValueType: 'none',
    isReadOnly: false,
    calculationOptions: COMMON_CALCULATIONS,
    exportValue: exportDisplayed(displayValue),
    ...overrides,
    displayValue,
  };
}

const textLike = behavior({ canFilter: true, filterValueType: 'text' });

const displaySelect: Display = (stored, card, template, context) => {
  const value = plainPropertyValue(stored);
  if (!value) return '';
  const option = template.options.find((candidate) => candidate.id === value);
  if (!option) {
    diagnostics.report(`invalid select option ${String(value)} on card "${card.title}"`);
  }
  return option?.label || (context.unknownOption ?? '(Unknown)');
};

const displayMultiSelect: Display = (stored, card, template) => {
  const value = plainPropertyValue(stored);
  if (!value?.length) return '';
  const options = template.options.filter((option) => value.includes(option.id));
  if (options.length === 0) {
    diagnostics.report(`invalid multi-select options ${String(value)} on card "${card.title}"`);
  }
  return options.map((option) => option.label);
};

/**
 * The titles of the cards a relation names. A card that is not there
 * (deleted) is left out; without a way to look titles up, the identifiers.
 */
const displayRelation: Display = (value, card, _template, context) => {
  const ids = relationCardIds(value).filter((id) => id !== card.id);
  const { cardTitle } = context;
  if (!cardTitle) return ids;
  // An untitled card adds no text, as in a sort.
  return ids.flatMap((id) => cardTitle(id) || []);
};

/** A day as a date, a moment as a date with its time. */
function displayPoint(point: DatePoint, context: DisplayContext): string {
  return point.instant === undefined
    ? displayCalendarDate(point.date, context)
    : displayDateTime(point.instant, context);
}

const displayDateValue: Display = (value, _card, _template, context) => {
  const { from, to } = parseDateValue(value);
  if (!from) return '';
  return to
    ? `${displayPoint(from, context)} -> ${displayPoint(to, context)}`
    : displayPoint(from, context);
};

/** The day a moment of the card itself falls on, as a one-day range. */
const dayOfMoment = (
  instant: number,
  timeZone: string,
): { first: CalendarDate; last: CalendarDate } => {
  const day = dateOfInstant(instant, timeZone);
  return { first: day, last: day };
};

const UNKNOWN_BEHAVIOR = behavior({});

/** The behaviour of every kind of property. `file` has none of its own and behaves as unknown. */
export const PROPERTY_BEHAVIORS: Record<PropertyType, PropertyBehavior> = {
  text: textLike,
  url: textLike,
  email: textLike,
  phone: textLike,
  number: behavior({
    calculationOptions: NUMBER_CALCULATIONS,
    exportValue: exportNumber,
  }),
  select: behavior({
    canGroup: true,
    canFilter: true,
    filterValueType: 'options',
    displayValue: displaySelect,
  }),
  multiSelect: behavior({
    canFilter: true,
    filterValueType: 'options',
    displayValue: displayMultiSelect,
  }),
  date: behavior({
    canFilter: true,
    filterValueType: 'date',
    displayValue: displayDateValue,
    days: (value, _card, timeZone) => daysOf(parseDateValue(value), timeZone),
  }),
  person: behavior({
    canGroup: true,
    canFilter: true,
    filterValueType: 'person',
    exportValue: exportPeople(storedPeople),
  }),
  multiPerson: behavior({
    canFilter: true,
    filterValueType: 'person',
    exportValue: exportPeople(storedPeople),
  }),
  // As a spreadsheet writes a truth value; an unset box is not ticked.
  checkbox: behavior({
    canFilter: true,
    filterValueType: 'boolean',
    exportValue: (value) => (plainPropertyValue(value) === 'true' ? 'TRUE' : 'FALSE'),
  }),
  createdTime: behavior({
    canFilter: true,
    filterValueType: 'date',
    isReadOnly: true,
    calculationOptions: TIME_CALCULATIONS,
    displayValue: (_value, card, _template, context) => displayDateTime(card.createdAt, context),
    days: (_value, card, timeZone) => dayOfMoment(card.createdAt, timeZone),
  }),
  updatedTime: behavior({
    canFilter: true,
    filterValueType: 'date',
    isReadOnly: true,
    calculationOptions: TIME_CALCULATIONS,
    displayValue: (_value, card, _template, context) =>
      displayDateTime(cardUpdatedAt(card), context),
    days: (_value, card, timeZone) => dayOfMoment(cardUpdatedAt(card), timeZone),
  }),
  createdBy: behavior({
    canGroup: true,
    canFilter: true,
    filterValueType: 'person',
    isReadOnly: true,
    exportValue: exportPeople((_value, card) => (card.createdBy ? [card.createdBy] : [])),
  }),
  updatedBy: behavior({
    canGroup: true,
    canFilter: true,
    filterValueType: 'person',
    isReadOnly: true,
    exportValue: exportPeople((_value, card) => (card.modifiedBy ? [card.modifiedBy] : [])),
  }),
  // Not grouped by and not calculated; a filter asks only whether there is a value.
  relation: behavior({
    canFilter: true,
    filterValueType: 'presence',
    calculationOptions: ['none'],
    displayValue: displayRelation,
  }),
  file: UNKNOWN_BEHAVIOR,
};

/** The behaviour of a kind; a kind this app does not know has no special behaviour. */
export function propertyBehavior(type: string): PropertyBehavior {
  return (
    (PROPERTY_BEHAVIORS as Record<string, PropertyBehavior | undefined>)[type] ?? UNKNOWN_BEHAVIOR
  );
}

/** The kinds a person can choose when adding a property, in menu order. */
export const SELECTABLE_PROPERTY_TYPES: readonly PropertyType[] = [
  'text',
  'number',
  'email',
  'phone',
  'url',
  'select',
  'multiSelect',
  'date',
  'person',
  'multiPerson',
  'checkbox',
  'createdTime',
  'createdBy',
  'updatedTime',
  'updatedBy',
  'relation',
];
