/**
 * Pure unit tests for `lib/logic/card-hierarchy.ts`:
 * depth, subtree height, ancestors/descendants, cycle/dangling safety,
 * eligible parent & sub-card pickers, status/checkbox progress, and
 * hierarchical row grouping with expand/collapse and filtered-out parents.
 */
import { describe, expect, it } from 'vitest';

import {
  ancestorsOf,
  buildHierarchyIndex,
  canAddSubcard,
  cardDepth,
  descendantsOf,
  directChildrenOf,
  eligibleParentsFor,
  eligibleSubcardsFor,
  hasVisibleHierarchy,
  hierarchicalRows,
  isCardCompleted,
  MAX_CARD_DEPTH,
  statusOptionOf,
  statusPropertyOf,
  subcardProgress,
  subtreeHeight,
} from '$lib/logic/card-hierarchy.ts';
import {
  createCard,
  setCardProperty,
  type Card,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';

function card(
  id: string,
  opts: {
    parentId?: string;
    title?: string;
    isTemplate?: boolean;
    properties?: Record<string, PropertyValue>;
  } = {},
): Card {
  const made = createCard({
    id,
    containerId: 'b',
    parentId: opts.parentId ?? '',
    type: 'card',
    title: opts.title ?? id,
    fields: { isTemplate: opts.isTemplate ?? false },
    createdAt: 0,
    updatedAt: 0,
    createdBy: '',
    modifiedBy: '',
  });
  for (const [key, value] of Object.entries(opts.properties ?? {})) {
    setCardProperty(made, key, value);
  }
  return made;
}

const STATUS_PROP: PropertyTemplate = {
  id: 'status',
  name: 'Status',
  type: 'select',
  options: [
    { id: 'todo', label: 'To Do', color: 'blue' },
    { id: 'doing', label: 'In Progress', color: 'yellow' },
    { id: 'done', label: 'Completed 🙌', color: 'green' },
  ],
};

const CHECKBOX_PROP: PropertyTemplate = {
  id: 'done-box',
  name: 'Completed',
  type: 'checkbox',
  options: [],
};

describe('card hierarchy depth, height, and traversal', () => {
  it('computes depth 1/2/3, subtree height, ancestors, children, and descendants', () => {
    const root = card('r');
    const childA = card('a', { parentId: 'r' });
    const childB = card('b', { parentId: 'r' });
    const grand = card('g', { parentId: 'a' });
    const tmpl = card('t', { isTemplate: true });

    const { byId, childrenMap } = buildHierarchyIndex([root, childA, childB, grand, tmpl]);

    expect(byId.has('t')).toBe(false);
    expect(cardDepth('r', byId)).toBe(1);
    expect(cardDepth('a', byId)).toBe(2);
    expect(cardDepth('b', byId)).toBe(2);
    expect(cardDepth('g', byId)).toBe(3);
    expect(cardDepth('missing', byId)).toBe(0);

    expect(subtreeHeight('r', childrenMap)).toBe(3);
    expect(subtreeHeight('a', childrenMap)).toBe(2);
    expect(subtreeHeight('b', childrenMap)).toBe(1);
    expect(subtreeHeight('g', childrenMap)).toBe(1);

    expect(ancestorsOf('r', byId).map((c) => c.id)).toEqual([]);
    expect(ancestorsOf('a', byId).map((c) => c.id)).toEqual(['r']);
    expect(ancestorsOf('g', byId).map((c) => c.id)).toEqual(['r', 'a']);

    expect(directChildrenOf('r', childrenMap).map((c) => c.id)).toEqual(['a', 'b']);
    expect(directChildrenOf('g', childrenMap)).toEqual([]);
    expect(descendantsOf('r', childrenMap).map((c) => c.id)).toEqual(['a', 'b', 'g']);

    expect(canAddSubcard('r', byId)).toBe(true);
    expect(canAddSubcard('a', byId)).toBe(true);
    expect(canAddSubcard('g', byId)).toBe(false);
    expect(canAddSubcard('t', byId)).toBe(false);
    expect(canAddSubcard('missing', byId)).toBe(false);
  });

  it('guards against cycles and dangling parent links without looping infinitely', () => {
    const a = card('a', { parentId: 'b' });
    const b = card('b', { parentId: 'a' });
    const self = card('s', { parentId: 's' });
    const orphan = card('o', { parentId: 'gone' });
    const { byId, childrenMap } = buildHierarchyIndex([a, b, self, orphan]);

    expect(cardDepth('a', byId)).toBe(2);
    expect(cardDepth('s', byId)).toBe(1);
    expect(cardDepth('o', byId)).toBe(1);
    expect(subtreeHeight('a', childrenMap)).toBe(2);
    expect(ancestorsOf('a', byId).map((c) => c.id)).toEqual(['b']);
    expect(ancestorsOf('s', byId)).toEqual([]);
    expect(ancestorsOf('o', byId)).toEqual([]);
    expect(descendantsOf('a', childrenMap).map((c) => c.id)).toEqual(['b']);
  });
});

describe('eligible parent and sub-card pickers', () => {
  it('filters eligible parents by depth + subtree height, cycles, and templates', () => {
    const d1 = card('d1');
    const d2 = card('d2', { parentId: 'd1' });
    const d3 = card('d3', { parentId: 'd2' });
    const solo = card('solo');
    const subRoot = card('subRoot');
    const subChild = card('subChild', { parentId: 'subRoot' });
    const tmpl = card('tmpl', { isTemplate: true });
    const all = [d1, d2, d3, solo, subRoot, subChild, tmpl];

    expect(MAX_CARD_DEPTH).toBe(3);

    // Leaf `solo` (height 1) can go under depth-1 or depth-2 cards, never depth-3 or template or self.
    expect(eligibleParentsFor('solo', all).map((c) => c.id)).toEqual([
      'd1',
      'd2',
      'subRoot',
      'subChild',
    ]);

    // `subRoot` (height 2) can only go under depth-1 cards (never self, descendant `subChild`, or depth-2+).
    expect(eligibleParentsFor('subRoot', all).map((c) => c.id)).toEqual(['d1', 'solo']);

    // `d1` (height 3) cannot move under any card without exceeding depth 3.
    expect(eligibleParentsFor('d1', all)).toEqual([]);

    // Template card has no eligible parents.
    expect(eligibleParentsFor('tmpl', all)).toEqual([]);
  });

  it('filters eligible existing cards to attach as sub-cards', () => {
    const d1 = card('d1');
    const d2 = card('d2', { parentId: 'd1' });
    const d3 = card('d3', { parentId: 'd2' });
    const solo = card('solo');
    const subRoot = card('subRoot');
    const subChild = card('subChild', { parentId: 'subRoot' });
    const tmpl = card('tmpl', { isTemplate: true });
    const all = [d1, d2, d3, solo, subRoot, subChild, tmpl];

    // Under depth-1 `solo`: height-1 and height-2 cards qualify; height-3 `d1` does not.
    expect(eligibleSubcardsFor('solo', all).map((c) => c.id)).toEqual([
      'd2',
      'd3',
      'subRoot',
      'subChild',
    ]);

    // Under depth-1 `d1`: excludes self, existing direct child `d2`, template, and itself (height 3).
    expect(eligibleSubcardsFor('d1', all).map((c) => c.id)).toEqual([
      'd3',
      'solo',
      'subRoot',
      'subChild',
    ]);

    // Under depth-2 `d2`: only height-1 leaves not ancestor/self/direct-child qualify.
    expect(eligibleSubcardsFor('d2', all).map((c) => c.id)).toEqual(['solo', 'subChild']);

    // Depth-3 and templates cannot accept any sub-cards.
    expect(eligibleSubcardsFor('d3', all)).toEqual([]);
    expect(eligibleSubcardsFor('tmpl', all)).toEqual([]);
  });
});

describe('sub-card completion progress', () => {
  it('computes progress from Status select property (preferring it over unrelated checkboxes)', () => {
    const reviewedBox: PropertyTemplate = {
      id: 'reviewed',
      name: 'Reviewed',
      type: 'checkbox',
      options: [],
    };
    const props = [STATUS_PROP, reviewedBox];
    expect(statusPropertyOf(props)?.id).toBe('status');

    const parent = card('p');
    const c1 = card('c1', { parentId: 'p', properties: { status: 'done' } });
    const c2 = card('c2', { parentId: 'p', properties: { status: 'todo', reviewed: 'true' } });
    const c3 = card('c3', { parentId: 'p', properties: { status: 'doing' } });
    const { childrenMap } = buildHierarchyIndex([parent, c1, c2, c3]);

    expect(isCardCompleted(c1, props)).toBe(true);
    expect(isCardCompleted(c2, props)).toBe(false);
    expect(statusOptionOf(c1, STATUS_PROP)?.label).toBe('Completed 🙌');
    expect(subcardProgress('p', childrenMap, props)).toEqual({ done: 1, total: 3, percent: 33 });
    expect(subcardProgress('c1', childrenMap, props)).toEqual({ done: 0, total: 0, percent: 0 });
  });

  it('falls back to green option on status select and to checkbox when no status select exists', () => {
    const koreanStatus: PropertyTemplate = {
      id: 'st',
      name: '상태',
      type: 'select',
      options: [
        { id: 'a', label: '미완료', color: 'gray' },
        { id: 'b', label: '완료', color: 'green' },
      ],
    };
    const negatedEnglish: PropertyTemplate = {
      id: 'st-en',
      name: 'Status',
      type: 'select',
      options: [
        { id: 'nd', label: 'Not done', color: 'gray' },
        { id: 'nc', label: 'Not completed', color: 'gray' },
        { id: 'ok', label: 'Done', color: 'green' },
      ],
    };
    const greenOnlyStatus: PropertyTemplate = {
      id: 'st2',
      name: 'Stage',
      type: 'select',
      options: [
        { id: 'a', label: 'Alpha', color: 'blue' },
        { id: 'b', label: 'Launch', color: 'green' },
      ],
    };

    expect(isCardCompleted(card('x', { properties: { st: 'a' } }), [koreanStatus])).toBe(false);
    expect(isCardCompleted(card('x', { properties: { st: 'b' } }), [koreanStatus])).toBe(true);
    expect(isCardCompleted(card('x', { properties: { 'st-en': 'nd' } }), [negatedEnglish])).toBe(
      false,
    );
    expect(isCardCompleted(card('x', { properties: { 'st-en': 'nc' } }), [negatedEnglish])).toBe(
      false,
    );
    expect(isCardCompleted(card('x', { properties: { 'st-en': 'ok' } }), [negatedEnglish])).toBe(
      true,
    );
    expect(isCardCompleted(card('x', { properties: { st2: 'b' } }), [greenOnlyStatus])).toBe(true);

    const parent = card('p');
    const c1 = card('c1', { parentId: 'p', properties: { 'done-box': 'true' } });
    const c2 = card('c2', { parentId: 'p', properties: {} });
    const { childrenMap } = buildHierarchyIndex([parent, c1, c2]);
    expect(subcardProgress('p', childrenMap, [CHECKBOX_PROP])).toEqual({
      done: 1,
      total: 2,
      percent: 50,
    });
    expect(subcardProgress('p', childrenMap, [])).toEqual({ done: 0, total: 2, percent: 0 });
  });
});

describe('hierarchical row grouping for Table and Gantt views', () => {
  it('returns flat rows unchanged on boards without hierarchy', () => {
    const rows = hierarchicalRows([card('c1'), card('c2')]);
    expect(
      rows.map((r) => ({ id: r.card.id, level: r.level, hasChildren: r.hasChildren })),
    ).toEqual([
      { id: 'c1', level: 0, hasChildren: false },
      { id: 'c2', level: 0, hasChildren: false },
    ]);
    expect(hasVisibleHierarchy(rows)).toBe(false);
  });

  it('places children directly beneath their parent and honors collapsed parents', () => {
    const p1 = card('p1');
    const p2 = card('p2');
    const c1 = card('c1', { parentId: 'p1' });
    const g1 = card('g1', { parentId: 'c1' });
    const c2 = card('c2', { parentId: 'p1' });

    const expanded = hierarchicalRows([p1, p2, c1, g1, c2]);
    expect(hasVisibleHierarchy(expanded)).toBe(true);
    expect(
      expanded.map((r) => ({
        id: r.card.id,
        level: r.level,
        hasChildren: r.hasChildren,
        collapsed: r.collapsed,
      })),
    ).toEqual([
      { id: 'p1', level: 0, hasChildren: true, collapsed: false },
      { id: 'c1', level: 1, hasChildren: true, collapsed: false },
      { id: 'g1', level: 2, hasChildren: false, collapsed: false },
      { id: 'c2', level: 1, hasChildren: false, collapsed: false },
      { id: 'p2', level: 0, hasChildren: false, collapsed: false },
    ]);

    const collapsedChild = hierarchicalRows([p1, p2, c1, g1, c2], new Set(['c1']));
    expect(collapsedChild.map((r) => r.card.id)).toEqual(['p1', 'c1', 'c2', 'p2']);
    expect(collapsedChild.find((r) => r.card.id === 'c1')?.collapsed).toBe(true);

    const collapsedParent = hierarchicalRows([p1, p2, c1, g1, c2], new Set(['p1']));
    expect(collapsedParent.map((r) => r.card.id)).toEqual(['p1', 'p2']);
    expect(collapsedParent[0]?.visibleChildCount).toBe(2);
    expect(collapsedParent[0]?.collapsed).toBe(true);
  });

  it('keeps children whose parent was filtered out at level 0 and recovers corrupt cycles', () => {
    const child = card('c1', { parentId: 'filtered-out-parent' });
    const grand = card('g1', { parentId: 'c1' });
    const rows = hierarchicalRows([child, grand]);
    expect(rows.map((r) => ({ id: r.card.id, level: r.level }))).toEqual([
      { id: 'c1', level: 0 },
      { id: 'g1', level: 1 },
    ]);

    const cycA = card('ca', { parentId: 'cb' });
    const cycB = card('cb', { parentId: 'ca' });
    const cycRows = hierarchicalRows([cycA, cycB]);
    expect(cycRows.map((r) => r.card.id)).toEqual(['ca', 'cb']);
  });
});
