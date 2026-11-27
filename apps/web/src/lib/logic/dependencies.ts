/**
 * The cards of a board as a graph, by one card relation property.
 *
 * A relation value is written on the card that has it and names other cards
 * with a dependency kind (`fs`, `ss`, `ff`, `sf`) and lag in whole calendar
 * days (`lagDays`). Read as "comes after" (a Gantt chart's predecessors), a
 * card B whose value names A gives the edge A → B: A comes first, B after it.
 *
 * - Identifiers of cards that are not there (deleted) are cut: they give no
 *   edge. They stay in the stored value; see `cutIds`.
 * - A card naming itself, or naming a card twice, gives no edge (twice: one).
 * - Loops are not refused by the server and may already be there. The editor
 *   refuses to close a new one where it is told to (`cardsClosingLoop`);
 *   one that is there is only shown (`edgesOnLoops`). Loop detection ignores
 *   relation kind and lag.
 * - Dates out of order (same day is in order; undated cards are never late):
 *   - `fs`: `startB < endA + lagDays`
 *   - `ss`: `startB < startA + lagDays`
 *   - `ff`: `endB < endA + lagDays`
 *   - `sf`: `endB < startA + lagDays`
 *
 * Everything here is pure: cards in, answers out.
 */
import { daysBetween, type CalendarDate } from './dates.ts';
import { propertyBehavior } from './properties.ts';
import {
  cardProperties,
  cardProperty,
  isRelationLinksValue,
  parseRelationValue,
  RELATION_LIMIT,
  relationCardIds,
  type Card,
  type PropertyTemplate,
  type PropertyValue,
  type RelationKind,
} from './wire.ts';

/** `from` is named in the relation value of `to`, with its dependency kind and lag days. */
export interface RelationEdge {
  from: string;
  to: string;
  kind: RelationKind;
  lagDays: number;
}

/**
 * The property values a copy of `card` on its own board is made with: a
 * relation names the cards of `cardIds` it named, each once, preserving `kind`
 * and `lagDays`, not `card` itself, at most RELATION_LIMIT. `cardIds` are the
 * cards a copy may name, those in the board's trash too: the caller knows
 * which those are. Stale relation link arrays on properties that are no longer
 * relations are omitted so the copy is accepted by the server; other values
 * are kept as they are.
 */
export function valuesOfCopy(
  card: Card,
  templates: readonly PropertyTemplate[],
  cardIds: ReadonlySet<string>,
): Record<string, PropertyValue> {
  const relationIds = new Set(
    templates.filter((template) => template.type === 'relation').map((template) => template.id),
  );
  const values: Record<string, PropertyValue> = {};
  for (const [id, value] of Object.entries(cardProperties(card))) {
    if (relationIds.has(id)) {
      values[id] = parseRelationValue(value)
        .filter((link) => link.card !== card.id && cardIds.has(link.card))
        .slice(0, RELATION_LIMIT);
    } else if (!isRelationLinksValue(value)) {
      values[id] = value;
    }
  }
  return values;
}

/** The identifiers in a card's value that name no card in `cardIds` (deleted ones). */
export function cutIds(card: Card, propertyId: string, cardIds: ReadonlySet<string>): string[] {
  return relationCardIds(cardProperty(card, propertyId)).filter((id) => !cardIds.has(id));
}

/**
 * The edges of one relation property among `cards`, in the order of the
 * cards and of their values. Identifiers of cards not among `cards`, and a
 * card naming itself, give none.
 */
export function relationEdges(cards: Iterable<Card>, propertyId: string): RelationEdge[] {
  const list = [...cards];
  const ids = new Set(list.map((card) => card.id));
  const edges: RelationEdge[] = [];
  for (const card of list) {
    for (const link of parseRelationValue(cardProperty(card, propertyId))) {
      if (link.card !== card.id && ids.has(link.card)) {
        edges.push({ from: link.card, to: card.id, kind: link.kind, lagDays: link.lagDays });
      }
    }
  }
  return edges;
}

type EdgeEndpoints = Pick<RelationEdge, 'from' | 'to'>;

function successorsOf(edges: readonly EdgeEndpoints[]): Map<string, string[]> {
  const next = new Map<string, string[]>();
  for (const { from, to } of edges) {
    const list = next.get(from);
    if (list) list.push(to);
    else next.set(from, [to]);
  }
  return next;
}

/**
 * The cards that `cardId` may not name without closing a loop: itself and
 * every card that comes after it, however far. Naming one of them adds an
 * edge from it to `cardId`, and there is a way back already.
 */
export function cardsClosingLoop(edges: readonly EdgeEndpoints[], cardId: string): Set<string> {
  const next = successorsOf(edges);
  const reached = new Set([cardId]);
  const waiting = [cardId];
  for (let id = waiting.pop(); id !== undefined; id = waiting.pop()) {
    for (const after of next.get(id) ?? []) {
      if (reached.has(after)) continue;
      reached.add(after);
      waiting.push(after);
    }
  }
  return reached;
}

