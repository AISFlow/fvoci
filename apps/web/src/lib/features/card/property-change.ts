/**
 * Changes to the definition of a card property, worked out without touching
 * anything: what the board, its views and its cards have to become when a
 * property is added, changes its kind, or is deleted. `property-actions.ts`
 * sends what these functions work out.
 *
 * A change of kind is the one that can lose something. The stored value of
 * a property has one of a few shapes (a text, the identifier of an option, a
 * list of them, a person, a date, a checkbox), and a value is carried over
 * only where the new kind can read it:
 *
 * | from                 | to                   | what happens to a value                     |
 * | -------------------- | -------------------- | ------------------------------------------- |
 * | text, number, email, | another of these     | stays as it is                              |
 * | phone, link          | select, multi-select | becomes an option of that name              |
 * |                      | checkbox             | stays if it is `true`, else removed         |
 * | select, multi-select | the other of the two | the options stay; a select keeps the first  |
 * |                      | a text kind          | becomes the names of its options            |
 * | person, multi-person | the other of the two | stays; a person keeps the first             |
 * | checkbox             | a text kind          | stays (`true`)                              |
 * | card relation        | anything else        | removed (it names cards, nothing else does) |
 * | anything             | anything else        | removed                                     |
 *
 * A value the new kind cannot read is removed from the cards rather than
 * left there hidden, so that what the confirmation says is what happens; the
 * change can be undone as one step.
 */
import { propertyBehavior } from '$lib/logic/properties.ts';
import {
  createId,
  DEFAULT_PROPERTY_COLOR,
  isFilterGroup,
  plainPropertyValue,
  type BoardView,
  type Card,
  type FilterGroup,
  type PropertyColor,
  type PropertyOption,
  type PropertyTemplate,
  type PropertyType,
  type PropertyValue,
} from '$lib/logic/wire.ts';

/** What the stored value of a kind looks like. `none`: the kind stores nothing on the card. */
type Shape =
  'text' | 'option' | 'options' | 'person' | 'people' | 'date' | 'checkbox' | 'cards' | 'none';

const SHAPES: Record<PropertyType, Shape> = {
  text: 'text',
  number: 'text',
  email: 'text',
  phone: 'text',
  url: 'text',
  select: 'option',
  multiSelect: 'options',
  person: 'person',
  multiPerson: 'people',
  date: 'date',
  checkbox: 'checkbox',
  relation: 'cards',
  file: 'none',
  createdTime: 'none',
  createdBy: 'none',
  updatedTime: 'none',
  updatedBy: 'none',
};

const shapeOf = (type: PropertyType): Shape =>
  (SHAPES as Record<string, Shape | undefined>)[type] ?? 'text';

/** The colour a new option starts with. */
export const NEW_OPTION_COLOR: PropertyColor = DEFAULT_PROPERTY_COLOR;

/** A new option of a select property, with an identifier made here, in the browser. */
export function newOption(name: string): PropertyOption {
  return { id: createId('none'), label: name, color: NEW_OPTION_COLOR };
}

/**
 * A name no property of the board has yet: the given one, or the given one
 * with the lowest number from 2 that is free.
 */
export function freePropertyName(name: string, existing: readonly PropertyTemplate[]): string {
  const taken = new Set(existing.map((template) => template.name));
  if (!taken.has(name)) return name;
  let count = 2;
  while (taken.has(`${name} ${count}`)) count += 1;
  return `${name} ${count}`;
}

/** A new property of a kind, named after the kind. */
export function newProperty(
  type: PropertyType,
  typeName: string,
  existing: readonly PropertyTemplate[],
): PropertyTemplate {
  return { id: createId('block'), name: freePropertyName(typeName, existing), type, options: [] };
}

function hasValue(value: PropertyValue | undefined): value is PropertyValue {
  if (value === undefined || value === '') return false;
  return !(Array.isArray(value) && value.length === 0);
}

/** The identifiers in a stored value of a select or person kind, whichever of the two shapes it has. */
function idsOf(value: PropertyValue): string[] {
  const plain = plainPropertyValue(value);
  if (Array.isArray(plain)) return plain.filter((id) => id !== '');
  return plain === undefined || plain === '' ? [] : [plain];
}

/** The text in a stored value of a text kind. */
function textOf(value: PropertyValue): string {
  const plain = plainPropertyValue(value);
  return (Array.isArray(plain) ? plain.join(', ') : (plain ?? '')).trim();
}

export interface TypeChangePlan {
  /** The definition after the change. */
  template: PropertyTemplate;
  /** The cards whose value has to be written again: the new value, or undefined to remove it. */
  values: Map<string, PropertyValue | undefined>;
  /** How many cards have a value that the new kind still shows. */
  kept: number;
  /** How many cards have a value that is removed. */
  lost: number;
}

/**
 * What changing the kind of a property does: the new definition, and for
 * every card that has a value whether it stays, is rewritten or is removed.
 * See the table at the top of this file.
 */
