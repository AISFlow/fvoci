/**
 * Pure O(N) card hierarchy calculations for a board:
 * - Depth (1 = top-level, 2 = child, 3 = grandchild, max 3) and subtree height
 * - Ancestors, direct children, descendants, and eligible parent / sub-card pickers
 * - Sub-card completion progress (matching board status select or checkbox conventions)
 * - Hierarchical grouping / expand-collapse flattening for Table and Gantt views
 *
 * Every graph walk guards against broken links and hostile cycles so corrupt or
 * concurrent data never loops infinitely or drops visible cards.
 */
import {
  cardProperty,
  isCardTemplate,
  type Card,
  type PropertyOption,
  type PropertyTemplate,
} from './wire.ts';

/** Maximum allowed nesting depth: top-level = 1, child = 2, grandchild = 3. */
export const MAX_CARD_DEPTH = 3;

/** Minimal card shape needed to build or query a board's card hierarchy. */
export type HierarchyCard = Card;

/** Pre-indexed lookup tables for O(1) parent and direct-child queries. */
export interface HierarchyIndex<T extends HierarchyCard> {
  /** Real (non-template) cards by identifier. */
  readonly byId: ReadonlyMap<string, T>;
  /** Direct real children for each parent card ID, preserving input order. */
  readonly childrenMap: ReadonlyMap<string, readonly T[]>;
}

/** Completion progress across a card's direct sub-cards. */
export interface SubcardProgress {
  done: number;
  total: number;
  /** Integer 0..100 (0 when `total === 0`). */
  percent: number;
}

/** One row in a hierarchically grouped Table or Gantt list. */
export interface HierarchicalRow<T> {
  card: T;
  /** Visual nesting level inside the current visible list (0, 1, or 2). */
  level: number;
  /** Whether this card has at least one visible child in the current list. */
  hasChildren: boolean;
  /** Number of visible direct children of this card in the current list. */
  visibleChildCount: number;
  /** Whether this card's children are currently collapsed. */
  collapsed: boolean;
}

const EMPTY_LIST: readonly never[] = [];

const STATUS_NAME_PATTERN = /status|state|stage|상태|진행/i;
const NOT_DONE_PATTERN =
  /\bnot\s+(done|complete|completed|closed|finished|published|resolved|shipped)\b|미완료|미종료/i;
const DONE_OPTION_PATTERN =
  /\b(done|complete|completed|closed|finished|published|resolved|shipped)\b|완료|종료|끝/i;
const DONE_CHECKBOX_PATTERN = /done|complete|completed|finish|완료|체크/i;

function isDoneOptionLabel(label: string): boolean {
  return !NOT_DONE_PATTERN.test(label) && DONE_OPTION_PATTERN.test(label);
}

/**
 * Builds `byId` and `childrenMap` in one O(N) pass over `cards`, ignoring template cards.
 */
export function buildHierarchyIndex<T extends HierarchyCard>(
  cards: readonly T[],
): HierarchyIndex<T> {
  const byId = new Map<string, T>();
  for (const card of cards) {
    if (!isCardTemplate(card)) byId.set(card.id, card);
  }
  const mutableChildren = new Map<string, T[]>();
  for (const card of cards) {
    if (isCardTemplate(card)) continue;
    const { parentId } = card;
    if (parentId === '' || parentId === card.id || !byId.has(parentId)) continue;
    const list = mutableChildren.get(parentId);
    if (list) list.push(card);
    else mutableChildren.set(parentId, [card]);
  }
  return { byId, childrenMap: mutableChildren };
}

/**
 * Depth of `cardId` in `byId`: 1 for top-level, 2 for child, 3 for grandchild.
 * Returns 0 when `cardId` is not present. Safe against broken parent references and cycles.
 */
export function cardDepth(
  cardId: string,
  byId: ReadonlyMap<string, Pick<Card, 'id' | 'parentId'>>,
): number {
  if (!byId.has(cardId)) return 0;
  let depth = 0;
  let cursor = cardId;
  const seen = new Set<string>();
  while (cursor !== '' && !seen.has(cursor)) {
    const card = byId.get(cursor);
    if (!card) break;
    seen.add(cursor);
    depth += 1;
    cursor = card.parentId;
  }
  return depth;
}

/**
 * Height of the subtree rooted at `cardId`: 1 for a leaf, 2 with direct children,
 * 3 with grandchildren. Safe against cycles.
 */
export function subtreeHeight(
  cardId: string,
  childrenMap: ReadonlyMap<string, readonly Pick<Card, 'id'>[]>,
): number {
  let height = 1;
  const seen = new Set<string>([cardId]);
  let frontier: readonly string[] = [cardId];
  while (frontier.length > 0) {
    const next: string[] = [];
    for (const id of frontier) {
      for (const child of childrenMap.get(id) ?? EMPTY_LIST) {
        if (!seen.has(child.id)) {
          seen.add(child.id);
          next.push(child.id);
        }
      }
    }
    if (next.length === 0) break;
    height += 1;
    frontier = next;
  }
  return height;
}

