/**
 * The cards as a graph by one relation property: edges, identifiers of
 * deleted cards, loops (closing a new one, finding those that are there) and
 * dates out of order.
 */
import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import { addDays, type CalendarDate } from '$lib/logic/dates.ts';
import {
  cardsClosingLoop,
  closesLoop,
  cutIds,
  daysOutOfOrder,
  edgesOnLoops,
  edgesOutOfOrder,
  edgesOutOfOrderByDays,
  isOutOfOrder,
  relationEdges,
  valuesOfCopy,
  type Days,
  type RelationEdge,
} from '$lib/logic/dependencies.ts';
import {
  createCard,
  RELATION_KINDS,
  RELATION_LAG_MAX,
  RELATION_LAG_MIN,
  RELATION_LIMIT,
  relationLink,
  setCardProperty,
  type Card,
  type PropertyTemplate,
  type PropertyValue,
  type RelationKind,
  type RelationLink,
} from '$lib/logic/wire.ts';

const AFTER = 'after';
const DUE: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };

type LinkInput = string | RelationLink;

function card(id: string, after?: LinkInput[], due?: PropertyValue): Card {
  const made = createCard();
  made.id = id;
  if (after) {
    setCardProperty(
      made,
      AFTER,
      after.map((item) => (typeof item === 'string' ? relationLink(item) : item)),
    );
  }
  if (due !== undefined) setCardProperty(made, 'due', due);
  return made;
}

const edge = (from: string, to: string, kind: RelationKind = 'fs', lagDays = 0): RelationEdge => ({
  from,
  to,
  kind,
  lagDays,
});

/** A chain a → b → c …: each card names the one before it. */
function chain(length: number): RelationEdge[] {
  return Array.from({ length: length - 1 }, (_, at) =>
    edge(`n${String(at)}`, `n${String(at + 1)}`),
  );
}

describe('edges', () => {
  it('go from the card named to the card that names it', () => {
    const cards = [card('a'), card('b', ['a']), card('c', ['a', 'b'])];
    expect(relationEdges(cards, AFTER)).toEqual([edge('a', 'b'), edge('a', 'c'), edge('b', 'c')]);
  });

  it('leave out deleted cards, the card itself and a card named twice', () => {
    const cards = [card('a'), card('b', ['gone', 'b', 'a', 'a'])];
    expect(relationEdges(cards, AFTER)).toEqual([edge('a', 'b')]);
  });

  it('come from a relation value only: a text, legacy string array, or another property gives none', () => {
    const odd = card('b');
    setCardProperty(odd, AFTER, 'a');
    setCardProperty(odd, 'other', [relationLink('a')]);
    const legacy = card('c');
    setCardProperty(legacy, AFTER, ['a']);
    expect(relationEdges([card('a'), odd, legacy], AFTER)).toEqual([]);
  });

  it('carry each link kind and lagDays into the graph edges', () => {
    const cards = [
      card('a'),
      card('b', [relationLink('a', 'ss', -2)]),
      card('c', [relationLink('a', 'ff', 0), relationLink('b', 'sf', 5)]),
    ];
    expect(relationEdges(cards, AFTER)).toEqual([
      edge('a', 'b', 'ss', -2),
      edge('a', 'c', 'ff', 0),
      edge('b', 'c', 'sf', 5),
    ]);
  });

  it('cut identifiers are the ones that name no card that is there', () => {
    const ids = new Set(['a', 'b']);
    expect(cutIds(card('b', ['gone', 'a', 'lost']), AFTER, ids)).toEqual(['gone', 'lost']);
    expect(cutIds(card('a'), AFTER, ids)).toEqual([]);
  });
});

describe('closing a loop', () => {
  it('is refused for the edge that turns two cards around, regardless of kind or lag', () => {
    const edges = [edge('a', 'b', 'ss', -3)];
    expect(closesLoop(edges, 'b', 'a')).toBe(true);
    expect(closesLoop(edges, 'a', 'b')).toBe(false);
    const mixedRing = [
      edge('a', 'b', 'fs', 0),
      edge('b', 'c', 'ss', 2),
      edge('c', 'd', 'ff', -1),
      edge('d', 'a', 'sf', 4),
    ];
    expect(edgesOnLoops(mixedRing)).toEqual(mixedRing);
  });

  it('is refused for a card naming itself', () => {
    expect(closesLoop([], 'a', 'a')).toBe(true);
  });

  it('is found at the end of a long chain', () => {
    const edges = chain(20_000);
    expect(closesLoop(edges, 'n19999', 'n0')).toBe(true);
    expect(closesLoop(edges, 'n0', 'n19999')).toBe(false);
  });

  it('is found through one of several branches, and a second way forward is no loop', () => {
    // a → b → d, a → c → d, d → e
    const edges = [edge('a', 'b'), edge('a', 'c'), edge('b', 'd'), edge('c', 'd'), edge('d', 'e')];
    expect(closesLoop(edges, 'e', 'c')).toBe(true);
    expect(closesLoop(edges, 'e', 'a')).toBe(true);
    expect(closesLoop(edges, 'b', 'c')).toBe(false);
    expect(closesLoop(edges, 'a', 'e')).toBe(false);
    expect([...cardsClosingLoop(edges, 'b')].sort()).toEqual(['b', 'd', 'e']);
  });

  it('agrees with walking every path, on generated graphs', () => {
    const graph = fc.array(fc.tuple(fc.nat(7), fc.nat(7)), { maxLength: 20 });
    fc.assert(
      fc.property(graph, fc.nat(7), fc.nat(7), (pairs, from, to) => {
        const edges = pairs.map(([a, b]) => edge(String(a), String(b)));
        expect(closesLoop(edges, String(from), String(to))).toBe(
          from === to || reaches(edges, String(to), String(from)),
        );
      }),
    );
  });
});

