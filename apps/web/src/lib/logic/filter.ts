/** Deciding which cards a view's filter lets through. */
import { cardUpdatedAt } from './card-time.ts';
import { arraysEqual } from './arrays.ts';
import { daysOf, type DateValue } from './dates.ts';
import { diagnostics } from './diagnostics.ts';
import { foldCase } from './text.ts';
import {
  cardProperty,
  DYNAMIC_FILTER_ME,
  isFilterGroup,
  parseDateValue,
  parseFilterDate,
  plainPropertyValue,
  relationCardIds,
  type Card,
  type FilterClause,
  type FilterGroup,
  type PropertyTemplate,
  type PropertyValue,
} from './wire.ts';

/** What evaluating a filter depends on besides the card. */
export interface FilterContext {
  /** The reader's time zone, an IANA name such as `Asia/Seoul`: decides which day a moment is on. */
  timeZone: string;
  /** The reader's user ID, used to resolve dynamic `'me'` person/author filters. */
  meId?: string | undefined;
  /** Optional current moment in milliseconds, used when evaluating `'today'`. */
  now?: number | undefined;
  /** The title of a card of the board; undefined for one that is not there (deleted). */
  cardTitle?: (cardId: string) => string | undefined;
}

/** Whether two clauses filter the same property in the same way. */
export function areEqual(first: FilterClause, second: FilterClause): boolean {
  return (
    first.propertyId === second.propertyId &&
    first.condition === second.condition &&
    arraysEqual(first.values, second.values)
  );
}

export function applyFilterGroup(
  group: FilterGroup,
  templates: readonly PropertyTemplate[],
  cards: readonly Card[],
  context: FilterContext,
): Card[] {
  return cards.filter((card) => isFilterGroupMet(group, templates, card, context));
}

/** A group without filters lets every card through. */
export function isFilterGroupMet(
  group: FilterGroup,
  templates: readonly PropertyTemplate[],
  card: Card,
  context: FilterContext,
): boolean {
  if (group.filters.length < 1) return true;
  const met = (filter: FilterClause | FilterGroup): boolean =>
    isFilterGroup(filter)
      ? isFilterGroupMet(filter, templates, card, context)
      : isClauseMet(filter, templates, card, context);
  return group.operation === 'or' ? group.filters.some(met) : group.filters.every(met);
}

/**
 * The value a clause is compared with. A property without a stored value
 * falls back to what the card itself knows: who created or changed it. For
 * the date kinds `dateValue` is the value as a date: the stored one, or the
 * moment the card was created or changed.
 */
function valueForClause(
  filter: FilterClause,
  template: PropertyTemplate | undefined,
  card: Card,
  context: FilterContext,
): { value: PropertyValue | undefined; dateValue: DateValue | undefined } {
  let value = cardProperty(card, filter.propertyId);
  if (filter.propertyId === 'title') value = card.title;

  switch (template?.type) {
    case 'date':
      return { value, dateValue: parseDateValue(value) };
    case 'createdTime':
      return { value: String(card.createdAt), dateValue: { from: { instant: card.createdAt } } };
    case 'updatedTime':
      return {
        value: String(cardUpdatedAt(card)),
        dateValue: { from: { instant: cardUpdatedAt(card) } },
      };
    case 'createdBy':
      return { value: value || card.createdBy, dateValue: undefined };
    case 'updatedBy':
      return { value: value || card.modifiedBy, dateValue: undefined };
    case 'relation': {
      // A card that is not there is not shown, and does not count.
      const { cardTitle } = context;
      const ids = relationCardIds(value).filter((id) => id !== card.id);
      return {
        value: cardTitle ? ids.filter((id) => cardTitle(id) !== undefined) : ids,
        dateValue: undefined,
      };
    }
    default:
      return { value, dateValue: undefined };
  }
}

/**
 * Whether a card meets one clause.
 *
 * A clause that cannot be evaluated (no value to compare with, an unknown
 * condition) is met: an unfinished filter must not hide every card.
 *
 * Text is compared without regard to case, for the title and for every kind
 * of property alike (see `foldCase`). Of a value that is a list, one item has
 * to match.
 *
 * Dates are compared as calendar days, in the reader's time zone where a
 * moment is involved: "is the 7th" finds a card created at any time on the
 * reader's 7th, and a range that includes the 7th.
 */