/**
 * Ancestors of `cardId` ordered outermost-first (`[grandparent, parent]` for depth 3,
 * `[parent]` for depth 2, `[]` for depth 1). Safe against cycles and missing parents.
 */
export function ancestorsOf<T extends Pick<Card, 'id' | 'parentId'>>(
  cardId: string,
  byId: ReadonlyMap<string, T>,
): T[] {
  const start = byId.get(cardId);
  if (!start) return [];
  const chain: T[] = [];
  const seen = new Set<string>([cardId]);
  let cursor = start.parentId;
  while (cursor !== '' && !seen.has(cursor)) {
    const parent = byId.get(cursor);
    if (!parent) break;
    seen.add(cursor);
    chain.push(parent);
    cursor = parent.parentId;
  }
  return chain.reverse();
}

/** Direct child cards of `cardId`, preserving the index's order. */
export function directChildrenOf<T>(
  cardId: string,
  childrenMap: ReadonlyMap<string, readonly T[]>,
): readonly T[] {
  return childrenMap.get(cardId) ?? EMPTY_LIST;
}

/** All descendants of `cardId` in breadth-first order. Safe against cycles. */
export function descendantsOf<T extends Pick<Card, 'id'>>(
  cardId: string,
  childrenMap: ReadonlyMap<string, readonly T[]>,
): T[] {
  const result: T[] = [];
  const seen = new Set<string>([cardId]);
  let head = 0;
  const queue: string[] = [cardId];
  while (head < queue.length) {
    const id = queue[head++];
    if (id === undefined) break;
    for (const child of childrenMap.get(id) ?? EMPTY_LIST) {
      if (seen.has(child.id)) continue;
      seen.add(child.id);
      result.push(child);
      queue.push(child.id);
    }
  }
  return result;
}

/**
 * Whether `cardId` may have a sub-card added under it:
 * must exist in `byId`, not be a template, and sit at depth < `MAX_CARD_DEPTH` (depth 1 or 2).
 */
export function canAddSubcard(cardId: string, byId: ReadonlyMap<string, HierarchyCard>): boolean {
  const card = byId.get(cardId);
  if (!card || isCardTemplate(card)) return false;
  return cardDepth(cardId, byId) < MAX_CARD_DEPTH;
}

/**
 * Cards on the board that can legally become the parent of `cardId`:
 * excludes template cards, `cardId` itself, all descendants of `cardId`, and any
 * candidate where `candidateDepth + movingSubtreeHeight > MAX_CARD_DEPTH`.
 */
export function eligibleParentsFor<T extends HierarchyCard>(
  cardId: string,
  cards: readonly T[],
  index: HierarchyIndex<T> = buildHierarchyIndex(cards),
): T[] {
  const { byId, childrenMap } = index;
  const moving = byId.get(cardId);
  if (!moving || isCardTemplate(moving)) return [];
  const movingHeight = subtreeHeight(cardId, childrenMap);
  const forbidden = new Set<string>([
    cardId,
    ...descendantsOf(cardId, childrenMap).map((card) => card.id),
  ]);
  const eligible: T[] = [];
  for (const candidate of cards) {
    if (isCardTemplate(candidate) || forbidden.has(candidate.id)) continue;
    if (cardDepth(candidate.id, byId) + movingHeight <= MAX_CARD_DEPTH) {
      eligible.push(candidate);
    }
  }
  return eligible;
}

/**
 * Existing cards on the board that can legally be attached as direct sub-cards of `parentId`:
 * excludes template cards, `parentId` itself, ancestors of `parentId`, cards already directly
 * under `parentId`, and any card whose subtree would exceed `MAX_CARD_DEPTH` beneath `parentId`.
 */
export function eligibleSubcardsFor<T extends HierarchyCard>(
  parentId: string,
  cards: readonly T[],
  index: HierarchyIndex<T> = buildHierarchyIndex(cards),
): T[] {
  const { byId, childrenMap } = index;
  if (!canAddSubcard(parentId, byId)) return [];
  const parentDepth = cardDepth(parentId, byId);
  const forbidden = new Set<string>([
    parentId,
    ...ancestorsOf(parentId, byId).map((card) => card.id),
  ]);
  const eligible: T[] = [];
  for (const candidate of cards) {
    if (isCardTemplate(candidate) || forbidden.has(candidate.id)) continue;
    if (candidate.parentId === parentId) continue;
    if (parentDepth + subtreeHeight(candidate.id, childrenMap) <= MAX_CARD_DEPTH) {
      eligible.push(candidate);
    }
  }
  return eligible;
}

/** Finds the board's primary completion property: status select first, else completion checkbox. */
export function statusPropertyOf(
  properties: readonly PropertyTemplate[],
): PropertyTemplate | undefined {
  const selects = properties.filter((template) => template.type === 'select');
  const namedStatus = selects.find((template) => STATUS_NAME_PATTERN.test(template.name));
  if (namedStatus) return namedStatus;
  const doneSelect = selects.find((template) =>
    template.options.some((option) => isDoneOptionLabel(option.label)),
  );
  if (doneSelect) return doneSelect;
  const checkboxes = properties.filter((template) => template.type === 'checkbox');
  return checkboxes.find((template) => DONE_CHECKBOX_PATTERN.test(template.name)) ?? checkboxes[0];
}

