/**
 * The order of the cards of a view, and of the views of a board.
 *
 * Every comparator here is a proper ordering: comparing two cards the other
 * way round gives the opposite answer, and no two different cards compare as
 * equal (the identifier decides in the end). So the result of a sort does
 * not depend on the order the cards came in, or on how the engine sorts.
 * tests/logic/sort-properties.test.ts checks this on generated data.
 */
import { cardUpdatedAt } from './card-time.ts';
import { comparePoints, type DatePoint } from './dates.ts';
import { diagnostics } from './diagnostics.ts';
import {
  cardProperty,
  parseDateValue,
  plainPropertyValue,
  relationCardIds,
  TITLE_COLUMN_ID,
  type BoardView,
  type Card,
  type PropertyTemplate,
  type SortOption,
} from './wire.ts';

type Comparator = (first: Card, second: Card) => number;

const byId: Comparator = (first, second) =>
  first.id === second.id ? 0 : first.id < second.id ? -1 : 1;

/** Oldest first; cards made in the same millisecond by identifier. */
export const createdOrder: Comparator = (first, second) =>
  Math.sign(first.createdAt - second.createdAt) || byId(first, second);

function titleComparator(collator: Intl.Collator): Comparator {
  return (first, second) => {
    // Cards without a title go last.
    if (!first.title || !second.title) {
      return Number(!first.title) - Number(!second.title) || createdOrder(first, second);
    }
    return collator.compare(first.title, second.title) || createdOrder(first, second);
  };
}

/** By title in the alphabetical order of `locale`; cards without a title go last; equal titles oldest first. */
export function titleOrCreatedOrder(first: Card, second: Card, locale: string): number {
  return titleComparator(new Intl.Collator(locale))(first, second);
}

/**
 * By the position the cards were dragged to. Cards that were never placed
 * come after all placed ones, oldest first.
 */
export function manualComparator(cardOrder: readonly string[]): Comparator {
  const position = new Map<string, number>();
  cardOrder.forEach((id, index) => {
    // An identifier listed twice counts at its first place.
    if (!position.has(id)) position.set(id, index);
  });
  const of = (card: Card): number => position.get(card.id) ?? Number.POSITIVE_INFINITY;
  return (first, second) => {
    const a = of(first);
    const b = of(second);
    if (a !== b) return a < b ? -1 : 1;
    return createdOrder(first, second);
  };
}

export function manualOrder(cardOrder: readonly string[], first: Card, second: Card): number {
  return manualComparator(cardOrder)(first, second);
}

/** What sorting needs to know besides the cards. */
export interface SortContext {
  templates: readonly PropertyTemplate[];
  sort: readonly SortOption[];
  /** The manual order, used when there are no sort options. */
  cardOrder: readonly string[];
  /** The username of a user, for sorting by person. */
  usernameOf?: (userId: string) => string | undefined;
  /** The language whose alphabetical order text is sorted in. */
  locale: string;
  /** The reader's time zone, an IANA name: decides which day a date with a time is on. */
  timeZone: string;
  /** The title of a card of the board; undefined for one that is not there (deleted). */
  cardTitle?: (cardId: string) => string | undefined;
}

/** The value a card is sorted by, per kind of property: absent when the card has none. */
type SortKey = string | number | DatePoint | undefined;

