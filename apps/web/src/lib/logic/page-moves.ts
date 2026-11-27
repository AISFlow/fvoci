/**
 * Moving a page in its wiki's tree, as the reader asks for it: the place a
 * page is at (to go back to), the place a step of the keyboard takes it to
 * (up, down, in and out), and the tree as it will be once the server takes
 * the move (shown at once).
 *
 * A place is said as the server takes it (`movePage`): below `parentId`
 * (empty for the top), right after `after` (empty for the first place,
 * left out for the last).
 */
import { orderBetween } from './page-order.ts';
import { ancestorsOf, comparePlaces, type PlacedPage } from './page-tree.ts';

/** Where a page goes, as `movePage` takes it. */
export interface PagePlace {
  parentId: string;
  after?: string;
}

/** Rules a tree places on where a node may move (e.g. a course's depth-2 chapter > unit rule). */
export interface TreeRules<P extends PlacedPage = PlacedPage> {
  /** Whether `moving` may be placed directly below `parent` (`undefined` for the top level). */
  canBeUnder?: (moving: P, parent: P | undefined) => boolean;
  /** Maximum depth from the top (1 = top only, 2 = chapter > unit); unlimited when omitted. */
  maxDepth?: number;
}

/** Rules for a course outline: depth at most 2, chapters top-level only, only chapters hold units. */
export function courseTreeRules<
  P extends PlacedPage & { kind?: 'chapter' | 'unit' },
>(): TreeRules<P> {
  return {
    maxDepth: 2,
    canBeUnder: (moving, parent) =>
      parent === undefined || (moving.kind !== 'chapter' && parent.kind === 'chapter'),
  };
}

/** How many levels `pageId` and its descendants take (1 for a leaf). */
function subtreeHeight(
  pages: readonly PlacedPage[],
  pageId: string,
  seen = new Set<string>([pageId]),
): number {
  let height = 1;
  for (const child of pages) {
    if (child.parentId === pageId && !seen.has(child.id)) {
      seen.add(child.id);
      height = Math.max(height, 1 + subtreeHeight(pages, child.id, seen));
    }
  }
  return height;
}

/** Whether `moving` may be placed directly under `parent` (`undefined` for the top) under `rules`. */
export function allowsUnder<P extends PlacedPage>(
  pages: readonly P[],
  moving: P,
  parent: P | undefined,
  rules?: TreeRules<P>,
): boolean {
  if (!rules) return true;
  if (rules.canBeUnder && !rules.canBeUnder(moving, parent)) return false;
  if (rules.maxDepth !== undefined) {
    const parentDepth = parent ? ancestorsOf(pages, parent.id).length + 1 : 0;
    if (parentDepth + subtreeHeight(pages, moving.id) > rules.maxDepth) return false;
  }
  return true;
}

/** A step of the keyboard: among the siblings, into the page above, out to the parent's level. */
export type MoveStep = 'up' | 'down' | 'in' | 'out';

/** The pages below `parentId`, in their order. */
function siblingsOf<P extends PlacedPage>(pages: readonly P[], parentId: string): P[] {
  return pages.filter((page) => page.parentId === parentId).sort(comparePlaces);
}

/** Where the page is now: below its parent, right after the sibling before it. */
export function placeOf(pages: readonly PlacedPage[], pageId: string): PagePlace | undefined {
  const page = pages.find((one) => one.id === pageId);
  if (!page) return undefined;
  const siblings = siblingsOf(pages, page.parentId);
  const at = siblings.findIndex((one) => one.id === pageId);
  return { parentId: page.parentId, after: at > 0 ? (siblings[at - 1]?.id ?? '') : '' };
}

/** Where a step of the keyboard takes the page; undefined where it goes nowhere. */
export function stepped<P extends PlacedPage>(
  pages: readonly P[],
  pageId: string,
  step: MoveStep,
  rules?: TreeRules<P>,
): PagePlace | undefined {
  const page = pages.find((one) => one.id === pageId);
  if (!page) return undefined;
  const siblings = siblingsOf(pages, page.parentId);
  const at = siblings.findIndex((one) => one.id === pageId);
  const parentId = page.parentId;
  switch (step) {
    case 'up':
      if (at < 1) return undefined;
      return { parentId, after: at > 1 ? (siblings[at - 2]?.id ?? '') : '' };
    case 'down': {
      const next = siblings[at + 1];
      return next ? { parentId, after: next.id } : undefined;
    }
    case 'in': {
      const above = siblings[at - 1];
      if (!above || !allowsUnder(pages, page, above, rules)) return undefined;
      return { parentId: above.id };
    }
    case 'out': {
      if (parentId === '') return undefined;
      const grandParentId = pages.find((one) => one.id === parentId)?.parentId ?? '';
      const grandParent = grandParentId ? pages.find((one) => one.id === grandParentId) : undefined;
      if (!allowsUnder(pages, page, grandParent, rules)) return undefined;
      return { parentId: grandParentId, after: parentId };
    }
    default:
      return undefined;
  }
}

/** Whether the page is below `to.parentId` at the place `to.after` says. */
export function isPlaced(pages: readonly PlacedPage[], pageId: string, to: PagePlace): boolean {
  const now = placeOf(pages, pageId);
  if (now?.parentId !== to.parentId) return false;
  if (to.after !== undefined) return now.after === to.after;
  return siblingsOf(pages, to.parentId).at(-1)?.id === pageId;
}

/** Whether the move changes where the page is. */
export function movesAnywhere(
  pages: readonly PlacedPage[],
  pageId: string,
  to: PagePlace,
): boolean {
  const now = placeOf(pages, pageId);
  if (now?.parentId !== to.parentId) return true;
  const siblings = siblingsOf(pages, to.parentId).filter((one) => one.id !== pageId);
  const last = siblings.at(-1)?.id ?? '';
  return (to.after ?? last) !== now.after;
}

/**
 * The pages as they will be once the page is moved: below its new parent,
 * at an order between its new neighbours. Where they have no order between
 * them the server numbers them anew, and tells; meanwhile it goes last.
 */
export function moved<P extends PlacedPage>(
  pages: readonly P[],
  pageId: string,
  to: PagePlace,
): P[] {
  const siblings = siblingsOf(pages, to.parentId).filter((one) => one.id !== pageId);
  let at = siblings.length;
  if (to.after === '') at = 0;
  else if (to.after !== undefined) at = siblings.findIndex((one) => one.id === to.after) + 1;
  let order: string;
  try {
    order = orderBetween(siblings[at - 1]?.order, siblings[at]?.order);
  } catch {
    order = '~';
  }
  return pages.map((page) =>
    page.id === pageId ? { ...page, parentId: to.parentId, order } : page,
  );
}
