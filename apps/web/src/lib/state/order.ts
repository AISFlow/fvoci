/**
 * The order things are shown in.
 *
 * No answer of the backend comes in an order to rely on, so every list a
 * screen shows is sorted here, and every comparison goes on until it has
 * told two different things apart (the identifier settles what is left).
 * The order of the cards of a view is the business of `logic/sort.ts`.
 */
import { isPlace, placeAfter, type Placed, type PlaceWrite } from '../logic/list-places.ts';
import {
  createBoardView,
  type Block,
  type Container,
  type ContainerMember,
  type BoardView,
} from '../logic/wire.ts';

import type { Category } from './backend.ts';

const byId = (first: { id: string }, second: { id: string }): number =>
  first.id < second.id ? -1 : first.id > second.id ? 1 : 0;

/** Oldest first; the identifier settles a tie. */
export function byCreation<T extends Pick<Block, 'id' | 'createdAt'>>(items: readonly T[]): T[] {
  return [...items].sort(
    (first, second) => first.createdAt - second.createdAt || byId(first, second),
  );
}

function textOrder(locale: string): (first: string, second: string) => number {
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
  return (first, second) => collator.compare(first, second);
}

/** By title in the reader's language, untitled ones last, then oldest first. */
export function byTitle<T extends Pick<Block, 'id' | 'createdAt' | 'title'>>(
  items: readonly T[],
  locale: string,
): T[] {
  const compare = textOrder(locale);
  return [...items].sort((first, second) => {
    if ((first.title === '') !== (second.title === '')) return first.title === '' ? 1 : -1;
    return (
      compare(first.title, second.title) ||
      first.createdAt - second.createdAt ||
      byId(first, second)
    );
  });
}

/**
 * The views of a board in the order of its list: those with a place
 * (`fields.order`, `logic/list-places.ts`) by it, the identifier settling a
 * tie, then those without one by title (every view made before views had
 * places, until somebody moves one).
 */
export function orderViews(views: readonly BoardView[], locale: string): BoardView[] {
  const placed = views.filter((view) => isPlace(view.fields.order));
  const place = (view: BoardView): string => view.fields.order ?? '';
  placed.sort((first, second) =>
    place(first) < place(second) ? -1 : place(first) > place(second) ? 1 : byId(first, second),
  );
  const loose = views.filter((view) => !isPlace(view.fields.order));
  return [...placed, ...byTitle(loose, locale)];
}

/** Views in the order of their list (`orderViews`), as `logic/list-places.ts` takes them. */
export function placesOf(ordered: readonly BoardView[]): Placed[] {
  return ordered.map((view) => ({
    id: view.id,
    place: isPlace(view.fields.order) ? view.fields.order : undefined,
  }));
}

/**
 * What a copy of `source` takes: its fields, with a place right after it, and
 * the places that keep the other views where they show (`Placing.keep`).
 */
export function viewCopy(
  source: BoardView,
  ordered: readonly BoardView[],
): { fields: BoardView['fields']; keep: PlaceWrite[] } {
  const { place, keep } = placeAfter(placesOf(ordered), source.id);
  return { fields: { ...createBoardView(source).fields, order: place }, keep };
}

/** The members of a board by the name they are shown under. */
export function orderMembers(
  members: readonly ContainerMember[],
  nameOf: (userId: string) => string,
  locale: string,
): ContainerMember[] {
  const compare = textOrder(locale);
  return [...members].sort(
    (first, second) =>
      compare(nameOf(first.userId), nameOf(second.userId)) ||
      (first.userId < second.userId ? -1 : first.userId > second.userId ? 1 : 0),
  );
}

export interface SidebarCategory {
  category: Category;
  containers: Container[];
}

/** Who reads the sidebar, for what of it waits to be joined (`memberships.svelte.ts`). */
export interface SidebarReader {
  id: string | undefined;
  /** A guest of the workspace, who joins nothing by themselves: what they were let into is theirs. */
  guest: boolean;
  /** The containers the reader is known to be a member of, directly or by a group. */
  memberOf: ReadonlySet<string>;
}