function sortKey(card: Card, template: PropertyTemplate, context: SortContext): SortKey {
  const value = cardProperty(card, template.id);
  const stored = plainPropertyValue(value);
  const username = (userId: string): string => context.usernameOf?.(userId) ?? userId;
  const text = (value: string | undefined): SortKey => (value ? value : undefined);

  switch (template.type) {
    case 'createdTime':
      return card.createdAt;
    case 'updatedTime':
      return cardUpdatedAt(card);
    case 'createdBy':
      return text(context.usernameOf?.(card.createdBy));
    case 'updatedBy':
      return text(context.usernameOf?.(card.modifiedBy));
    case 'number': {
      const number =
        typeof stored === 'string' && stored.trim() !== '' ? Number(stored) : Number.NaN;
      return Number.isNaN(number) ? undefined : number;
    }
    case 'date':
      return parseDateValue(value).from;
    case 'select':
    case 'multiSelect': {
      const id = Array.isArray(stored) ? stored[0] : stored;
      return text(template.options.find((option) => option.id === id)?.label);
    }
    case 'person':
    case 'multiPerson': {
      const ids = Array.isArray(stored) ? stored : stored ? [stored] : [];
      return text(ids.map(username).join(', '));
    }
    case 'relation': {
      // By the titles shown, in the order picked; cards that are not there are not shown.
      // An untitled card adds nothing, as untitled cards go last by title.
      const { cardTitle } = context;
      const ids = relationCardIds(value).filter((id) => id !== card.id);
      const titles = cardTitle ? ids.flatMap((id) => cardTitle(id) || []) : ids;
      return text(titles.join(', '));
    }
    default:
      return text(Array.isArray(stored) ? stored.join(', ') : stored);
  }
}

/**
 * The comparator of one sort option. Cards without a value come after those
 * with one whichever way the option runs; `reversed` only turns the order
 * among cards that have a value. Returns 0 when the option does not tell the
 * two cards apart.
 */
function optionComparator(
  option: SortOption,
  context: SortContext,
  collator: Intl.Collator,
): Comparator | undefined {
  const direction = option.reversed ? -1 : 1;

  if (option.propertyId === TITLE_COLUMN_ID) {
    return (first, second) => {
      if (!first.title || !second.title) return Number(!first.title) - Number(!second.title);
      return direction * collator.compare(first.title, second.title);
    };
  }

  const template = context.templates.find((candidate) => candidate.id === option.propertyId);
  if (!template) {
    diagnostics.report(`sortCards: no template for property ${option.propertyId}`);
    return undefined;
  }
  // A key is worked out once per card and sort: a relation's key looks up titles.
  const keys = new Map<Card, SortKey>();
  const keyOf = (card: Card): SortKey => {
    if (keys.has(card)) return keys.get(card);
    const key = sortKey(card, template, context);
    keys.set(card, key);
    return key;
  };
  return (first, second) => {
    const a = keyOf(first);
    const b = keyOf(second);
    if (a === undefined || b === undefined)
      return Number(a === undefined) - Number(b === undefined);
    if (typeof a === 'number' && typeof b === 'number') return direction * Math.sign(a - b);
    if (typeof a === 'string' && typeof b === 'string') return direction * collator.compare(a, b);
    if (typeof a === 'object' && typeof b === 'object') {
      return direction * comparePoints(a, b, context.timeZone);
    }
    return 0;
  };
}

/**
 * The cards in the order the view asks for. The list passed in is not
 * changed.
 *
 * Without sort options the order is the manual one. With them, the first
 * option decides; where it does not tell two cards apart the second does,
 * and so on; cards equal in every option are ordered by title, then by age.
 * An option on a property that no longer exists is skipped.
 */
export function sortCards(cards: readonly Card[], context: SortContext): Card[] {
  return [...cards].sort(cardComparator(context));
}

/** The comparator `sortCards` sorts with. */
export function cardComparator(context: SortContext): Comparator {
  if (context.sort.length < 1) return manualComparator(context.cardOrder);

  const collator = new Intl.Collator(context.locale);
  const byTitle = titleComparator(collator);
  const comparators = context.sort
    .map((option) => optionComparator(option, context, collator))
    .filter((comparator) => comparator !== undefined);
  return (first, second) => {
    for (const compare of comparators) {
      const result = compare(first, second);
      if (result !== 0) return result;
    }
    return byTitle(first, second);
  };
}

/** Views by title, ignoring an emoji at the start of the title. */
export function sortBoardViewsAlphabetically(views: readonly BoardView[]): BoardView[] {
  return views
    .map((view) => ({ view, title: view.title.replace(/^\p{Emoji}*\s*/u, '') }))
    .sort((first, second) => first.title.localeCompare(second.title))
    .map(({ view }) => view);
}