/** Whether `to` can be reached from `from` by one edge or more, the slow way. */
function reaches(edges: readonly RelationEdge[], from: string, to: string): boolean {
  const seen = new Set<string>();
  const visit = (id: string): boolean =>
    edges.some((each) => {
      if (each.from !== id || seen.has(each.to)) return false;
      seen.add(each.to);
      return each.to === to || visit(each.to);
    });
  return visit(from);
}

describe('loops that are there', () => {
  it('are the edges whose two cards reach each other', () => {
    // a ⇄ b, then b → c → d → b, and e hangs off c.
    const edges = [
      edge('a', 'b'),
      edge('b', 'a'),
      edge('b', 'c'),
      edge('c', 'd'),
      edge('d', 'b'),
      edge('c', 'e'),
      edge('x', 'y'),
    ];
    expect(edgesOnLoops(edges)).toEqual(edges.slice(0, 5));
  });

  it('are found around a long ring without running out of stack', () => {
    const ring = [...chain(20_000), edge('n19999', 'n0')];
    expect(edgesOnLoops(ring)).toHaveLength(20_000);
    expect(edgesOnLoops(chain(20_000))).toEqual([]);
  });

  it('agree with walking every path, on generated graphs', () => {
    const graph = fc.array(fc.tuple(fc.nat(6), fc.nat(6)), { maxLength: 18 });
    fc.assert(
      fc.property(graph, (pairs) => {
        const edges = pairs.map(([a, b]) => edge(String(a), String(b)));
        const slow = edges.filter(({ from, to }) => from === to || reaches(edges, to, from));
        expect(edgesOnLoops(edges)).toEqual(slow);
      }),
    );
  });
});

