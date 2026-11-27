/**
 * The tree of a wiki's pages as the server decides over it
 * (`crates/model/src/page_tree.rs`): where a page may go, how deep the pages
 * are and how many, and the place a page takes among its siblings, the
 * siblings numbered anew where there is no room left between them.
 */
import { BackendError, problemOf, refusedMember } from '../backend.ts';

import { orderBetween } from '../../logic/page-order.ts';
import { fail } from './refuse.ts';

/** How deep the pages of a wiki may go (a page at the top is at depth 1), as the server's default. */
export const PAGE_DEPTH = 10;
/** How many pages a wiki may have, those in the trash not counted, as the server's default. */
export const WIKI_PAGES = 5000;
/** How deep a course's outline may go (chapter > unit). */
export const COURSE_DEPTH = 2;
/** How many units a course may have (chapters and trash excluded), as the server's default. */
export const COURSE_UNITS = 2000;
/** How many chapters a course may have (trash excluded), as the server's default. */
export const COURSE_CHAPTERS = 200;
/** The longest order a page is given before its siblings are numbered anew. */
const ORDER_MAX_CHARS = 32;

const DIGITS = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz';

/** The limits a memory wiki is held to: the server's defaults unless a test says otherwise. */
export interface PageLimits {
  depth: number;
  pages: number;
}

/** The limits a memory course is held to: the server's defaults unless a test says otherwise. */
export interface CourseLimits {
  depth: number;
  units: number;
  chapters: number;
}

export const DEFAULT_LIMITS: PageLimits = { depth: PAGE_DEPTH, pages: WIKI_PAGES };
export const DEFAULT_COURSE_LIMITS: CourseLimits = {
  depth: COURSE_DEPTH,
  units: COURSE_UNITS,
  chapters: COURSE_CHAPTERS,
};

/** A page as the tree's decisions need it. */
export interface TreeNode {
  id: string;
  parentId: string;
  order: string;
}

/** Where a page goes: its order, and the siblings numbered anew, with their orders. */
export interface Place {
  order: string;
  renumbered: [string, string][];
}

/** Refuses as the server does: the reason of the problem, its limit and, for depth, `parentId`. */
export function refuseLimit(
  reason: 'page_too_deep' | 'too_many_pages' | 'too_many_units' | 'too_many_chapters',
  limit: number,
): never {
  const detail =
    reason === 'page_too_deep'
      ? `the page would be deeper than ${limit}`
      : reason === 'too_many_units'
        ? `the course would have more than ${limit} units`
        : reason === 'too_many_chapters'
          ? `the course would have more than ${limit} chapters`
          : `the wiki would have more than ${limit} pages`;
  throw new BackendError(400, {
    ...problemOf(400, detail),
    type: `/problems/${reason.replaceAll('_', '-')}`,
    reason,
    limit,
    ...(reason === 'page_too_deep' ? { errors: [refusedMember('parentId', detail)] } : {}),
  });
}

const byPlace = (one: TreeNode, other: TreeNode): number => {
  if (one.order !== other.order) return one.order < other.order ? -1 : 1;
  if (one.id === other.id) return 0;
  return one.id < other.id ? -1 : 1;
};

function isOrder(order: string): boolean {
  return /^[0-9A-Za-z]*[1-9A-Za-z]$/u.test(order);
}

/** `count` orders, each after the one before, spread evenly over as few digits as leave room. */
export function spreadOrders(count: number): string[] {
  const base = BigInt(DIGITS.length);
  const slots = BigInt(count + 1);
  let width = 1;
  while (base ** BigInt(width) < slots * 2n) width += 1;
  const span = base ** BigInt(width);
  return Array.from({ length: count }, (_, index) => {
    let value = (BigInt(index + 1) * span) / slots;
    let order = '';
    for (let place = 0; place < width; place += 1) {
      order = DIGITS.charAt(Number(value % base)) + order;
      value /= base;
    }
    return order.replace(/0+$/u, '');
  });
}

export class PageTree {
  readonly #pages = new Map<string, TreeNode>();
  readonly #children = new Map<string, string[]>();

  constructor(pages: Iterable<TreeNode>) {
    for (const page of pages) {
      this.#pages.set(page.id, page);
      const siblings = this.#children.get(page.parentId) ?? [];
      siblings.push(page.id);
      this.#children.set(page.parentId, siblings);
    }
  }

  get size(): number {
    return this.#pages.size;
  }

  has(pageId: string): boolean {
    return this.#pages.has(pageId);
  }

  /** How many pages lead from the top to the page, itself counted: 0 for the top (''). */
  depth(pageId: string): number {
    let depth = 0;
    let at = this.#pages.get(pageId);
    while (at && depth <= this.#pages.size) {
      depth += 1;
      at = this.#pages.get(at.parentId);
    }
    return depth;
  }