export function isClauseMet(
  filter: FilterClause,
  templates: readonly PropertyTemplate[],
  card: Card,
  context: FilterContext,
): boolean {
  const template = templates.find((candidate) => candidate.id === filter.propertyId);
  const { value, dateValue } = valueForClause(filter, template, card, context);
  const isPersonLike =
    template?.type === 'person' ||
    template?.type === 'multiPerson' ||
    template?.type === 'createdBy' ||
    template?.type === 'updatedBy';
  const resolvedValues = isPersonLike
    ? filter.values.flatMap((v) =>
        v === DYNAMIC_FILTER_ME ? (context.meId ? [context.meId] : []) : [v],
      )
    : filter.values;
  const first = resolvedValues[0];

  // Text or a list (a date value counts by its key); anything else counts as empty.
  const plain = plainPropertyValue(value);
  const stored = typeof plain === 'string' || Array.isArray(plain) ? plain : '';
  const hasAnyValue = (): boolean =>
    resolvedValues.some((candidate) =>
      Array.isArray(stored) ? stored.includes(candidate) : candidate === stored,
    );
  const needle = foldCase(first ?? '');
  const texts = (Array.isArray(stored) ? stored : [stored]).map(foldCase);
  const someText = (test: (text: string) => boolean): boolean => texts.some(test);

  const days = dateValue ? daysOf(dateValue, context.timeZone) : undefined;
  const day = parseFilterDate(filter.values[0], context.timeZone, context.now);

  switch (filter.condition) {
    case 'includes':
      return filter.values.length < 1 || hasAnyValue();
    case 'notIncludes':
      return filter.values.length < 1 || !hasAnyValue();
    case 'isEmpty':
      return stored.length <= 0;
    case 'isNotEmpty':
      return stored.length > 0;
    case 'isSet':
      return dateValue ? days !== undefined : Boolean(value);
    case 'isNotSet':
      return dateValue ? days === undefined : !value;
    case 'is':
      if (first === undefined) return true;
      if (!dateValue) return someText((text) => text === needle);
      if (day === undefined) return true;
      return days !== undefined && days.first <= day && day <= days.last;
    case 'contains':
      return first === undefined || someText((text) => text.includes(needle));
    case 'notContains':
      return first === undefined || !someText((text) => text.includes(needle));
    case 'startsWith':
      return first === undefined || someText((text) => text.startsWith(needle));
    case 'notStartsWith':
      return first === undefined || !someText((text) => text.startsWith(needle));
    case 'endsWith':
      return first === undefined || someText((text) => text.endsWith(needle));
    case 'notEndsWith':
      return first === undefined || !someText((text) => text.endsWith(needle));
    case 'isBefore':
      if (first === undefined || day === undefined) return true;
      // A range is before a day when it starts before it.
      return days !== undefined && days.first < day;
    case 'isAfter':
      if (first === undefined || day === undefined) return true;
      // A range is after a day when it ends after it.
      return days !== undefined && days.last > day;
    default:
      diagnostics.report(`invalid filter condition: ${String(filter.condition)}`);
      return true;
  }
}

/**
 * Property values a new card needs in order to pass the filter, so that a
 * card added to a filtered view does not vanish at once. Only the clauses
 * directly in the group are looked at, and of an `or` group only the first.
 */
export function propertiesThatMeetFilterGroup(
  group: FilterGroup | undefined,
  templates: readonly PropertyTemplate[],
  meId?: string,
): Record<string, string> {
  if (!group) return {};
  const clauses = group.filters.filter((filter): filter is FilterClause => !isFilterGroup(filter));
  const considered = group.operation === 'or' ? clauses.slice(0, 1) : clauses;
  const result: Record<string, string> = {};
  for (const clause of considered) {
    const property = propertyThatMeetsFilterClause(clause, templates, meId);
    if (property.value) result[property.id] = property.value;
  }
  return result;
}

/** The value that makes a card meet the clause, when there is an obvious one. */
export function propertyThatMeetsFilterClause(
  clause: FilterClause,
  templates: readonly PropertyTemplate[],
  meId?: string,
): { id: string; value?: string } {
  const id = clause.propertyId;
  const template = templates.find((candidate) => candidate.id === id);
  if (!template) {
    diagnostics.report(`propertyThatMeetsFilterClause: no template for property ${id}`);
    return { id };
  }
  if (template.type === 'createdBy' || template.type === 'updatedBy') return { id };

  if (clause.condition === 'includes') {
    const raw = clause.values[0];
    const first =
      raw === DYNAMIC_FILTER_ME && (template.type === 'person' || template.type === 'multiPerson')
        ? meId
        : raw;
    return first === undefined ? { id } : { id, value: first };
  }
  if (clause.condition === 'isNotEmpty' && template.type === 'select') {
    const option = template.options[0];
    return option ? { id, value: option.id } : { id };
  }
  return { id };
}