describe('dates out of order', () => {
  const day = (from: string, to?: string): PropertyValue => (to ? { from, to } : { from });

  it('when the later card starts before the earlier one ends', () => {
    const before = card('a', undefined, day('2026-03-02', '2026-03-10'));
    expect(isOutOfOrder(before, card('b', ['a'], day('2026-03-09')), DUE, 'UTC')).toBe(true);
    expect(isOutOfOrder(before, card('b', ['a'], day('2026-03-11')), DUE, 'UTC')).toBe(false);
  });

  it('not when one ends and the other starts on the same day', () => {
    const before = card('a', undefined, day('2026-03-02', '2026-03-10'));
    expect(
      isOutOfOrder(before, card('b', ['a'], day('2026-03-10', '2026-03-12')), DUE, 'UTC'),
    ).toBe(false);
  });

  it('not when either card has no date', () => {
    expect(isOutOfOrder(card('a'), card('b', [], day('2026-03-01')), DUE, 'UTC')).toBe(false);
    expect(isOutOfOrder(card('a', [], day('2026-03-09')), card('b'), DUE, 'UTC')).toBe(false);
  });

  it('a value with a time counts by the day it falls on for the reader', () => {
    // 16:00 UTC on 10 March is 01:00 on 11 March in Seoul.
    const before = card('a', undefined, { from: Date.UTC(2026, 2, 10, 16) });
    const after = card('b', ['a'], day('2026-03-10'));
    expect(isOutOfOrder(before, after, DUE, 'UTC')).toBe(false);
    expect(isOutOfOrder(before, after, DUE, 'Asia/Seoul')).toBe(true);
  });

  it('a time on the later card, and a range of times that ends on the next day for the reader', () => {
    const before = card('a', undefined, day('2026-03-10'));
    // 15:30 UTC on 9 March is 00:30 on 10 March in Seoul.
    const after = card('b', ['a'], { from: Date.UTC(2026, 2, 9, 15, 30) });
    expect(isOutOfOrder(before, after, DUE, 'UTC')).toBe(true);
    expect(isOutOfOrder(before, after, DUE, 'Asia/Seoul')).toBe(false);
    // 10:00 to 16:00 UTC on 10 March ends at 01:00 on the 11th in Seoul.
    const evening = card('c', undefined, {
      from: Date.UTC(2026, 2, 10, 10),
      to: Date.UTC(2026, 2, 10, 16),
    });
    const next = card('d', ['c'], day('2026-03-10'));
    expect(isOutOfOrder(evening, next, DUE, 'UTC')).toBe(false);
    expect(isOutOfOrder(evening, next, DUE, 'Asia/Seoul')).toBe(true);
  });

  it('by the moment a card was made, for the kind that has no stored value', () => {
    const made: PropertyTemplate = { id: 'made', name: 'Made', type: 'createdTime', options: [] };
    const before = card('a');
    before.createdAt = Date.UTC(2026, 2, 10, 16);
    const after = card('b', ['a']);
    after.createdAt = Date.UTC(2026, 2, 10, 12);
    // Both on the 10th in UTC; in Seoul the first is on the 11th, the second on the 10th.
    expect(isOutOfOrder(before, after, made, 'UTC')).toBe(false);
    expect(isOutOfOrder(before, after, made, 'Asia/Seoul')).toBe(true);
  });

  it('are the edges of a relation whose cards are out of order', () => {
    const cards = [
      card('a', undefined, day('2026-03-05')),
      card('b', ['a'], day('2026-03-04')),
      card('c', ['a', 'b'], day('2026-03-05')),
    ];
    const byId = new Map(cards.map((each) => [each.id, each]));
    expect(edgesOutOfOrder(byId, relationEdges(cards, AFTER), DUE, 'UTC')).toEqual([
      edge('a', 'b'),
    ]);
  });

  it('are the same edges by days worked out already, a card without days never among them', () => {
    const span = (first: string, last: string): Days => ({ first, last }) as Days;
    const days: Record<string, Days> = {
      a: span('2026-03-01', '2026-03-05'),
      b: span('2026-03-04', '2026-03-09'),
      c: span('2026-03-05', '2026-03-05'),
    };
    // The same day is in order; so is anything with a card that has no days.
    expect(daysOutOfOrder(days.a, days.c)).toBe(false);
    expect(daysOutOfOrder(days.a, undefined)).toBe(false);
    const edges = [edge('a', 'b'), edge('a', 'c'), edge('b', 'c'), edge('a', 'none')];
    expect(edgesOutOfOrderByDays(edges, (id) => days[id])).toEqual([
      edge('a', 'b'),
      edge('b', 'c'),
    ]);
  });

  describe('4 relation kinds × lagDays × card shapes matrix', () => {
    const span = (first: string, last = first): Days => ({
      first: first as CalendarDate,
      last: last as CalendarDate,
    });
    const LAGS = [-3, 0, 3, RELATION_LAG_MIN, RELATION_LAG_MAX] as const;

    it('never marks undated cards out of order for any kind or lag', () => {
      const dated = span('2026-03-10', '2026-03-15');
      for (const kind of RELATION_KINDS) {
        for (const lag of LAGS) {
          expect(daysOutOfOrder(undefined, dated, kind, lag)).toBe(false);
          expect(daysOutOfOrder(dated, undefined, kind, lag)).toBe(false);
          expect(daysOutOfOrder(undefined, undefined, kind, lag)).toBe(false);
        }
      }
    });

    it('checks exact boundary (late / same-day in-order / after) for single-day cards across all 4 kinds × lags', () => {
      const before = span('2026-06-15');
      for (const kind of RELATION_KINDS) {
        for (const lag of LAGS) {
          const boundary = addDays(before.first, lag);
          const lateDay = addDays(boundary, -1);
          const afterDay = addDays(boundary, 1);
          expect(daysOutOfOrder(before, span(lateDay), kind, lag), `${kind} lag=${lag} late`).toBe(
            true,
          );
          expect(
            daysOutOfOrder(before, span(boundary), kind, lag),
            `${kind} lag=${lag} same-day boundary`,
          ).toBe(false);
          expect(
            daysOutOfOrder(before, span(afterDay), kind, lag),
            `${kind} lag=${lag} after`,
          ).toBe(false);
        }
      }
    });

    it('compares the exact start/end anchors for multi-day cards across all 4 kinds × negative/0/positive lags', () => {
      // Predecessor A: Mar 10 .. Mar 15 (startA = Mar 10, endA = Mar 15)
      // Successor B has duration 5 days (endB = startB + 4)
      const before = span('2026-03-10', '2026-03-15');
      const DURATION = 4;
      for (const kind of RELATION_KINDS) {
        const anchorA = kind === 'fs' || kind === 'ff' ? before.last : before.first;
        const targetIsStart = kind === 'fs' || kind === 'ss';
        for (const lag of LAGS) {
          const boundary = addDays(anchorA, lag);
          const makeAfter = (targetDay: CalendarDate): Days =>
            targetIsStart
              ? { first: targetDay, last: addDays(targetDay, DURATION) }
              : { first: addDays(targetDay, -DURATION), last: targetDay };
          expect(
            daysOutOfOrder(before, makeAfter(addDays(boundary, -1)), kind, lag),
            `${kind} lag=${lag} multi-day late`,
          ).toBe(true);
          expect(
            daysOutOfOrder(before, makeAfter(boundary), kind, lag),
            `${kind} lag=${lag} multi-day same-day boundary`,
          ).toBe(false);
          expect(
            daysOutOfOrder(before, makeAfter(addDays(boundary, 1)), kind, lag),
            `${kind} lag=${lag} multi-day after`,
          ).toBe(false);
        }
      }
    });

    it('distinguishes FS, SS, FF, and SF on the same pair of multi-day cards', () => {
      // A: Mar 10..Mar 20, B: Mar 12..Mar 18
      // FS: startB (12) < endA (20) -> late
      // SS: startB (12) >= startA (10) -> in order (with lag +3: 12 < 13 -> late)
      // FF: endB (18) < endA (20) -> late (with lag -2: 18 >= 18 -> in order)
      // SF: endB (18) >= startA (10) -> in order (with lag +9: 18 < 19 -> late)
      const a = card('a', undefined, day('2026-03-10', '2026-03-20'));
      const b = card('b', undefined, day('2026-03-12', '2026-03-18'));
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'fs', 0)).toBe(true);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'fs', -8)).toBe(false);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'fs', -7)).toBe(true);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'ss', 0)).toBe(false);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'ss', 2)).toBe(false);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'ss', 3)).toBe(true);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'ff', 0)).toBe(true);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'ff', -2)).toBe(false);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'ff', -1)).toBe(true);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'sf', 0)).toBe(false);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'sf', 8)).toBe(false);
      expect(isOutOfOrder(a, b, DUE, 'UTC', 'sf', 9)).toBe(true);

      const graphCards = [
        a,
        card('b1', [relationLink('a', 'ss', 0)], day('2026-03-12', '2026-03-18')),
        card('b2', [relationLink('a', 'ss', 3)], day('2026-03-12', '2026-03-18')),
        card('b3', [relationLink('a', 'ff', -2)], day('2026-03-12', '2026-03-18')),
        card('b4', [relationLink('a', 'sf', 9)], day('2026-03-12', '2026-03-18')),
      ];
      const byId = new Map(graphCards.map((each) => [each.id, each]));
      expect(edgesOutOfOrder(byId, relationEdges(graphCards, AFTER), DUE, 'UTC')).toEqual([
        edge('a', 'b2', 'ss', 3),
        edge('a', 'b4', 'sf', 9),
      ]);
    });
  });
});

