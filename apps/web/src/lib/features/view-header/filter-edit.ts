/**
 * Changes to the filter of a view, as the filter editor makes them.
 *
 * A filter is a group of conditions and of groups, nested as deep as the
 * model allows (eight). The editor makes and changes two levels of it: the
 * conditions and the groups directly in the filter, and the conditions of
 * those groups. A group deeper than that is shown as being there and kept
 * as it is. A part of the filter is found by its path: the places, one per
 * level, that lead to it from the filter (`[2, 0]`: the first part of the
 * third). The path of the filter itself is empty.
 *
 * Every function returns a new filter in which whatever was not changed is
 * the very object it was, nested groups included, so that saving an edit
 * never drops a part the editor does not show. A function that has nothing
 * to change returns the filter it was given; the caller then has nothing to
 * save.
 */
import { filterConditionValidOrDefault } from '$lib/logic/filter-conditions.ts';
import { DONE_FILTER_ID, doneRuleOf } from '$lib/logic/filter-done.ts';
import { propertyBehavior, type FilterValueType } from '$lib/logic/properties.ts';
import {
  isFilterGroup,
  type DoneRule,
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
  'isWithin',
  'isSet',
  'isNotSet',
];

/** The conditions that compare with nothing: they have no value to enter. */
const WITHOUT_VALUE: readonly FilterCondition[] = ['isEmpty', 'isNotEmpty', 'isSet', 'isNotSet'];

/** The properties a condition can be on. The title is offered besides these. */
export function filterableProperties(templates: readonly PropertyTemplate[]): PropertyTemplate[] {
  return templates.filter((template) => propertyBehavior(template.type).canFilter);
}

/**
 * What a condition can be on, in the order the list shows them: the title,
 * whether a card is done where the board has a rule of done, then the
 * properties a condition can be on.
 */
export function filterPropertyIds(
  templates: readonly PropertyTemplate[],
  doneWhen: DoneRule | null | undefined,
): string[] {
  const done = doneRuleOf(doneWhen, templates) ? [DONE_FILTER_ID] : [];
  return [TITLE_FILTER_ID, ...done, ...filterableProperties(templates).map((each) => each.id)];
}

/**
 * The kind of value a condition on this property compares with. The title
 * is text; whether a card is done is a yes or a no, as a checkbox is.
 */
