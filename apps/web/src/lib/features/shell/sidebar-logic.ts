/**
 * What the sidebar works out from the reader's categories: which boards a
 * search leaves, the order of the categories after one moved a step, and
 * where in a category a dropped board is filed.
 */
import type { Destination, ListOrder } from '$lib/logic/drop-target.ts';
import { foldCase } from '$lib/logic/text.ts';
import type { Category, SidebarCategory } from '$lib/state/index.ts';

/** Whether a title has what was typed into the search of the sidebar, whatever the case. */
export function titleHas(title: string, query: string): boolean {
  return foldCase(title).includes(foldCase(query.trim()));
}

/**
 * The sidebar with only the boards whose title has `query` in it, whatever
 * the case. A category that is left without a board is left out. An empty
 * query (or one of spaces) changes nothing.
 */
export function filterSidebar(
  sidebar: readonly SidebarCategory[],
  query: string,
): readonly SidebarCategory[] {
  const wanted = foldCase(query.trim());
  if (wanted === '') return sidebar;
  return sidebar
    .map((entry) => ({
      category: entry.category,
      containers: entry.containers.filter((container) => titleHas(container.title, query)),
    }))
    .filter((entry) => entry.containers.length > 0);
}

/**
 * The order of the categories after `categoryId` moved one place up (`-1`)
 * or down (`1`). Undefined when it is already at that end, or is not one of
 * them.
 */
export function categoryOrderAfterStep(
  categoryIds: readonly string[],
  categoryId: string,
  step: -1 | 1,
): string[] | undefined {
  const from = categoryIds.indexOf(categoryId);
  const to = from + step;
  if (from === -1 || to < 0 || to >= categoryIds.length) return undefined;
  const next = [...categoryIds];
  next.splice(from, 1);
  next.splice(to, 0, categoryId);
  return next;
}

/** The lists a board can be dragged within and between: the categories that are open. */
export function dragOrder(sidebar: readonly SidebarCategory[]): ListOrder[] {
  return sidebar
    .filter((entry) => !entry.category.collapsed)
    .map((entry) => ({
      id: entry.category.id,
      items: entry.containers.map((container) => container.id),
    }));
}

/** The categories a board can only be dropped onto, not placed in: the folded ones. */
export function dragZones(sidebar: readonly SidebarCategory[]): string[] {
  return sidebar.filter((entry) => entry.category.collapsed).map((entry) => entry.category.id);
}

/**
 * Where a board goes in the list a category keeps (`containerIds`) when it is
 * put at `index` among the boards the category shows, both counted without
 * the board itself.
 *
 * The two lists are not the same: a category may name boards that are not
 * shown (deleted, hidden, no longer shared with the reader), and the default
 * category also shows boards that nobody filed. So the place is given by the
 * board that will follow: the dropped board goes right before it. Before a
 * board the category does not name, or after the last one, it goes to the end.
 */
export function filedIndex(
  category: Pick<Category, 'containerIds'>,
  shown: readonly string[],
  containerId: string,
  index: number,
): number {
  const filed = category.containerIds.filter((id) => id !== containerId);
  const following = shown.filter((id) => id !== containerId)[index];
  const at = following === undefined ? -1 : filed.indexOf(following);
  return at === -1 ? filed.length : at;
}

export interface ContainerMove {
  containerId: string;
  categoryId: string;
  /** The place in the category's own list, as `Workspace.moveContainer` takes it. */
  index: number;
}

/**
 * What a drop of a board means: into an open category at a place, or onto a
 * folded one, where it goes last. Undefined for a category that is gone.
 */
export function containerMoveOf(
  to: Destination,
  containerId: string,
  sidebar: readonly SidebarCategory[],
): ContainerMove | undefined {
  if (to.kind === 'zone') return moveToEnd(to.zoneId, containerId, sidebar);
  if (to.kind === 'list') return moveToEnd(to.listId, containerId, sidebar);
  const entry = sidebar.find((candidate) => candidate.category.id === to.listId);
  if (!entry) return undefined;
  const shown = entry.containers.map((container) => container.id);
  return {
    containerId,
    categoryId: to.listId,
    index: filedIndex(entry.category, shown, containerId, to.index),
  };
}

/** Files a board last in a category: what the menu's "Move to" and a drop on a folded category do. */
export function moveToEnd(
  categoryId: string,
  containerId: string,
  sidebar: readonly SidebarCategory[],
): ContainerMove | undefined {
  const entry = sidebar.find((candidate) => candidate.category.id === categoryId);
  if (!entry) return undefined;
  return {
    containerId,
    categoryId,
    index: entry.category.containerIds.filter((id) => id !== containerId).length,
  };
}