/** Whether adding the edge `from` → `to` closes a loop. */
export function closesLoop(edges: readonly EdgeEndpoints[], from: string, to: string): boolean {
  return cardsClosingLoop(edges, to).has(from);
}

/**
 * The groups of cards that reach each other (strongly connected
 * components, Tarjan's algorithm without recursion, so that a long chain
 * does not run out of stack). Every card of `edges` is in one group.
 */
function reachingGroups(edges: readonly EdgeEndpoints[]): Map<string, number> {
  const next = successorsOf(edges);
  const nodes = [...new Set(edges.flatMap(({ from, to }) => [from, to]))];
  const index = new Map<string, number>();
  const low = new Map<string, number>();
  const group = new Map<string, number>();
  const stack: string[] = [];
  let counter = 0;
  let groups = 0;
  for (const root of nodes) {
    if (index.has(root)) continue;
    const path: { id: string; at: number }[] = [{ id: root, at: 0 }];
    index.set(root, counter);
    low.set(root, counter);
    counter += 1;
    stack.push(root);
    while (path.length > 0) {
      const top = path[path.length - 1] as { id: string; at: number };
      const after = next.get(top.id)?.[top.at];
      if (after !== undefined) {
        top.at += 1;
        if (!index.has(after)) {
          index.set(after, counter);
          low.set(after, counter);
          counter += 1;
          stack.push(after);
          path.push({ id: after, at: 0 });
        } else if (!group.has(after)) {
          low.set(top.id, Math.min(low.get(top.id) ?? 0, index.get(after) ?? 0));
        }
        continue;
      }
      path.pop();
      const parent = path[path.length - 1];
      if (parent) low.set(parent.id, Math.min(low.get(parent.id) ?? 0, low.get(top.id) ?? 0));
      if (low.get(top.id) !== index.get(top.id)) continue;
      for (let id = stack.pop(); id !== undefined; id = stack.pop()) {
        group.set(id, groups);
        if (id === top.id) break;
      }
      groups += 1;
    }
  }
  return group;
}

/** The edges that lie on a loop: both ends reach each other. */
export function edgesOnLoops<E extends EdgeEndpoints>(edges: readonly E[]): E[] {
  const group = reachingGroups(edges);
  return edges.filter(({ from, to }) => group.get(from) === group.get(to));
}

/** The first and last day a card takes by a date property, for a reader in `timeZone`. */
function daysOfCard(card: Card, template: PropertyTemplate, timeZone: string): Days | undefined {
  return propertyBehavior(template.type).days?.(cardProperty(card, template.id), card, timeZone);
}

/** The first and last day of something on the calendar. */
export interface Days {
  first: CalendarDate;
  last: CalendarDate;
}

/**
 * Whether `after` is out of order with `before` under dependency `kind` and
 * `lagDays` (whole calendar days). Same day (`daysBetween === lagDays`) is in
 * order; an undated card is never out of order.
 */
export function daysOutOfOrder(
  before: Days | undefined,
  after: Days | undefined,
  kind: RelationKind = 'fs',
  lagDays = 0,
): boolean {
  if (before === undefined || after === undefined) return false;
  const anchor = kind === 'fs' || kind === 'ff' ? before.last : before.first;
  const target = kind === 'fs' || kind === 'ss' ? after.first : after.last;
  return daysBetween(anchor, target) < lagDays;
}

/**
 * Whether the dates of `after` are out of order with those of `before`, by a
 * date property, relation kind, and lag in whole calendar days. A value with a
 * time counts by the day it falls on in `timeZone`. Equal boundary days are in
 * order; a card without a date is never out of order.
 */
export function isOutOfOrder(
  before: Card,
  after: Card,
  template: PropertyTemplate,
  timeZone: string,
  kind: RelationKind = 'fs',
  lagDays = 0,
): boolean {
  return daysOutOfOrder(
    daysOfCard(before, template, timeZone),
    daysOfCard(after, template, timeZone),
    kind,
    lagDays,
  );
}

/**
 * The edges whose two cards are out of order, by days worked out already
 * (a chart has them for its bars): `daysOf` answers undefined for a card
 * without a date.
 */
export function edgesOutOfOrderByDays(
  edges: readonly RelationEdge[],
  daysOf: (cardId: string) => Days | undefined,
): RelationEdge[] {
  return edges.filter(({ from, to, kind, lagDays }) =>
    daysOutOfOrder(daysOf(from), daysOf(to), kind, lagDays),
  );
}

/** The edges whose two cards are out of order by a date property. */
export function edgesOutOfOrder(
  cards: ReadonlyMap<string, Card>,
  edges: readonly RelationEdge[],
  template: PropertyTemplate,
  timeZone: string,
): RelationEdge[] {
  return edges.filter(({ from, to, kind, lagDays }) => {
    const before = cards.get(from);
    const after = cards.get(to);
    return before !== undefined && after !== undefined
      ? isOutOfOrder(before, after, template, timeZone, kind, lagDays)
      : false;
  });
}