describe('the values of a copy of a card', () => {
  const RELATION: PropertyTemplate = { id: AFTER, name: 'After', type: 'relation', options: [] };

  it('name the cards that are there, each once, preserving kind and lagDays, not the card copied, at most the limit', () => {
    const source = card('source', [
      relationLink('a', 'ss', -2),
      relationLink('gone', 'ff', 3),
      relationLink('source', 'fs', 0),
      relationLink('a', 'sf', 9),
      relationLink('b', 'ff', 4),
    ]);
    setCardProperty(source, 'text', ['x', 'x']);
    const there = new Set(['source', 'a', 'b']);
    expect(valuesOfCopy(source, [RELATION, DUE], there)).toEqual({
      [AFTER]: [relationLink('a', 'ss', -2), relationLink('b', 'ff', 4)],
      text: ['x', 'x'],
    });
    const many = Array.from({ length: RELATION_LIMIT + 5 }, (_, at) =>
      relationLink(`c${String(at)}`, 'ss', at % 3),
    );
    const crowded = valuesOfCopy(
      card('source', many),
      [RELATION],
      new Set(many.map((link) => link.card)),
    );
    expect(crowded[AFTER]).toEqual(many.slice(0, RELATION_LIMIT));
  });

  it('leave a card without a relation value as it is, and drop stale relation link lists on non-relation properties', () => {
    expect(valuesOfCopy(card('source'), [RELATION], new Set())).toEqual({});
    const stale = card('source');
    setCardProperty(stale, 'note', [relationLink('a', 'ss', 2)]);
    expect(
      valuesOfCopy(
        stale,
        [{ id: 'note', name: 'Note', type: 'text', options: [] }],
        new Set(['a']),
      ),
    ).toEqual({});
  });
});
