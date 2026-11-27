/**
 * The calculations of the summary row: their names, which of them a column
 * can have, and the result for the cards shown.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { CALCULATIONS, type CalculationId } from '$lib/logic/calculations.ts';
import type { DisplayContext } from '$lib/logic/format.ts';
import { propertyBehavior, type CalculationOption } from '$lib/logic/properties.ts';
import { TITLE_COLUMN_ID, type Card, type PropertyTemplate } from '$lib/logic/wire.ts';

interface Names {
  /** In the menu: "Count unique values". */
  label: () => string;
  /** In the cell, next to the result: "Unique". */
  short: () => string;
}

const NAMES: Record<CalculationOption, Names> = {
  none: {
    label: m['Calculations.Options.none.label'],
    short: m['Calculations.Options.none.displayName'],
  },
  count: {
    label: m['Calculations.Options.count.label'],
    short: m['Calculations.Options.count.displayName'],
  },
  countEmpty: {
    label: m.table_calculation_count_empty,
    short: m.table_calculation_empty,
  },
  countNotEmpty: {
    label: m.table_calculation_count_not_empty,
    short: m.table_calculation_not_empty,
  },
  percentEmpty: {
    label: m.table_calculation_percent_empty,
    short: m.table_calculation_empty,
  },
  percentNotEmpty: {
    label: m.table_calculation_percent_not_empty,
    short: m.table_calculation_not_empty,
  },
  countValue: {
    label: m['Calculations.Options.countValue.label'],
    short: m['Calculations.Options.countValue.displayName'],
  },
  countUniqueValue: {
    label: m['Calculations.Options.countUniqueValue.label'],
    short: m['Calculations.Options.countUniqueValue.displayName'],
  },
  countChecked: {
    label: m['Calculations.Options.countChecked.label'],
    short: m['Calculations.Options.countChecked.displayName'],
  },
  countUnchecked: {
    label: m['Calculations.Options.countUnchecked.label'],
    short: m['Calculations.Options.countUnchecked.displayName'],
  },
  percentChecked: {
    label: m['Calculations.Options.percentChecked.label'],
    short: m['Calculations.Options.percentChecked.displayName'],
  },
  percentUnchecked: {
    label: m['Calculations.Options.percentUnchecked.label'],
    short: m['Calculations.Options.percentUnchecked.displayName'],
  },
  sum: {
    label: m['Calculations.Options.sum.label'],
    short: m['Calculations.Options.sum.displayName'],
  },
  average: {
    label: m['Calculations.Options.average.label'],
    short: m['Calculations.Options.average.displayName'],
  },
  median: {
    label: m['Calculations.Options.median.label'],
    short: m['Calculations.Options.median.displayName'],
  },
  min: {
    label: m['Calculations.Options.min.label'],
    short: m['Calculations.Options.min.displayName'],
  },
  max: {
    label: m['Calculations.Options.max.label'],
    short: m['Calculations.Options.max.displayName'],
  },
  range: {
    label: m['Calculations.Options.range.label'],
    short: m['Calculations.Options.range.displayName'],
  },
  earliest: {
    label: m['Calculations.Options.earliest.label'],
    short: m['Calculations.Options.earliest.displayName'],
  },
  latest: {
    label: m['Calculations.Options.latest.label'],
    short: m['Calculations.Options.latest.displayName'],
  },
  dateRange: {
    label: m['Calculations.Options.dateRange.label'],
    short: m['Calculations.Options.dateRange.displayName'],
  },
};

const isOption = (value: string): value is CalculationOption => value in NAMES;

/** The title column as a property: it counts as a text. */
export function titleProperty(name: string): PropertyTemplate {
  return { id: TITLE_COLUMN_ID, name, type: 'text', options: [] };
}

/**
 * The calculation a column has: what the view stores for it, or, where it
 * stores none, the count for the title column and none for the others. A stored
 * name this app does not know, or one the kind of the column cannot have, is
 * none.
 */
export function calculationOf(
  stored: Readonly<Record<string, string>>,
  property: PropertyTemplate,
): CalculationOption {
  const value = stored[property.id] ?? (property.id === TITLE_COLUMN_ID ? 'count' : 'none');
  return isOption(value) && calculationOptions(property).includes(value) ? value : 'none';
}

/**
 * What the summary row of a table offers on top of what the kind itself
 * lists: the ticks of a checkbox column and the ends of a date column.
 */
const TABLE_EXTRAS: Partial<Record<PropertyTemplate['type'], readonly CalculationOption[]>> = {
  checkbox: ['countChecked', 'countUnchecked', 'percentChecked', 'percentUnchecked'],
  date: ['earliest', 'latest', 'dateRange'],
};

/** The calculations the kind of a column can have, in the order of the menu. */
export function calculationOptions(property: PropertyTemplate): readonly CalculationOption[] {
  const own = propertyBehavior(property.type).calculationOptions;
  const extras = (TABLE_EXTRAS[property.type] ?? []).filter((option) => !own.includes(option));
  return extras.length > 0 ? [...own, ...extras] : own;
}

export const calculationLabel = (option: CalculationOption): string => NAMES[option].label();
export const calculationShortName = (option: CalculationOption): string => NAMES[option].short();

/** The result for the cards the view shows, as text. Empty for no calculation. */
export function calculationResult(
  option: CalculationOption,
  cards: readonly Card[],
  property: PropertyTemplate,
  context: DisplayContext,
): string {
  if (option === 'none') return '';
  const id: CalculationId = option;
  return CALCULATIONS[id](cards, property, context);
}
