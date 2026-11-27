/** The columns of a board view: cards grouped by the value of one property. */
import {
  cardProperty,
  DEFAULT_PROPERTY_COLOR,
  type Card,
  type PropertyOption,
  type PropertyTemplate,
} from './wire.ts';

export interface BoardGroup {
  option: PropertyOption;
  cards: Card[];
}

export interface BoardGroups {
  visible: BoardGroup[];
  hidden: BoardGroup[];
}

/** The label of the group of cards that have no value: "No Status", "No Owner". */
export type NoValueLabel = (propertyName: string) => string;

function groupCardsByOptions(
  optionIds: readonly string[],
  optionsById: ReadonlyMap<string, PropertyOption>,
  buckets: ReadonlyMap<string, Card[]>,
  groupBy: PropertyTemplate | undefined,
  noValueLabel: NoValueLabel,
): BoardGroup[] {
  const groups: BoardGroup[] = [];
  for (const optionId of optionIds) {
    if (optionId) {
      // An identifier without an option is a deleted option and gets no column.
      const option = optionsById.get(optionId);
      if (option) {
        groups.push({ option, cards: [...(buckets.get(optionId) ?? [])] });
      }
      continue;
    }
    groups.push({
      option: { id: '', label: noValueLabel(groupBy?.name ?? ''), color: DEFAULT_PROPERTY_COLOR },
      cards: [...(buckets.get('') ?? [])],
    });
  }
  return groups;
}

function getOptionGroups(
  cards: readonly Card[],
  visibleGroups: readonly string[],
  hiddenGroups: readonly string[],
  groupBy: PropertyTemplate | undefined,
  noValueLabel: NoValueLabel,
): BoardGroups {
  const optionsById = new Map<string, PropertyOption>(
    (groupBy?.options ?? []).map((option) => [option.id, option]),
  );
  // Single O(N) pass bucketing every card by its option ID (or '' when unset / deleted) (§1.2.4).
  const buckets = new Map<string, Card[]>();
  for (const card of cards) {
    const raw = groupBy ? cardProperty(card, groupBy.id) : undefined;
    const key = typeof raw === 'string' && raw !== '' && optionsById.has(raw) ? raw : '';
    const list = buckets.get(key);
    if (list) list.push(card);
    else buckets.set(key, [card]);
  }

  // An option the view does not mention yet is shown, after the mentioned ones.
  const shownSet = new Set(visibleGroups);
  const hiddenSet = new Set(hiddenGroups);
  const unassigned = [...optionsById.keys()].filter(
    (id) => !shownSet.has(id) && !hiddenSet.has(id),
  );
  const visible = [...visibleGroups, ...unassigned];
  // Unless the view placed it, the column of cards without a value comes first.
  if (!shownSet.has('') && !hiddenSet.has('')) visible.unshift('');

  return {
    visible: groupCardsByOptions(visible, optionsById, buckets, groupBy, noValueLabel),
    hidden: groupCardsByOptions(hiddenGroups, optionsById, buckets, groupBy, noValueLabel),
  };
}

/**
 * One group per person that occurs, in the order of first occurrence. The
 * cards without a person form the group with the empty identifier, which
 * comes first, like the column of cards without a value of a select property.
 */
function getPersonGroups(
  cards: readonly Card[],
  groupBy: PropertyTemplate,
  hiddenGroups: readonly string[],
  noValueLabel: NoValueLabel,
): BoardGroups {
  const byPerson = new Map<string, Card[]>([['', []]]);
  for (const card of cards) {
    let key = cardProperty(card, groupBy.id);
    if (groupBy.type === 'createdBy') key = card.createdBy;
    else if (groupBy.type === 'updatedBy') key = card.modifiedBy;
    const person = typeof key === 'string' ? key : '';
    const group = byPerson.get(person);
    if (group) group.push(card);
    else byPerson.set(person, [card]);
  }

  const groups: BoardGroups = { visible: [], hidden: [] };
  for (const [person, groupCards] of byPerson) {
    // Without cards there is no "nobody" group, unlike the columns of a select property.
    if (person === '' && groupCards.length === 0) continue;
    const label = person === '' ? noValueLabel(groupBy.name) : person;
    const group: BoardGroup = {
      option: { id: person, label, color: DEFAULT_PROPERTY_COLOR },
      cards: groupCards,
    };
    if (hiddenGroups.includes(person)) groups.hidden.push(group);
    else groups.visible.push(group);
  }
  return groups;
}

/** The visible and the hidden columns of a view, each with its cards in the order given. */
export function getVisibleAndHiddenGroups(
  cards: readonly Card[],
  visibleGroups: readonly string[],
  hiddenGroups: readonly string[],
  groupBy: PropertyTemplate | undefined,
  noValueLabel: NoValueLabel,
): BoardGroups {
  if (
    groupBy &&
    (groupBy.type === 'createdBy' || groupBy.type === 'updatedBy' || groupBy.type === 'person')
  ) {
    return getPersonGroups(cards, groupBy, hiddenGroups, noValueLabel);
  }
  return getOptionGroups(cards, visibleGroups, hiddenGroups, groupBy, noValueLabel);
}
