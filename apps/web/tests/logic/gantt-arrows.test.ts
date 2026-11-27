/**
 * The relations a Gantt view draws (lib/logic/gantt.ts): which edges, which
 * of them are out of order or on a loop, and the way an arrow takes from
 * the end of one bar to the start of another.
 */
import fc from 'fast-check';
import { describe, expect, it } from 'vitest';

import type { CalendarDate } from '$lib/logic/dates.ts';
import type { Days } from '$lib/logic/dependencies.ts';
import {
  ARROW_GAP,
  arrowHead,
  arrowHitPoints,
  arrowLabelPoint,
  arrowPath,
  arrowPoints,
  arrowsOf,
  axisOf,
  barBox,
  barsOf,
  cardsBefore,
  dependencyProperty,
  edgeKey,
  MIN_BAR_WIDTH,
  relationsOf,
  rowsOf,
  TURN_INSET,
  type Bar,
} from '$lib/logic/gantt.ts';
import {
  clampRelationLagDays,
  parseLagDaysInput,
  RELATION_KINDS,
  RELATION_LAG_MAX,
  RELATION_LAG_MIN,
  removeRelationLink,
  updateRelationLink,
} from '$lib/logic/relations.ts';
import {
  createCard,
  relationLink,
  setCardProperty,
  type Card,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';

const day = (text: string): CalendarDate => text as CalendarDate;
const days = (first: string, last = first): Days => ({ first: day(first), last: day(last) });
const TODAY = day('2026-11-10');
const SUNDAY = 7;

const AFTER = 'after';

function card(id: string, after?: string[]): Card {
  const made = createCard();
  made.id = id;
  if (after)
    setCardProperty(
      made,
      AFTER,
      after.map((target) => relationLink(target)),
    );
  return made;
}

describe('the property read as what comes before what', () => {
  const templates: PropertyTemplate[] = [
    { id: 'after', name: 'After', type: 'relation', options: [] },
    { id: 'note', name: 'Note', type: 'text', options: [] },
  ];

  it('is the card relation the view names, and none for anything else', () => {
    expect(dependencyProperty('after', templates)?.name).toBe('After');
    expect(dependencyProperty(undefined, templates)).toBeUndefined();
    expect(dependencyProperty('', templates)).toBeUndefined();
    // Deleted since, or made another kind.
    expect(dependencyProperty('gone', templates)).toBeUndefined();
    expect(dependencyProperty('note', templates)).toBeUndefined();
  });
});

describe('the relations a chart draws', () => {
  const spans: Record<string, Days> = {
    a: days('2026-11-02', '2026-11-05'),
    b: days('2026-11-05', '2026-11-06'),
    c: days('2026-11-04'),
  };
  const daysOf = (id: string): Days | undefined => spans[id];

  it('are none without a property', () => {
    const cards = [card('a'), card('b', ['a'])];
    expect(relationsOf(cards, cards, undefined, daysOf).edges).toEqual([]);
    expect(relationsOf(cards, cards, '', daysOf).edges).toEqual([]);
  });

  it('go from the card named to the card that names it, among the cards drawn', () => {
    const all = [card('a'), card('b', ['a', 'gone']), card('c', ['a', 'hidden']), card('hidden')];
    const drawn = all.slice(0, 3);
    const relations = relationsOf(drawn, all, AFTER, daysOf);
    expect(relations.edges).toEqual([
      { from: 'a', to: 'b', kind: 'fs', lagDays: 0 },
      { from: 'a', to: 'c', kind: 'fs', lagDays: 0 },
    ]);
  });

  it('are out of order where the later card starts before the day the earlier ends', () => {
    const cards = [card('a'), card('b', ['a']), card('c', ['a'])];
    const relations = relationsOf(cards, cards, AFTER, daysOf);
    // b starts on the day a ends: in order. c starts the day before.
    expect([...relations.late]).toEqual([edgeKey({ from: 'a', to: 'c' })]);
    expect(relations.loops.size).toBe(0);
  });

  it('are only between cards that have days: a card without a date has no bar to reach', () => {
    const cards = [card('a'), card('undated', ['a']), card('b', ['undated', 'a'])];
    const relations = relationsOf(cards, cards, AFTER, daysOf);
    expect(relations.edges).toEqual([{ from: 'a', to: 'b', kind: 'fs', lagDays: 0 }]);
  });

  it('say for each card which cards it comes after, in the order of its value', () => {
    const cards = [card('a'), card('c'), card('b', ['c', 'a']), card('d', ['b'])];
    const all: Record<string, Days> = { ...spans, d: days('2026-11-20') };
    const { edges } = relationsOf(cards, cards, AFTER, (id) => all[id]);
    expect([...cardsBefore(edges)]).toEqual([
      ['b', ['c', 'a']],
      ['d', ['b']],
    ]);
  });

  it('are never out of order with a card that has no date', () => {
    const cards = [card('a'), card('undated', ['a']), card('b', ['undated'])];
    const relations = relationsOf(cards, cards, AFTER, daysOf);
    expect(relations.late.size).toBe(0);
  });

  it('are marked on a loop, one that goes through a card that is not drawn too', () => {
    // a → b → hidden → a, and b → c beside the loop.
    const all = [card('a', ['hidden']), card('b', ['a']), card('hidden', ['b']), card('c', ['b'])];
    const drawn = all.filter((each) => each.id !== 'hidden');
    const relations = relationsOf(drawn, all, AFTER, daysOf);
    expect(relations.edges).toHaveLength(2);
    expect([...relations.loops]).toEqual([edgeKey({ from: 'a', to: 'b' })]);
    // Every card of the loop is known, the one that is not drawn too; the one beside it is not.
    expect([...relations.loopCards].sort()).toEqual(['a', 'b', 'hidden']);
  });

  it('know a card on a loop none of whose edges is drawn', () => {
    // a → undated → a: neither edge has two bars.
    const cards = [card('a', ['undated']), card('undated', ['a'])];
    const relations = relationsOf(cards, cards, AFTER, daysOf);
    expect(relations.edges).toEqual([]);
    expect(relations.loops.size).toBe(0);
    expect([...relations.loopCards].sort()).toEqual(['a', 'undated']);
  });
});

describe('an arrow', () => {
  const bar = (id: string, row: number, x: number, width: number): Bar => ({
    id,
    row,
    x,
    width,
    fromBefore: false,
    toAfter: false,
  });

  it('goes out of the end of the first bar, down, and into the start of the second', () => {
    expect(arrowPoints(bar('a', 0, 2, 4), bar('b', 2, 10, 3))).toEqual([
      [6, 0.5],
      [6 + ARROW_GAP, 0.5],
      [6 + ARROW_GAP, 2.5],
      [10, 2.5],
    ]);
  });

  it('goes up to a card in a row above', () => {
    const points = arrowPoints(bar('a', 3, 2, 4), bar('b', 1, 10, 3));
    expect(points.at(0)).toEqual([6, 3.5]);
    expect(points.at(-1)).toEqual([10, 1.5]);
    expect(points).toHaveLength(4);
  });

  it('turns back between the rows where the second bar starts no later than the first ends', () => {
    // The second starts on the day the first ends: no room to go straight in.
    expect(arrowPoints(bar('a', 0, 2, 4), bar('b', 1, 5, 3))).toEqual([
      [6, 0.5],
      [6 + ARROW_GAP, 0.5],
      [6 + ARROW_GAP, 1 - TURN_INSET],
      [5 - ARROW_GAP, 1 - TURN_INSET],
      [5 - ARROW_GAP, 1.5],
      [5, 1.5],
    ]);
    // Upwards the turn is at the top of the first bar's row.
    expect(arrowPoints(bar('a', 4, 2, 4), bar('b', 1, 1, 3))[2]).toEqual([
      6 + ARROW_GAP,
      4 + TURN_INSET,
    ]);
    // A bar at the very start of the axis: the arrow stays on the chart.
    expect(arrowPoints(bar('a', 0, 2, 4), bar('b', 1, 0, 3))[3]).toEqual([0, 1 - TURN_INSET]);
  });

  it('goes straight in from two gaps of room on, and turns back with any less', () => {
    const first = bar('a', 0, 2, 4); // ends at 6
    expect(arrowPoints(first, bar('b', 1, 6 + 2 * ARROW_GAP, 3))).toHaveLength(4);
    expect(arrowPoints(first, bar('b', 1, 6 + 2 * ARROW_GAP - 0.01, 3))).toHaveLength(6);
    expect(arrowPoints(first, bar('b', 1, 6 + ARROW_GAP, 3))).toHaveLength(6);
  });

  it('turns back along the edge of its own row that faces the second bar, however far that is', () => {
    expect(arrowPoints(bar('a', 1, 2, 4), bar('b', 5, 3, 3))[2]).toEqual([
      6 + ARROW_GAP,
      2 - TURN_INSET,
    ]);
    expect(arrowPoints(bar('a', 5, 2, 4), bar('b', 1, 3, 3))[2]).toEqual([
      6 + ARROW_GAP,
      5 + TURN_INSET,
    ]);
  });

  it('does not share its way back with an arrow out of the next row', () => {
    // One down out of row 3 and one up out of row 4: either side of the line between them.
    const down = arrowPoints(bar('a', 3, 20, 10), bar('b', 5, 18, 3));
    const up = arrowPoints(bar('c', 4, 20, 10), bar('d', 0, 18, 3));
    expect(down[2]?.[1]).toBe(4 - TURN_INSET);
    expect(up[2]?.[1]).toBe(4 + TURN_INSET);
  });

  it('stays on the axis at both edges', () => {
    // Cards so far apart that the axis is a window: one a stub at each edge.
    const made = axisOf({
      zoom: 'day',
      weekStart: SUNDAY,
      today: TODAY,
      spans: [days('0001-01-01'), days('9999-12-31')],
    });
    const early: Bar = { id: 'early', row: 0, ...barBox(made, days('0001-01-01')) };
    const now: Bar = { id: 'now', row: 1, ...barBox(made, days('2026-11-10', '2026-11-12')) };
    const late: Bar = { id: 'late', row: 2, ...barBox(made, days('9999-12-31')) };
    for (const [from, to] of [
      [late, now],
      [now, early],
      [late, early],
      [early, late],
    ] as const) {
      const points = arrowPoints(from, to, made.width);
      for (const [x] of points) {
        expect(x).toBeGreaterThanOrEqual(0);
        expect(x).toBeLessThanOrEqual(made.width);
      }
      expect(points.at(-1)).toEqual([to.x, to.row + 0.5]);
    }
    // Out of the stub at the end of the axis the arrow turns at the edge.
    expect(arrowPoints(late, now, made.width)[1]).toEqual([made.width, 2.5]);
  });

  it('always ends at the start of the second bar, pointing into it', () => {
    const place = fc.record({
      x: fc.double({ min: 0, max: 500, noNaN: true }),
      width: fc.double({ min: MIN_BAR_WIDTH, max: 100, noNaN: true }),
    });
    fc.assert(
      fc.property(place, place, fc.nat(50), fc.nat(50), (a, b, rowA, rowB) => {
        fc.pre(rowA !== rowB);
        const points = arrowPoints(bar('a', rowA, a.x, a.width), bar('b', rowB, b.x, b.width));
        expect(points[0]).toEqual([a.x + a.width, rowA + 0.5]);
        expect(points.at(-1)).toEqual([b.x, rowB + 0.5]);
        // The last piece runs to the right, along the middle of the second row.
        const before = points.at(-2);
        expect(before?.[1]).toBe(rowB + 0.5);
        expect(before?.[0]).toBeLessThanOrEqual(b.x);
        // Every piece is straight across or straight down.
        points.slice(1).forEach((point, at) => {
          const from = points[at];
          expect(point[0] === from?.[0] || point[1] === from?.[1]).toBe(true);
        });
      }),
    );
  });

  it('is drawn once for each edge whose two cards have a bar, with its marks', () => {
    const cards = [card('a'), card('b', ['a']), card('c', ['a', 'b']), card('undated', ['a'])];
    const spans: Record<string, Days> = {
      a: days('2026-11-04', '2026-11-06'),
      b: days('2026-11-05'),
      c: days('2026-11-09'),
    };
    const daysOf = (id: string): Days | undefined => spans[id];
    const made = axisOf({ zoom: 'day', weekStart: SUNDAY, today: TODAY, spans: [] });
    const rows = rowsOf([{ cardIds: cards.map((each) => each.id) }]);
    const relations = relationsOf(cards, cards, AFTER, daysOf);
    const arrows = arrowsOf(relations, barsOf(made, rows, daysOf));
    expect(arrows.map(({ from, to, late, loop }) => ({ from, to, late, loop }))).toEqual([
      { from: 'a', to: 'b', late: true, loop: false },
      { from: 'a', to: 'c', late: false, loop: false },
      { from: 'b', to: 'c', late: false, loop: false },
    ]);
    expect(arrows).toHaveLength(relations.edges.length);
    // An edge to a card whose bar is not among those given draws nothing.
    const fewer = barsOf(made, rowsOf([{ cardIds: ['a', 'b'] }]), daysOf);
    expect(arrowsOf(relations, fewer).map(({ from, to }) => [from, to])).toEqual([['a', 'b']]);
  });

  it('is written as the path of its line and of its head', () => {
    const points = arrowPoints(bar('a', 0, 2, 4), bar('b', 2, 10, 3));
    expect(arrowPath(points)).toBe('M6 0.5L6.5 0.5L6.5 2.5L10 2.5');
    expect(arrowHead(points)).toBe('M10 2.5L9.6 2.36L9.6 2.64Z');
    expect(arrowHead([])).toBe('');
    // To a thousandth of a rem and of a row, no further.
    expect(
      arrowPath([
        [1.23456, 0.5],
        [2, 0.5],
      ]),
    ).toBe('M1.235 0.5L2 0.5');
  });

  it('routes all four relation kinds with the right bar ends and arrowhead directions', () => {
    const a = bar('a', 0, 4, 4); // x: 4..8
    const b = bar('b', 2, 10, 3); // x: 10..13

    // FS: end of a (8) -> start of b (10), right-pointing head
    const fs = arrowPoints(a, b, 30, 'fs');
    expect(fs[0]).toEqual([8, 0.5]);
    expect(fs.at(-1)).toEqual([10, 2.5]);
    expect(arrowHead(fs, 'fs')).toBe('M10 2.5L9.6 2.36L9.6 2.64Z');

    // SS: start of a (4) -> start of b (10), C-route on left, right-pointing head
    const ss = arrowPoints(a, b, 30, 'ss');
    expect(ss).toEqual([
      [4, 0.5],
      [4 - ARROW_GAP, 0.5],
      [4 - ARROW_GAP, 2.5],
      [10, 2.5],
    ]);
    expect(arrowHead(ss, 'ss')).toBe('M10 2.5L9.6 2.36L9.6 2.64Z');

    // FF: end of a (8) -> end of b (13), backwards-C route on right, left-pointing head
    const ff = arrowPoints(a, b, 30, 'ff');
    expect(ff).toEqual([
      [8, 0.5],
      [13 + ARROW_GAP, 0.5],
      [13 + ARROW_GAP, 2.5],
      [13, 2.5],
    ]);
    expect(arrowHead(ff, 'ff')).toBe('M13 2.5L13.4 2.36L13.4 2.64Z');

    // SF direct: start of a (14) -> end of b (5), left-pointing head
    const sfDirect = arrowPoints(bar('a', 0, 14, 4), bar('b', 2, 2, 3), 30, 'sf');
    expect(sfDirect).toEqual([
      [14, 0.5],
      [14 - ARROW_GAP, 0.5],
      [14 - ARROW_GAP, 2.5],
      [5, 2.5],
    ]);
    expect(arrowHead(sfDirect, 'sf')).toBe('M5 2.5L5.4 2.36L5.4 2.64Z');

    // SF detour when start of a is left of end of b
    const sfDetour = arrowPoints(a, b, 30, 'sf');
    expect(sfDetour).toEqual([
      [4, 0.5],
      [4 - ARROW_GAP, 0.5],
      [4 - ARROW_GAP, 1 - TURN_INSET],
      [13 + ARROW_GAP, 1 - TURN_INSET],
      [13 + ARROW_GAP, 2.5],
      [13, 2.5],
    ]);
    expect(arrowHead(sfDetour, 'sf')).toBe('M13 2.5L13.4 2.36L13.4 2.64Z');
  });

  it('keeps all four relation kinds clamped within 0..=width with valid entry directions', () => {
    const width = 40;
    const place = fc.record({
      x: fc.double({ min: 0, max: width - MIN_BAR_WIDTH, noNaN: true }),
      width: fc.double({ min: MIN_BAR_WIDTH, max: 10, noNaN: true }),
    });
    fc.assert(
      fc.property(
        place,
        place,
        fc.nat(20),
        fc.nat(20),
        fc.constantFrom(...RELATION_KINDS),
        (rawA, rawB, rowA, rowB, kind) => {
          fc.pre(rowA !== rowB);
          const a = bar('a', rowA, rawA.x, Math.min(rawA.width, width - rawA.x));
          const b = bar('b', rowB, rawB.x, Math.min(rawB.width, width - rawB.x));
          const points = arrowPoints(a, b, width, kind);
          const startX = kind === 'fs' || kind === 'ff' ? a.x + a.width : a.x;
          const endX = kind === 'fs' || kind === 'ss' ? b.x : b.x + b.width;
          expect(points[0]).toEqual([startX, rowA + 0.5]);
          expect(points.at(-1)).toEqual([endX, rowB + 0.5]);
          const before = points.at(-2);
          expect(before?.[1]).toBe(rowB + 0.5);
          if (kind === 'fs' || kind === 'ss') expect(before?.[0]).toBeLessThanOrEqual(endX);
          else expect(before?.[0]).toBeGreaterThanOrEqual(endX);
          for (const [x] of points) {
            expect(x).toBeGreaterThanOrEqual(0);
            expect(x).toBeLessThanOrEqual(width);
          }
          points.slice(1).forEach((point, at) => {
            const from = points[at];
            expect(point[0] === from?.[0] || point[1] === from?.[1]).toBe(true);
          });
        },
      ),
    );
  });

  it('trims hit paths away from bar grips and computes clear label anchors', () => {
    const direct = arrowPoints(bar('a', 0, 2, 4), bar('b', 2, 10, 3), 30, 'fs');
    expect(arrowHitPoints(direct)).toEqual([
      [6.5, 0.85],
      [6.5, 2.5],
      [9.5, 2.5],
    ]);
    expect(arrowLabelPoint(direct)).toEqual([6.5, 1.5]);

    const detour = arrowPoints(bar('a', 0, 2, 4), bar('b', 1, 5, 3), 30, 'fs');
    expect(arrowHitPoints(detour)).toEqual([
      [6.5, 1 - TURN_INSET],
      [4.5, 1 - TURN_INSET],
    ]);
    expect(arrowLabelPoint(detour)).toEqual([5.5, 1 - TURN_INSET]);
  });
});

describe('relation link mutation helpers', () => {
  it('updates and removes one link while preserving untouched and hidden links', () => {
    const initial = [
      relationLink('gone', 'sf', -2),
      relationLink('a', 'fs', 0),
      relationLink('b', 'ff', 3),
    ];
    expect(updateRelationLink(initial, 'a', { kind: 'ss', lagDays: 5 })).toEqual([
      relationLink('gone', 'sf', -2),
      relationLink('a', 'ss', 5),
      relationLink('b', 'ff', 3),
    ]);
    expect(removeRelationLink(initial, 'a')).toEqual([
      relationLink('gone', 'sf', -2),
      relationLink('b', 'ff', 3),
    ]);
  });

  it('clamps and parses lag days within bounds', () => {
    expect(clampRelationLagDays(RELATION_LAG_MAX + 50)).toBe(RELATION_LAG_MAX);
    expect(clampRelationLagDays(RELATION_LAG_MIN - 50)).toBe(RELATION_LAG_MIN);
    expect(clampRelationLagDays(-0)).toBe(0);
    expect(parseLagDaysInput(' +12 ')).toBe(12);
    expect(parseLagDaysInput('-0')).toBe(0);
    expect(parseLagDaysInput('3.5')).toBeUndefined();
    expect(parseLagDaysInput('')).toBeUndefined();
    expect(parseLagDaysInput('3651')).toBeUndefined();
    expect(parseLagDaysInput('-3651')).toBeUndefined();
  });
});