/** Resolves the selected option of a `select` status property on `card`, if any. */
export function statusOptionOf(
  card: Card,
  property: PropertyTemplate | undefined,
): PropertyOption | undefined {
  if (property?.type !== 'select') return undefined;
  const value = cardProperty(card, property.id);
  if (typeof value !== 'string' || value === '') return undefined;
  return property.options.find((option) => option.id === value);
}

function isCompletedWith(
  card: Card,
  prop: PropertyTemplate | undefined,
  hasKeywordOption: boolean,
): boolean {
  if (!prop) return false;
  if (prop.type === 'checkbox') return cardProperty(card, prop.id) === 'true';
  const chosen = statusOptionOf(card, prop);
  if (!chosen) return false;
  if (isDoneOptionLabel(chosen.label)) return true;
  return !hasKeywordOption && chosen.color === 'green';
}

/** Whether `card` counts as completed according to the board's status or checkbox property. */
export function isCardCompleted(
  card: Card,
  properties: readonly PropertyTemplate[],
  prop: PropertyTemplate | undefined = statusPropertyOf(properties),
): boolean {
  const hasKeywordOption =
    prop?.type === 'select' && prop.options.some((option) => isDoneOptionLabel(option.label));
  return isCompletedWith(card, prop, hasKeywordOption);
}

/** Completion progress across direct sub-cards of `cardId`. */
export function subcardProgress(
  cardId: string,
  childrenMap: ReadonlyMap<string, readonly Card[]>,
  properties: readonly PropertyTemplate[],
  prop: PropertyTemplate | undefined = statusPropertyOf(properties),
): SubcardProgress {
  const children = directChildrenOf(cardId, childrenMap);
  const total = children.length;
  if (total === 0) return { done: 0, total: 0, percent: 0 };
  const hasKeywordOption =
    prop?.type === 'select' && prop.options.some((option) => isDoneOptionLabel(option.label));
  let done = 0;
  for (const child of children) {
    if (isCompletedWith(child, prop, hasKeywordOption)) done += 1;
  }
  return { done, total, percent: Math.round((done / total) * 100) };
}

/**
 * Orders visible `cards` so children sit immediately below their visible parent (depth 0/1/2),
 * omitting descendants of collapsed parents.
 *
 * Cards whose parent is absent from `cards` (for example, filtered out or in another group)
 * remain at level 0 so no visible card is ever lost. Any members of a corrupt cycle are also
 * appended at level 0 rather than dropped.
 */
export function hierarchicalRows<T extends Pick<Card, 'id' | 'parentId'>>(
  cards: readonly T[],
  collapsedIds?: ReadonlySet<string>,
): HierarchicalRow<T>[] {
  if (cards.length === 0) return [];
  const visibleIds = new Set(cards.map((card) => card.id));
  const childrenByParent = new Map<string, T[]>();
  const roots: T[] = [];
  for (const card of cards) {
    const { parentId } = card;
    if (parentId !== '' && parentId !== card.id && visibleIds.has(parentId)) {
      const list = childrenByParent.get(parentId);
      if (list) list.push(card);
      else childrenByParent.set(parentId, [card]);
    } else {
      roots.push(card);
    }
  }
  // Fast path: flat list with no visible parent-child relationships.
  if (childrenByParent.size === 0) {
    return cards.map((card) => ({
      card,
      level: 0,
      hasChildren: false,
      visibleChildCount: 0,
      collapsed: false,
    }));
  }

  const rows: HierarchicalRow<T>[] = [];
  const visited = new Set<string>();
  const markHidden = (parentId: string): void => {
    for (const child of childrenByParent.get(parentId) ?? EMPTY_LIST) {
      if (visited.has(child.id)) continue;
      visited.add(child.id);
      markHidden(child.id);
    }
  };
  const emit = (card: T, level: number): void => {
    if (visited.has(card.id)) return;
    visited.add(card.id);
    const kids = childrenByParent.get(card.id) ?? EMPTY_LIST;
    const hasChildren = kids.length > 0;
    const collapsed = hasChildren && (collapsedIds?.has(card.id) ?? false);
    rows.push({
      card,
      level,
      hasChildren,
      visibleChildCount: kids.length,
      collapsed,
    });
    if (hasChildren) {
      if (collapsed) markHidden(card.id);
      else for (const child of kids) emit(child, Math.min(level + 1, MAX_CARD_DEPTH - 1));
    }
  };

  for (const root of roots) emit(root, 0);
  // Recover any cards stuck in a corrupt cycle without a root.
  for (const card of cards) {
    if (!visited.has(card.id)) emit(card, 0);
  }
  return rows;
}

/** Whether any row in `rows` participates in a visible parent-child relationship. */
export function hasVisibleHierarchy<T>(rows: readonly HierarchicalRow<T>[]): boolean {
  return rows.some((row) => row.hasChildren || row.level > 0);
}