export function filterValueTypeOf(
  propertyId: string,
  templates: readonly PropertyTemplate[],
): FilterValueType {
  if (propertyId === TITLE_FILTER_ID) return 'text';
  if (propertyId === DONE_FILTER_ID) return 'boolean';
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

/** Where a part of the filter is: its place in each group on the way to it. */
export type FilterPath = readonly number[];

/** A part of a filter: a condition or a group. */
type FilterNode = FilterClause | FilterGroup;

/** How deep the groups the editor makes and changes are: those directly in the filter. */
export const EDITED_GROUP_DEPTH = 1;

/** One part of the filter as the editor shows it, with its path. */
export type FilterEntry =
  | { kind: 'clause'; path: FilterPath; clause: FilterClause }
  | { kind: 'group'; path: FilterPath; group: FilterGroup; entries: FilterEntry[] }
  /** A group deeper than the editor goes: shown as being there, kept as it is. */
  | { kind: 'kept'; path: FilterPath; group: FilterGroup };

/** A condition as the editor shows it. */
export type ClauseEntry = Extract<FilterEntry, { kind: 'clause' }>;
/** A group the editor makes and changes. */
export type GroupEntry = Extract<FilterEntry, { kind: 'group' }>;

/** The parts of a group (the filter, by default), each with its path, groups with theirs. */
export function filterEntries(group: FilterGroup, path: FilterPath = []): FilterEntry[] {
  return group.filters.map((node, index): FilterEntry => {
    const at = [...path, index];
    if (!isFilterGroup(node)) return { kind: 'clause', path: at, clause: node };
    if (at.length > EDITED_GROUP_DEPTH) return { kind: 'kept', path: at, group: node };
    return { kind: 'group', path: at, group: node, entries: filterEntries(node, at) };
  });
}

/** A path as one string, to key the lines of a list by. */
export function pathKey(path: FilterPath): string {
  return path.join('.');
}

/** The part of the filter at `path`: the filter itself for the empty path. */
export function nodeAt(group: FilterGroup, path: FilterPath): FilterNode | undefined {
  let node: FilterNode | undefined = group;
  for (const index of path) {
    node = node && isFilterGroup(node) ? node.filters[index] : undefined;
  }
  return node;
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

/**
 * The filter with the part at `path` replaced by what `change` makes of it,
 * or taken out where `change` makes nothing of it. A path that leads to no
 * part changes nothing.
 */
function withNode(
  group: FilterGroup,
  path: FilterPath,
  change: (node: FilterNode) => FilterNode | undefined,
): FilterGroup {
  const [index, ...rest] = path;
  const current = index === undefined ? undefined : group.filters[index];
  if (current === undefined) return group;
  let next: FilterNode | undefined;
  if (rest.length === 0) next = change(current);
  else next = isFilterGroup(current) ? withNode(current, rest, change) : current;
  if (next === current) return group;
  const filters =
    next === undefined
      ? group.filters.filter((_, at) => at !== index)
      : group.filters.map((filter, at) => (at === index ? next : filter));
  return { ...group, filters };
}

/** The filter with the group at `path` (the filter itself when empty) changed. */
function withGroup(
  root: FilterGroup,
  path: FilterPath,
  change: (group: FilterGroup) => FilterGroup,
): FilterGroup {
  if (path.length === 0) return change(root);
  return withNode(root, path, (node) => (isFilterGroup(node) ? change(node) : node));
}

/** The filter with the condition at `path` changed; a group there is left alone. */
function withClause(
  root: FilterGroup,
  path: FilterPath,
  change: (clause: FilterClause) => FilterClause | undefined,
): FilterGroup {
  return withNode(root, path, (node) => (isFilterGroup(node) ? node : change(node)));
}

/**
 * A new condition for a group: on the first property that has none directly
 * in it yet (the title when every property has one), with that property's
 * default condition and no value. It lets every card through until a value
 * is entered.
 */
function newClause(group: FilterGroup, templates: readonly PropertyTemplate[]): FilterClause {
  const used = new Set(
    group.filters.flatMap((filter) => (isFilterGroup(filter) ? [] : [filter.propertyId])),
  );
  const candidates = [...filterableProperties(templates).map((t) => t.id), TITLE_FILTER_ID];
  const propertyId = candidates.find((id) => !used.has(id)) ?? TITLE_FILTER_ID;
  const condition = filterConditionValidOrDefault(
    filterValueTypeOf(propertyId, templates),
    'includes',
  );
  return { propertyId, condition, values: [] };
}

/** Adds a condition at the end of the group at `at`: the filter itself, by default. */
export function addClause(
  root: FilterGroup,
  templates: readonly PropertyTemplate[],
  at: FilterPath = [],
): FilterGroup {
  return withGroup(root, at, (group) => ({
    ...group,
    filters: [...group.filters, newClause(group, templates)],
  }));
}

/**
 * Adds a group at the end of the group at `at` (the filter, by default),
 * with one new condition in it. It asks the other of what the group it is
 * in asks: any one of its conditions where that one asks all, and the
 * other way round. A group that asked the same would be the same as its
 * conditions without it.
 */
export function addGroup(
  root: FilterGroup,
  templates: readonly PropertyTemplate[],
  at: FilterPath = [],
): FilterGroup {
  return withGroup(root, at, (group) => {
    const operation: FilterGroupOperation = group.operation === 'or' ? 'and' : 'or';
    const added: FilterGroup = { operation, filters: [newClause(EMPTY_GROUP, templates)] };
    return { ...group, filters: [...group.filters, added] };
  });
}
const EMPTY_GROUP: FilterGroup = { operation: 'and', filters: [] };

/**
 * Puts the condition at `path` on another property. Its condition stays
 * when the new property's kind of value offers it and becomes that kind's
 * default otherwise; its values belonged to the old property and are dropped.
 */
export function setClauseProperty(
  root: FilterGroup,
  path: FilterPath,
  propertyId: string,
  templates: readonly PropertyTemplate[],
): FilterGroup {
  return withClause(root, path, (clause) =>
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

/** Changes the condition at `path`. Its values stay: going back finds them again. */
export function setClauseCondition(
  root: FilterGroup,
  path: FilterPath,
  condition: FilterCondition,
): FilterGroup {
  return withClause(root, path, (clause) =>
    clause.condition === condition ? clause : { ...clause, condition },
  );
}

export function setClauseValues(
  root: FilterGroup,
  path: FilterPath,
  values: readonly string[],
): FilterGroup {
  return withClause(root, path, (clause) =>
    sameValues(clause.values, values) ? clause : { ...clause, values: [...values] },
  );
}

/** Puts a value into the condition at `path`, or takes it out when it is there. */
export function toggleClauseValue(root: FilterGroup, path: FilterPath, value: string): FilterGroup {
  return withClause(root, path, (clause) => ({
    ...clause,
    values: clause.values.includes(value)
      ? clause.values.filter((other) => other !== value)
      : [...clause.values, value],
  }));
}

/**
 * Takes the condition at `path` out. A group at that place is left alone. A
 * group the condition was the last of goes with it: an empty group lets
 * every card through, which in a filter of "any one" is all of them.
 */
export function removeClause(root: FilterGroup, path: FilterPath): FilterGroup {
  const next = withClause(root, path, () => undefined);
  if (next === root) return root;
  const parentPath = path.slice(0, -1);
  const parent = nodeAt(next, parentPath);
  const emptied =
    parentPath.length > 0 && parent && isFilterGroup(parent) && !parent.filters.length;
  return emptied ? withNode(next, parentPath, () => undefined) : next;
}

/** Whether every part of the group at `path` has to hold (`and`) or one of them (`or`). */
export function setOperation(
  root: FilterGroup,
  path: FilterPath,
  operation: FilterGroupOperation,
): FilterGroup {
  return withGroup(root, path, (group) =>
    group.operation === operation ? group : { ...group, operation },
  );
}

/** Takes the group at `path` out, with everything in it. The filter itself stays. */
export function removeGroup(root: FilterGroup, path: FilterPath): FilterGroup {
  return withNode(root, path, (node) => (isFilterGroup(node) ? undefined : node));
}

/**
 * Takes the group at `path` apart: its parts take its place in the group it
 * is in, in their order, and are asked as that group asks.
 */
export function ungroup(root: FilterGroup, path: FilterPath): FilterGroup {
  const index = path.at(-1);
  if (index === undefined) return root;
  return withGroup(root, path.slice(0, -1), (parent) => {
    const group = parent.filters[index];
    if (!group || !isFilterGroup(group)) return parent;
    const { filters } = parent;
    return {
      ...parent,
      filters: [...filters.slice(0, index), ...group.filters, ...filters.slice(index + 1)],
    };
  });
}