/** One about whom nothing is known: what others made open waits to be joined. */
const NOBODY: SidebarReader = { id: undefined, guest: false, memberOf: new Set() };

/**
 * Whether a container no category of the reader's files is one they neither
 * made nor joined: an open one of the workspace, which the server files once
 * it is joined. It waits at the foot of the sidebar (`openToJoinOf`). Not to a
 * guest, who cannot join it, nor where the reader is known to be a member.
 */
function waitsToBeJoined(container: Container, reader: SidebarReader): boolean {
  return (
    !reader.guest &&
    container.visibility === 'open' &&
    !container.isTemplate &&
    container.createdBy !== reader.id &&
    !reader.memberOf.has(container.id)
  );
}

/**
 * The sidebar: the categories in the reader's order, each with its boards in
 * the reader's order. A board that no category names and that the reader made
 * (it was just made, and is not filed yet) or was given goes to the end of the
 * default category, by title; an open one they have not joined is not here
 * (`openToJoinOf`). A category naming a board the reader cannot see simply
 * does not show it.
 */
export function sidebarOf(
  categories: readonly Category[],
  containers: readonly Container[],
  locale: string,
  reader: SidebarReader = NOBODY,
): SidebarCategory[] {
  const compare = textOrder(locale);
  const ordered = [...categories].sort(
    (first, second) =>
      first.sortOrder - second.sortOrder || compare(first.name, second.name) || byId(first, second),
  );
  const known = new Map(containers.map((board) => [board.id, board]));
  const filed = new Set<string>();
  const result = ordered.map((category) => {
    const own: Container[] = [];
    for (const boardId of category.containerIds) {
      const board = known.get(boardId);
      if (!board || filed.has(boardId) || category.hiddenContainerIds.includes(boardId)) continue;
      filed.add(boardId);
      own.push(board);
    }
    return { category, containers: own };
  });
  const hidden = new Set(categories.flatMap((category) => category.hiddenContainerIds));
  const loose = byTitle(
    containers.filter(
      (board) => !filed.has(board.id) && !hidden.has(board.id) && !waitsToBeJoined(board, reader),
    ),
    locale,
  );
  const fallback = result.find((entry) => entry.category.type === 'system') ?? result.at(-1);
  fallback?.containers.push(...loose);
  return result;
}

/** The open containers of the workspace the reader neither made nor joined, by title. */
export function openToJoinOf(
  categories: readonly Category[],
  containers: readonly Container[],
  locale: string,
  reader: SidebarReader,
): Container[] {
  const filed = new Set(
    categories.flatMap((category) => [...category.containerIds, ...category.hiddenContainerIds]),
  );
  return byTitle(
    containers.filter((board) => !filed.has(board.id) && waitsToBeJoined(board, reader)),
    locale,
  );
}

/** The boards a category hides that the sidebar does not show elsewhere, by title. */
export function hiddenContainersOf(
  categories: readonly Category[],
  containers: readonly Container[],
  sidebar: readonly SidebarCategory[],
  locale: string,
): Container[] {
  const hidden = new Set(categories.flatMap((category) => category.hiddenContainerIds));
  const shown = new Set(sidebar.flatMap((entry) => entry.containers.map((board) => board.id)));
  return byTitle(
    containers.filter((board) => hidden.has(board.id) && !shown.has(board.id)),
    locale,
  );
}

/** The categories without a board that is gone: it is filed and hidden nowhere. */
export function withoutContainer(categories: readonly Category[], containerId: string): Category[] {
  return categories.map((category) => ({
    ...category,
    containerIds: category.containerIds.filter((id) => id !== containerId),
    hiddenContainerIds: category.hiddenContainerIds.filter((id) => id !== containerId),
  }));
}