export function planTypeChange(
  template: PropertyTemplate,
  type: PropertyType,
  cards: readonly Card[],
): TypeChangePlan {
  const from = shapeOf(template.type);
  const to = shapeOf(type);
  const selects = (shape: Shape): boolean => shape === 'option' || shape === 'options';
  const people = (shape: Shape): boolean => shape === 'person' || shape === 'people';

  // Options go along between the two select kinds and are made from texts; any other kind has none.
  const options: PropertyOption[] =
    selects(from) && selects(to) ? template.options.map((option) => ({ ...option })) : [];
  const optionNamed = (name: string): PropertyOption => {
    let option = options.find((existing) => existing.label === name);
    if (!option) {
      option = newOption(name);
      options.push(option);
    }
    return option;
  };
  const one = (ids: string[], list: boolean): PropertyValue | undefined => {
    const first = ids[0];
    if (first === undefined) return undefined;
    return list ? ids : first;
  };

  /** The value under the new kind; `value` itself when it can stay as it is. */
  const convert = (value: PropertyValue): PropertyValue | undefined => {
    if (from === to) return value;
    if (selects(from) && selects(to)) {
      const known = idsOf(value).filter((id) => options.some((option) => option.id === id));
      return one(known, to === 'options');
    }
    if (people(from) && people(to)) return one(idsOf(value), to === 'people');
    if (selects(from) && to === 'text') {
      const names = idsOf(value)
        .map((id) => template.options.find((option) => option.id === id)?.label)
        .filter((name): name is string => name !== undefined && name !== '');
      return names.length === 0 ? undefined : names.join(', ');
    }
    if (from === 'text' && selects(to)) {
      const text = textOf(value);
      if (text === '') return undefined;
      const option = optionNamed(text);
      return to === 'options' ? [option.id] : option.id;
    }
    if (from === 'text' && to === 'checkbox') return textOf(value) === 'true' ? 'true' : undefined;
    if (from === 'checkbox' && to === 'text') return value;
    return undefined;
  };

  const values = new Map<string, PropertyValue | undefined>();
  let kept = 0;
  let lost = 0;
  // A kind that stores nothing left nothing to carry over, whatever lies on the cards.
  if (type !== template.type && from !== 'none') {
    for (const card of cards) {
      const value = card.fields.properties[template.id];
      if (!hasValue(value)) continue;
      const next = convert(value);
      if (next === undefined) lost += 1;
      else kept += 1;
      if (next === undefined || JSON.stringify(next) !== JSON.stringify(value)) {
        values.set(card.id, next);
      }
    }
  }
  return { template: { ...template, type, options }, values, kept, lost };
}

// ------------------------------------------------------------------ views

function withoutClauses(group: FilterGroup, propertyId: string): FilterGroup {
  const filters = group.filters
    .filter((filter) => isFilterGroup(filter) || filter.propertyId !== propertyId)
    .map((filter) => (isFilterGroup(filter) ? withoutClauses(filter, propertyId) : filter))
    // A group that was only about this property has nothing left to say.
    .filter((filter) => !isFilterGroup(filter) || filter.filters.length > 0);
  return { ...group, filters };
}

function without<T>(
  record: Record<string, T>,
  drop: (key: string, value: T) => boolean,
): Record<string, T> {
  return Object.fromEntries(Object.entries(record).filter(([key, value]) => !drop(key, value)));
}

/**
 * A view that no longer leans on a property.
 *
 * Without `becomes` the property is being deleted, and every mention of it
 * goes: the visible properties, the sort, the filter, the grouping, the date
 * a calendar shows cards by, the relation a Gantt chart draws arrows by,
 * the widths and the calculations.
 *
 * With `becomes` the property stays and changes its kind to that. Where it
 * is shown and sorted by stays. What was written for the old kind goes: the
 * filter clauses (their conditions belong to a kind), the calculations, the
 * columns of a grouping (they name options), and the grouping or the
 * calendar date itself when the new kind cannot do that, and the relation a
 * Gantt chart draws arrows by when it is no card relation any more.
 *
 * Returns the view it was given when nothing in it changes.
 */
export function viewWithoutProperty(
  view: BoardView,
  propertyId: string,
  becomes?: PropertyType,
): BoardView {
  const deleting = becomes === undefined;
  const behavior = deleting ? undefined : propertyBehavior(becomes);
  const fields: BoardView['fields'] = {
    ...view.fields,
    filter: withoutClauses(view.fields.filter, propertyId),
    columnCalculations: without(view.fields.columnCalculations, (key) => key === propertyId),
    groupCalculations: without(
      view.fields.groupCalculations,
      (_key, calculation) => calculation.propertyId === propertyId,
    ),
  };
  if (deleting) {
    fields.shownProperties = fields.shownProperties.filter((id) => id !== propertyId);
    fields.sort = fields.sort.filter((option) => option.propertyId !== propertyId);
    fields.columnWidths = without(fields.columnWidths, (key) => key === propertyId);
  }
  // A member that goes is taken out, not set to nothing: a patch names the members it deletes.
  if (fields.groupBy === propertyId) {
    fields.visibleGroups = [];
    fields.hiddenGroups = [];
    fields.collapsedGroups = [];
    if (!behavior?.canGroup) delete fields.groupBy;
  }
  if (fields.dateProperty === propertyId && behavior?.days === undefined) {
    delete fields.dateProperty;
  }
  if (fields.dependencyPropertyId === propertyId && becomes !== 'relation') {
    delete fields.dependencyPropertyId;
  }
  const next: BoardView = { ...view, fields };
  return JSON.stringify(next.fields) === JSON.stringify(view.fields) ? view : next;
}
