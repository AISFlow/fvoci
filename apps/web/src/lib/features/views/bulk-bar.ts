/**
 * What the bar of a selection (BulkBar.svelte) offers, worked out without a
 * page: which properties have a button of their own and which are in its
 * menu, and the value the selected cards share, if they share one.
 */
import type { Card, PropertyTemplate, PropertyType, PropertyValue } from '$lib/logic/wire.ts';

/**
 * How much more room the lists of a view leave below their last card while
 * the bar is there (`--bulk-room`, beside `--fab-room`): the bar and its gap.
 */
export const BULK_ROOM = '4rem';

/**
 * The kinds of property the bar sets on many cards. Not a relation: the
 * cards it names differ card by card. Not what the card itself says (who
 * made it, when).
 */
const BULK_KINDS: ReadonlySet<PropertyType> = new Set<PropertyType>([
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
]);

/**
 * Where the focus goes once the bar let the selection go by its own button
 * (✕, Delete), which goes with it: the first of `cardIds` still in the view,
 * unless they were deleted, else the first other card. `cards`: the elements
 * of the view's cards (`[data-card]`), in its order.
 */
export function cardAfter(
  cards: readonly HTMLElement[],
  cardIds: readonly string[],
  deleted: boolean,
): HTMLElement | undefined {
  const named = (card: HTMLElement): boolean => cardIds.includes(card.dataset.card ?? '');
  return (deleted ? undefined : cards.find(named)) ?? cards.find((card) => !named(card));
}

export function isBulkKind(type: PropertyType): boolean {
  return BULK_KINDS.has(type);
}

export interface BulkProperties {
  /** The first person property: whom the cards are given to. */
  assignee?: PropertyTemplate;
  /** The property the view groups by, else the board's status. */
  status?: PropertyTemplate;
  /** The board's due date. */
  due?: PropertyTemplate;
  /** Every other property the bar can set, in the board's order. */
  others: PropertyTemplate[];
}

/** The properties of the bar, from the board's definitions and the ids of those with a role. */
export function bulkProperties(
  templates: readonly PropertyTemplate[],
  roles: { groupBy?: string; status?: string; due?: string | null },
): BulkProperties {
  const settable = templates.filter((template) => isBulkKind(template.type));
  const byId = (id: string | null | undefined): PropertyTemplate | undefined =>
    id ? settable.find((template) => template.id === id) : undefined;
  const status = byId(roles.groupBy) ?? byId(roles.status);
  // A view grouped by a person has that person as its status: the next one is whom cards are given to.
  const assignee = settable.find(
    (template) =>
      (template.type === 'person' || template.type === 'multiPerson') && template.id !== status?.id,
  );
  const dated = byId(roles.due);
  const due = dated?.type === 'date' && dated.id !== status?.id ? dated : undefined;
  const taken = new Set([assignee?.id, status?.id, due?.id]);
  return {
    assignee,
    status,
    due,
    others: settable.filter((template) => !taken.has(template.id)),
  };
}

/** A value with its objects' keys in order: two copies of one value write the same. */
function ordered(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(ordered);
  if (typeof value !== 'object' || value === null) return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, ordered((value as Record<string, unknown>)[key])]),
  );
}

/** No value, an empty text and an empty list are all no value. */
function isEmpty(value: PropertyValue | undefined): boolean {
  return value === undefined || value === '' || (Array.isArray(value) && value.length === 0);
}

/** The value the cards share; `mixed` where two of them differ. No value is a value they can share. */
export interface SharedValue {
  mixed: boolean;
  value: PropertyValue | undefined;
}

export function sharedValue(cards: readonly Card[], propertyId: string): SharedValue {
  const [first, ...rest] = cards.map((card) => card.fields.properties[propertyId]);
  const key = (value: PropertyValue | undefined): string =>
    isEmpty(value) ? '' : JSON.stringify(ordered(value));
  const shared = key(first);
  const mixed = rest.some((value) => key(value) !== shared);
  return { mixed, value: mixed || isEmpty(first) ? undefined : first };
}