  /** The page and every page below it, a parent before what is below it. */
  below(pageId: string): string[] {
    if (!this.#pages.has(pageId)) return [];
    const found = [pageId];
    const seen = new Set(found);
    // The list grows as it is walked: each page found brings the pages below it.
    for (const at of found) {
      for (const child of this.#children.get(at) ?? []) {
        if (!seen.has(child)) {
          seen.add(child);
          found.push(child);
        }
      }
    }
    return found;
  }

  /** How many levels the page and the pages below it take: 1 for a page with none below. */
  height(pageId: string): number {
    const levels = new Map<string, number>();
    let height = 0;
    for (const id of this.below(pageId)) {
      const parent = this.#pages.get(id)?.parentId ?? '';
      const level = id === pageId ? 1 : (levels.get(parent) ?? 0) + 1;
      levels.set(id, level);
      height = Math.max(height, level);
    }
    return height;
  }

  /** Refuses `height` levels of pages, `adding` of them new, below `parentId`, as the server does. */
  checkRoom(parentId: string, height: number, adding: number, limits: PageLimits): void {
    if (parentId !== '' && !this.#pages.has(parentId)) {
      fail(400, 'the parent is no page of the wiki', { field: 'parentId' });
    }
    if (this.depth(parentId) + height > limits.depth) refuseLimit('page_too_deep', limits.depth);
    if (adding > 0 && this.#pages.size + adding > limits.pages) {
      refuseLimit('too_many_pages', limits.pages);
    }
  }

  /**
   * Refuses a move of the page below itself, below a page below it, or deeper
   * than the limit and than it is (a limit lowered since leaves it movable).
   */
  checkMove(pageId: string, parentId: string, limits: PageLimits): void {
    if (parentId !== '' && this.below(pageId).includes(parentId)) {
      fail(400, 'the parent is the page itself or below it', { field: 'parentId' });
    }
    if (parentId !== '' && !this.#pages.has(parentId)) {
      fail(400, 'the parent is no page of the wiki', { field: 'parentId' });
    }
    const height = this.height(pageId);
    const deepest = this.depth(parentId) + height;
    // Among its siblings too, however far past a limit lowered since it is.
    if (deepest > limits.depth && deepest > Math.max(0, this.depth(pageId) + height - 1)) {
      refuseLimit('page_too_deep', limits.depth);
    }
  }

  /** Whether the page is already below `parentId` where `after` says: a move there changes nothing. */
  isAt(pageId: string, parentId: string, after: string | undefined): boolean {
    const others = this.siblings(parentId, pageId);
    let at = others.length;
    if (after === '') at = 0;
    else if (after !== undefined) {
      const found = others.findIndex((page) => page.id === after);
      if (found < 0) fail(400, 'no other page of the same parent', { field: 'after' });
      at = found + 1;
    }
    return this.siblings(parentId).findIndex((page) => page.id === pageId) === at;
  }

  /** The pages below `parentId` but `except`, in the order they are listed in. */
  siblings(parentId: string, except = ''): TreeNode[] {
    return (this.#children.get(parentId) ?? [])
      .filter((id) => id !== except)
      .map((id) => this.#pages.get(id))
      .filter((page): page is TreeNode => page !== undefined)
      .sort(byPlace);
  }

  /** Where a page goes below `parentId` as `after` says (`PageDraft.after`), among the pages there but `except`. */
  place(parentId: string, after: string | undefined, except = ''): Place {
    const siblings = this.siblings(parentId, except);
    let at = siblings.length;
    if (after === '') at = 0;
    else if (after !== undefined) {
      const found = siblings.findIndex((page) => page.id === after);
      if (found < 0) fail(400, 'no other page of the same parent', { field: 'after' });
      at = found + 1;
    }
    const inOrder =
      siblings.every((page) => isOrder(page.order)) &&
      siblings.every(
        (page, index) => index === 0 || (siblings[index - 1]?.order ?? '') < page.order,
      );
    if (inOrder) {
      const order = orderBetween(siblings[at - 1]?.order, siblings[at]?.order);
      if (order.length <= ORDER_MAX_CHARS) return { order, renumbered: [] };
    }
    const orders = spreadOrders(siblings.length + 1);
    const [order] = orders.splice(at, 1);
    const renumbered: [string, string][] = [];
    siblings.forEach((page, index) => {
      const fresh = orders[index] ?? '';
      if (page.order !== fresh) renumbered.push([page.id, fresh]);
    });
    return { order: order ?? '', renumbered };
  }
}
