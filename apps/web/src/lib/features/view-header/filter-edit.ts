/**
 * Changes to the filter of a view, as the filter editor makes them.
 *
 * The editor shows the conditions that are directly in the filter, one per
 * line. A filter can also hold groups inside the group; nothing here makes
 * one and nothing here takes one apart. Every function returns a new filter
 * in which whatever was not changed is the very object it was, nested groups
 * included, so that saving an edit never drops a part the editor does not
 * show. A function that has nothing to change returns the filter it was
 * given; the caller then has nothing to save.
 */
import { filterConditionValidOrDefault } from '$lib/logic/filter-conditions.ts';
import { propertyBehavior, type FilterValueType } from '$lib/logic/properties.ts';
import {
  isFilterGroup,
  type FilterClause,
  type FilterCondition,
  type FilterGroup,
  type FilterGroupOperation,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

/** The pseudo property of a condition on the title of a card. */
export const TITLE_FILTER_ID = 'title';

/**
 * Every condition, in the order a list of them is shown. Each kind of value
 * offers a run of these (lib/logic/filter-conditions.ts decides which).
 */
const CONDITIONS_IN_ORDER: readonly FilterCondition[] = [
  'includes',
  'notIncludes',
  'isEmpty',
  'isNotEmpty',
  'is',
  'contains',
  'notContains',
  'startsWith',
  'notStartsWith',
  'endsWith',
  'notEndsWith',
  'isBefore',
  'isAfter',
  'isSet',
  'isNotSet',
];

/** The conditions that compare with nothing: they have no value to enter. */
const WITHOUT_VALUE: readonly FilterCondition[] = ['isEmpty', 'isNotEmpty', 'isSet', 'isNotSet'];

/** The properties a condition can be on. The title is offered besides these. */
export function filterableProperties(templates: readonly PropertyTemplate[]): PropertyTemplate[] {
  return templates.filter((template) => propertyBehavior(template.type).canFilter);
}

/** The kind of value a condition on this property compares with. The title is text. */
export function filterValueTypeOf(
  propertyId: string,
  templates: readonly PropertyTemplate[],
): FilterValueType {
  if (propertyId === TITLE_FILTER_ID) return 'text';
  const template = templates.find((candidate) => candidate.id === propertyId);
  return template ? propertyBehavior(template.type).filterValueType : 'none';
}

/**
 * The conditions a kind of value offers, the default first. A condition is
 * offered when the logic keeps it as it is for that kind.
 */
export function conditionsFor(valueType: FilterValueType): FilterCondition[] {
  if (valueType === 'none') return [];
  const offered = CONDITIONS_IN_ORDER.filter(
    (condition) => filterConditionValidOrDefault(valueType, condition) === condition,
  );
  // The default is what a condition the kind does not offer turns into.
  const fallback = filterConditionValidOrDefault(
    valueType,
    CONDITIONS_IN_ORDER.find((condition) => !offered.includes(condition)) ?? 'includes',
  );
  return [fallback, ...offered.filter((condition) => condition !== fallback)];
}

/** Whether a condition is compared with a value the reader enters. */
export function takesValue(condition: FilterCondition): boolean {
  return !WITHOUT_VALUE.includes(condition);
}

/** One line of the editor: a condition and where it is in the filter. */
export interface ClauseEntry {
  /** The place among everything directly in the filter, groups included. */
  index: number;
  clause: FilterClause;
}

/** The conditions directly in the filter, each with its place. */
export function clauseEntries(group: FilterGroup): ClauseEntry[] {
  const entries: ClauseEntry[] = [];
  group.filters.forEach((filter, index) => {
    if (!isFilterGroup(filter)) entries.push({ index, clause: filter });
  });
  return entries;
}

/** How many groups are directly in the filter: the parts the editor shows no lines for. */
export function nestedGroupCount(group: FilterGroup): number {
  return group.filters.filter(isFilterGroup).length;
}

/** How many conditions the filter has in all, those of the groups inside it included. */
export function conditionCount(group: FilterGroup): number {
  return group.filters.reduce(
    (count, filter) => count + (isFilterGroup(filter) ? conditionCount(filter) : 1),
    0,
  );
}

const sameValues = (first: readonly string[], second: readonly string[]): boolean =>
  first.length === second.length && first.every((value, index) => value === second[index]);

/** The filter with the condition at `index` replaced by what `change` makes of it. */
function withClause(
  group: FilterGroup,
  index: number,
  change: (clause: FilterClause) => FilterClause,
): FilterGroup {
  const current = group.filters[index];
  if (!current || isFilterGroup(current)) return group;
  const next = change(current);
  if (next === current) return group;
  return { ...group, filters: group.filters.map((filter, at) => (at === index ? next : filter)) };
}

/**
 * Adds a condition at the end, on the first property that has none yet (the
 * title when every property has one), with that property's default condition
 * and no value: it lets every card through until a value is entered.
 */
export function addClause(group: FilterGroup, templates: readonly PropertyTemplate[]): FilterGroup {
  const used = new Set(clauseEntries(group).map((entry) => entry.clause.propertyId));
  const candidates = [...filterableProperties(templates).map((t) => t.id), TITLE_FILTER_ID];
  const propertyId = candidates.find((id) => !used.has(id)) ?? TITLE_FILTER_ID;
  const condition = filterConditionValidOrDefault(
    filterValueTypeOf(propertyId, templates),
    'includes',
  );
  return { ...group, filters: [...group.filters, { propertyId, condition, values: [] }] };
}

/**
 * Puts the condition at `index` on another property. Its condition stays
 * when the new property's kind of value offers it and becomes that kind's
 * default otherwise; its values belonged to the old property and are dropped.
 */
export function setClauseProperty(
  group: FilterGroup,
  index: number,
  propertyId: string,
  templates: readonly PropertyTemplate[],
): FilterGroup {
  return withClause(group, index, (clause) =>
    clause.propertyId === propertyId
      ? clause
      : {
          propertyId,
          condition: filterConditionValidOrDefault(
            filterValueTypeOf(propertyId, templates),
            clause.condition,
          ),
          values: [],
        },
  );
}

/** Changes the condition at `index`. Its values stay: going back finds them again. */
export function setClauseCondition(
  group: FilterGroup,
  index: number,
  condition: FilterCondition,
): FilterGroup {
  return withClause(group, index, (clause) =>
    clause.condition === condition ? clause : { ...clause, condition },
  );
}

export function setClauseValues(
  group: FilterGroup,
  index: number,
  values: readonly string[],
): FilterGroup {
  return withClause(group, index, (clause) =>
    sameValues(clause.values, values) ? clause : { ...clause, values: [...values] },
  );
}

/** Puts a value into the condition at `index`, or takes it out when it is there. */
export function toggleClauseValue(group: FilterGroup, index: number, value: string): FilterGroup {
  return withClause(group, index, (clause) => ({
    ...clause,
    values: clause.values.includes(value)
      ? clause.values.filter((other) => other !== value)
      : [...clause.values, value],
  }));
}

/** Takes the condition at `index` out. A group at that place is left alone. */
export function removeClause(group: FilterGroup, index: number): FilterGroup {
  const current = group.filters[index];
  if (!current || isFilterGroup(current)) return group;
  return { ...group, filters: group.filters.filter((_, at) => at !== index) };
}

/** Whether every condition has to hold (`and`) or one of them (`or`). */
export function setOperation(group: FilterGroup, operation: FilterGroupOperation): FilterGroup {
  return group.operation === operation ? group : { ...group, operation };
}
