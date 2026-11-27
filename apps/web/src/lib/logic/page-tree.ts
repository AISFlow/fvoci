/**
 * The pages of a wiki as a tree. The backend lists them flat: each names
 * the page it is below (`parentId`, empty at the top), and its place among
 * the pages of the same parent (`order`). Siblings go by `order` as strings
 * compare by code unit, then by `id` where two have the same, which is how
 * the server lists them (`getPageTree`); never by a locale, which would put
 * them in another order than the server's.
 *
 * A page whose parent is not in the list (one heard of before its parent, or
 * left behind by a deletion not heard of yet) is shown at the top, so that
 * nothing the list has goes unseen.
 */

/** What the tree needs of a page. */
export interface PlacedPage {
  id: string;
  parentId: string;
  order: string;
}

export interface PageTreeNode<P extends PlacedPage> {
  page: P;
  children: PageTreeNode<P>[];
}

/** One line of the tree as it is drawn: the open branches unfolded, the folded ones not. */
export interface PageRow<P extends PlacedPage> {
  page: P;
  /** 0 at the top of the wiki. */
  depth: number;
  hasChildren: boolean;
  /** Its children are shown. */
  open: boolean;
  /** The page whose children it is among; undefined at the top. */
  parentId: string | undefined;
  /** Its place among its siblings, from 1, and how many they are. */
  position: number;
  siblings: number;
}

/** Code units, not a locale: `<` on strings. */
function compareText(first: string, second: string): number {
  if (first === second) return 0;
  return first < second ? -1 : 1;
}

/** The order of two siblings: by `order`, then by `id`, both as code units. */
export function comparePlaces(first: PlacedPage, second: PlacedPage): number {
  return compareText(first.order, second.order) || compareText(first.id, second.id);
}

/** The tree of `pages`; each list of siblings in their order. */
export function pageTree<P extends PlacedPage>(pages: readonly P[]): PageTreeNode<P>[] {
  const ids = new Set(pages.map((page) => page.id));
  const below = new Map<string, P[]>();
  for (const page of pages) {
    // At the top: its parent is no page of the list (or itself).
    const parent = page.parentId !== page.id && ids.has(page.parentId) ? page.parentId : '';
    below.set(parent, [...(below.get(parent) ?? []), page]);
  }
  const placed = new Set<string>();
  const grow = (parentId: string): PageTreeNode<P>[] =>
    [...(below.get(parentId) ?? [])]
      .sort(comparePlaces)
      .filter((page) => !placed.has(page.id))
      .map((page) => {
        placed.add(page.id);
        return { page, children: grow(page.id) };
      });
  const roots = grow('');
  // Pages that are each other's parents reach no top: they are put there.
  const loose = pages.filter((page) => !placed.has(page.id)).sort(comparePlaces);
  for (const page of loose) {
    if (!placed.has(page.id)) {
      placed.add(page.id);
      roots.push({ page, children: grow(page.id) });
    }
  }
  return roots;
}

/** The lines of the tree as drawn: a page's children follow it while it is open (`isOpen`). */
export function pageRows<P extends PlacedPage>(
  tree: readonly PageTreeNode<P>[],
  isOpen: (pageId: string) => boolean,
): PageRow<P>[] {
  const rows: PageRow<P>[] = [];
  const walk = (nodes: readonly PageTreeNode<P>[], depth: number, parentId?: string): void => {
    for (const [at, node] of nodes.entries()) {
      const hasChildren = node.children.length > 0;
      const open = hasChildren && isOpen(node.page.id);
      const place = { position: at + 1, siblings: nodes.length };
      rows.push({ page: node.page, depth, hasChildren, open, parentId, ...place });
      if (open) walk(node.children, depth + 1, node.page.id);
    }
  };
  walk(tree, 0);
  return rows;
}

/** The pages above `pageId`, its parent first; none for a page at the top or not listed. */
export function ancestorsOf(pages: readonly PlacedPage[], pageId: string): string[] {
  const byId = new Map(pages.map((page) => [page.id, page]));
  const found: string[] = [];
  let parentId = byId.get(pageId)?.parentId;
  while (parentId && byId.has(parentId) && !found.includes(parentId) && parentId !== pageId) {
    found.push(parentId);
    parentId = byId.get(parentId)?.parentId;
  }
  return found;
}

/** `pageId` and every page below it in the list. */
export function withDescendants(pages: readonly PlacedPage[], pageId: string): Set<string> {
  const gone = new Set([pageId]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const page of pages) {
      if (!gone.has(page.id) && gone.has(page.parentId)) {
        gone.add(page.id);
        grew = true;
      }
    }
  }
  return gone;
}

/** The first page of the tree as drawn: the first at the top. */
export function firstPage<P extends PlacedPage>(pages: readonly P[]): P | undefined {
  return pageTree(pages)[0]?.page;
}

/**
 * The first unit of a course's outline in display order (depth-first, skipping
 * title-only chapters). Returns undefined when the course has no units yet.
 */
export function firstUnit<P extends PlacedPage & { kind?: 'chapter' | 'unit' }>(
  nodes: readonly P[],
): P | undefined {
  const find = (branch: readonly PageTreeNode<P>[]): P | undefined => {
    for (const node of branch) {
      if (node.page.kind !== 'chapter') return node.page;
      const inside = find(node.children);
      if (inside) return inside;
    }
    return undefined;
  };
  return find(pageTree(nodes));
}

/** The most characters of a page's title, as the server takes a block's (`BLOCK_TITLE_MAX_CHARS`). */
export const PAGE_TITLE_MAX = 16_383;
