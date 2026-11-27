/**
 * Dragging a node of a wiki's or course's tree to another place: between two
 * nodes, or onto one to go below it (when `rules` allows it). A mouse takes
 * the node once it has moved a few pixels; a finger rests on it first.
 *
 * When the hovered row cannot accept the dragged node as a child (e.g. a unit
 * in a course, or dragging a chapter), the row splits in half into `before`
 * and `after` instead of highlighting an invalid `inside` drop zone.
 */
import { AutoScroller } from '$lib/drag/auto-scroll.ts';
import { pointerDrag, type PointerDragSession } from '$lib/drag/pointer-drag.ts';
import { DRAG_SETTINGS } from '$lib/drag/settings.ts';
import type { Point } from '$lib/logic/drop-target.ts';
import { allowsUnder, type PagePlace, type TreeRules } from '$lib/logic/page-moves.ts';
import { comparePlaces, withDescendants, type PageRow } from '$lib/logic/page-tree.ts';
import type { TreeItem } from '$lib/state/index.ts';

/** How long a folded row is held over before it unfolds, in milliseconds. */
const UNFOLD_AFTER = 700;
/** How far in a level of the tree is drawn, in pixels (`PageTree`: 1rem a level). */
const LEVEL_WIDTH = 16;

/** Where a node let go would go: the row it is over, and how. */
export interface DropMark {
  /** The node whose row is under the pointer. */
  pageId: string;
  zone: 'before' | 'after' | 'inside';
  /** How deep the node would be: 0 at the top. */
  depth: number;
  to: PagePlace;
}

export interface TreeDragSource<P extends TreeItem = TreeItem> {
  rows: () => readonly PageRow<P>[];
  pages: () => readonly P[];
  /** Nodes may be dragged: by an editor. */
  enabled: () => boolean;
  /** Optional placement rules (e.g. course depth-2 and chapter-only parenting). */
  rules?: () => TreeRules<P> | undefined;
  /** Unfolds a node held over. */
  unfold: (pageId: string) => void;
  ondrop: (pageId: string, to: PagePlace) => void;
}

/** The node whose row is under a point, within the tree. */
function rowAt(root: HTMLElement, point: Point): HTMLElement | undefined {
  const hit = document.elementFromPoint(point.x, point.y);
  const row = hit?.closest<HTMLElement>('[data-page]');
  return row && root.contains(row) ? row : undefined;
}

/** The place before `row`'s node among its siblings, the dragged node left out. */
function before<P extends TreeItem>(
  pages: readonly P[],
  row: PageRow<P>,
  dragged: string,
): PagePlace {
  const parentId = row.parentId ?? '';
  const siblings = pages
    .filter((page) => page.parentId === parentId && page.id !== dragged)
    .sort(comparePlaces);
  const at = siblings.findIndex((page) => page.id === row.page.id);
  return { parentId, after: at > 0 ? (siblings[at - 1]?.id ?? '') : '' };
}

/**
 * The rows a node let go after `row` may go after: the row, and while each is
 * the last of its siblings, the row it is below, up to the top.
 */
function levelsAfter<P extends TreeItem>(
  rows: readonly PageRow<P>[],
  row: PageRow<P>,
): PageRow<P>[] {
  const levels = [row];
  let at = row;
  while (at.position === at.siblings && at.parentId !== undefined) {
    const above = rows.find((one) => one.page.id === at.parentId);
    if (!above) break;
    levels.push(above);
    at = above;
  }
  return levels;
}

export class TreeDrag<P extends TreeItem = TreeItem> {
  readonly #source: TreeDragSource<P>;
  /** The node being dragged. */
  dragged = $state<string | undefined>(undefined);
  /** Where it would go if let go now; none where it can go nowhere. */
  mark = $state.raw<DropMark | undefined>(undefined);
  /** Where the pointer is while a node is dragged: its title follows it. */
  at = $state.raw<Point | undefined>(undefined);
  #root: HTMLElement | undefined;
  #unfolding: { pageId: string; timer: ReturnType<typeof setTimeout> } | undefined;
  readonly #scroller = new AutoScroller();
  #frame: number | undefined;

  constructor(source: TreeDragSource<P>) {
    this.#source = source;
  }

  /** The list of the tree, which scrolls. For `{@attach}`. */
  readonly attach = (root: HTMLElement): (() => void) => {
    this.#root = root;
    const handle = pointerDrag(root, this.#press);
    return () => {
      handle.destroy();
      this.#stop();
      this.#root = undefined;
    };
  };

  #stop(): void {
    if (this.#unfolding) clearTimeout(this.#unfolding.timer);
    this.#unfolding = undefined;
    if (this.#frame !== undefined) cancelAnimationFrame(this.#frame);
    this.#frame = undefined;
    this.#scroller.clear();
  }

  /** Unfolds the folded row held over, once it has been held long enough. */
  #holdOver(row: PageRow<P> | undefined): void {
    const pageId = row && row.hasChildren && !row.open ? row.page.id : undefined;
    if (this.#unfolding?.pageId === pageId) return;
    if (this.#unfolding) clearTimeout(this.#unfolding.timer);
    this.#unfolding = undefined;
    if (pageId === undefined) return;
    const timer = setTimeout(() => {
      this.#unfolding = undefined;
      this.#source.unfold(pageId);
    }, UNFOLD_AFTER);
    this.#unfolding = { pageId, timer };
  }

  /** Where the dragged node would go over `point`; `moving` is it and its descendants. */
  #markAt(dragged: string, moving: ReadonlySet<string>, point: Point): DropMark | undefined {
    const root = this.#root;
    const element = root && rowAt(root, point);
    const rows = this.#source.rows();
    const pages = this.#source.pages();
    const row = rows.find((one) => one.page.id === element?.dataset.page);
    const draggedItem = pages.find((one) => one.id === dragged);
    if (
      !element ||
      !row ||
      !draggedItem ||
      moving.has(row.page.id) ||
      row.page.parentId !== (row.parentId ?? '')
    ) {
      this.#holdOver(undefined);
      return undefined;
    }
    const rules = this.#source.rules?.();
    const canGoUnder = (parentId: string): boolean => {
      const parent = parentId === '' ? undefined : pages.find((one) => one.id === parentId);
      return allowsUnder(pages, draggedItem, parent, rules);
    };
    const canInside = canGoUnder(row.page.id);
    const box = element.getBoundingClientRect();
    const share = box.height > 0 ? (point.y - box.top) / box.height : 0.5;
    const zone = canInside
      ? share < 0.25
        ? 'before'
        : share > 0.75
          ? 'after'
          : 'inside'
      : share < 0.5
        ? 'before'
        : 'after';
    this.#holdOver(zone === 'inside' && canInside ? row : undefined);
    const mark = { pageId: row.page.id, zone } as const;
    if (zone === 'before') {
      if (!canGoUnder(row.parentId ?? '')) return undefined;
      return { ...mark, depth: row.depth, to: before(pages, row, dragged) };
    }
    if (zone === 'inside') {
      return { ...mark, depth: row.depth + 1, to: { parentId: row.page.id } };
    }
    if (row.open && canInside) {
      return { ...mark, depth: row.depth + 1, to: { parentId: row.page.id, after: '' } };
    }
    const levels = levelsAfter(rows, row).filter((level) => canGoUnder(level.parentId ?? ''));
    if (levels.length === 0) return undefined;
    const level = Math.floor((point.x - box.left - 4) / LEVEL_WIDTH);
    const up = Math.min(levels.length - 1, Math.max(0, row.depth - level));
    const target = levels[up] ?? levels[0];
    if (!target) return undefined;
    // When placing after an open node whose children stay visible (e.g. a
    // chapter dragged past an open chapter), draw the drop line after the
    // subtree's last visible row so it matches where the node will land.
    let anchor = row.page.id;
    if (row.open && row.hasChildren && target.page.id === row.page.id) {
      const start = rows.indexOf(row);
      for (let i = start + 1; i < rows.length; i += 1) {
        const next = rows[i];
        if (!next || next.depth <= row.depth) break;
        anchor = next.page.id;
      }
    }
    return {
      pageId: anchor,
      zone: 'after',
      depth: target.depth,
      to: { parentId: target.parentId ?? '', after: target.page.id },
    };
  }

  /** Scrolls the tree with the pointer near an edge, a frame at a time, while dragging. */
  #scroll(dragged: string, moving: ReadonlySet<string>): void {
    let last = performance.now();
    const frame = (now: number): void => {
      const root = this.#root;
      const at = this.at;
      if (!root || !at || this.dragged !== dragged) return;
      const elapsed = Math.min(now - last, DRAG_SETTINGS.longestFrame);
      last = now;
      if (this.#scroller.step(at, [{ element: root, axis: 'y' }], elapsed)) {
        this.mark = this.#markAt(dragged, moving, at);
      }
      this.#frame = requestAnimationFrame(frame);
    };
    this.#frame = requestAnimationFrame(frame);
  }

  readonly #press = (event: PointerEvent): PointerDragSession | undefined => {
    if (!this.#source.enabled()) return undefined;
    const target = event.target instanceof Element ? event.target : null;
    // The buttons of a row (its fold, its menu) keep their presses; chapter toggle buttons can still be dragged by the row label when marked.
    if (!target || target.closest('button:not([data-drag-handle])')) return undefined;
    const pageId = target.closest<HTMLElement>('[data-page]')?.dataset.page;
    if (pageId === undefined) return undefined;
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    let moving: ReadonlySet<string> = new Set([pageId]);
    const show = (point: Point): void => {
      this.at = point;
      this.mark = this.#markAt(pageId, moving, point);
    };
    const end = (): void => {
      this.#stop();
      [this.dragged, this.mark, this.at] = [undefined, undefined, undefined];
    };
    return {
      start: (point) => {
        moving = withDescendants(this.#source.pages(), pageId);
        this.dragged = pageId;
        show(point);
        this.#scroll(pageId, moving);
      },
      move: show,
      drop: (point) => {
        show(point);
        const to = this.mark?.to;
        end();
        if (to) this.#source.ondrop(pageId, to);
      },
      cancel: end,
    };
  };
}
