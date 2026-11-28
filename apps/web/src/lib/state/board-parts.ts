/**
 * What the board store is built on besides the backend: a board of the
 * workspace shown, a member's effective role, adding a member,
 * the place of a drop, the card a copy is made as, the cards a view shows.
 */
import { filteredCardsOfView, narrowCardsBySearch } from '../logic/card-list.ts';
import { getVisibleAndHiddenGroups, type BoardGroups, type NoValueLabel } from '../logic/group.ts';
import { propertyBehavior } from '../logic/properties.ts';
import { applyQuickFilters } from '../logic/quick-filter.ts';
import type { Weekday } from '../logic/regional.ts';
import {
  viewGrouping,
  viewQuery,
  type BoardView,
  type Container,
  type ContainerKind,
  type ContainerMember,
  type Card,
  type MemberRole,
  type PropertyTemplate,
  type PropertyValue,
} from '../logic/wire.ts';

import type { Backend, CardSummary } from './backend.ts';
import { quickFilters } from './board-quick.svelte.ts';
import { DeviceToday } from './device-today.svelte.ts';
import { BackendError } from './problems.ts';

/**
 * Reads a board of the workspace `workspaceId`, the one the address names,
 * of the `kind` the address says (a wiki at a board's address, or a board at
 * a wiki's). A board of another workspace or kind is not found there (404),
 * as one that is not there at all: the address does not say where else it is.
 */
export async function containerOfWorkspace(
  backend: Backend,
  containerId: string,
  workspaceId: string | undefined,
  kind: ContainerKind = 'board',
): Promise<Container> {
  const board = await backend.container(containerId);
  if (board.workspaceId !== workspaceId) {
    throw new BackendError(404, { detail: 'the board is of another workspace' });
  }
  if ((board.kind ?? 'board') !== kind) {
    throw new BackendError(404, { detail: `the board is no ${kind}` });
  }
  return board;
}

const ROLE_RANK: Record<Exclude<MemberRole, ''>, number> = {
  viewer: 1,
  commenter: 2,
  editor: 3,
  admin: 4,
};

export function roleOf(member: ContainerMember | undefined): MemberRole {
  if (!member) return '';
  if (!member.inheritedRole) return member.role;
  return ROLE_RANK[member.inheritedRole] > ROLE_RANK[member.role]
    ? member.inheritedRole
    : member.role;
}

/**
 * Whether a reader of `role` changes, deletes and brings back a comment that
 * `createdBy` wrote: their own when they may comment, anyone's when they are
 * an admin (the user's decision of 2026-11-11 22:13 KST; `patchPageComment`
 * in `contracts/openapi.json`), as `BoardStore.canEditComment` has it of a board.
 */
export function canChangeComment(
  role: MemberRole,
  createdBy: string,
  readerId: string | undefined,
): boolean {
  if (role === 'admin') return true;
  return (role === 'editor' || role === 'commenter') && createdBy === readerId;
}

/** A container with its workspace visibility set to `'open'` or `'private'`. */
export function withVisibility(container: Container, open: boolean): Container {
  return { ...container, visibility: open ? 'open' : 'private' };
}

/** A card with one property set to `value`, or cleared when empty. */
export function withCardProperty(
  card: CardSummary,
  propertyId: string,
  value: PropertyValue | undefined,
): Card {
  const empty = value === undefined || value === '' || (Array.isArray(value) && !value.length);
  const properties = empty
    ? Object.fromEntries(Object.entries(card.fields.properties).filter(([id]) => id !== propertyId))
    : { ...card.fields.properties, [propertyId]: value };
  return { ...card, fields: { ...card.fields, properties } };
}

/** Where dropped cards go: a column (by the value of the grouping property) and a place in it. */
export interface CardDrop {
  viewId: string;
  cardIds: readonly string[];
  /**
   * The option of the column they were dropped in; `''` is the column of
   * cards without a value. Undefined: the view is not grouped, or the column
   * is not to change.
   */
  optionId?: string;
  /**
   * The place among the cards of that column, counted without the dropped
   * ones. Undefined: the view is sorted and places itself.
   */
  index?: number;
  /**
   * The cards of that column the page shows, in shown order, when a filter,
   * a search or a folded group may hide some of them: `index` is then a
   * place among these, and the store finds the place among all of the
   * column (`placeAmongAll`). Left out: `index` counts every card.
   */
  shown?: readonly string[];
}

/** The property a view groups its cards by, falling back to the first groupable property on boards. */
export function groupingPropertyOf(
  properties: readonly PropertyTemplate[],
  view: BoardView,
): PropertyTemplate | undefined {
  const named = properties.find((template) => template.id === view.fields.groupBy);
  if (view.fields.viewType !== 'board') return named;
  if (named && propertyBehavior(named.type).canGroup) return named;
  return properties.find((template) => propertyBehavior(template.type).canGroup);
}

/** The columns of a view, visible and hidden, for cards already filtered and sorted. */
export function groupsOfView(
  properties: readonly PropertyTemplate[],
  view: BoardView,
  cards: readonly Card[],
  noValueLabel: NoValueLabel,
): BoardGroups {
  const grouping = viewGrouping(view);
  return getVisibleAndHiddenGroups(
    cards,
    grouping.visibleGroups,
    grouping.hiddenGroups,
    groupingPropertyOf(properties, view),
    noValueLabel,
  );
}

/** What a board view needs from a board store to filter and order its cards. */
export interface BoardCardsSource {
  readonly boardId: string;
  /** Counts the visits to the board (board-quick.svelte.ts). */
  readonly visit?: number;
  readonly realCards: readonly CardSummary[];
  readonly board: Container | undefined;
  readonly cardsById: ReadonlyMap<string, CardSummary>;
  readonly workspace: {
    userName(userId: string): string | undefined;
    readonly me?: { readonly id: string } | undefined;
    readonly locale: string;
    readonly regional: { readonly timeZone: string; readonly weekStart: Weekday };
  };
}

type Reader = BoardCardsSource['workspace'];

/** Today on the device of each reader, read again at their midnight. */
const todays = new WeakMap<Reader, DeviceToday>();

function todayOf(workspace: Reader): DeviceToday {
  let today = todays.get(workspace);
  if (!today) {
    today = new DeviceToday(() => workspace.regional.timeZone);
    todays.set(workspace, today);
  }
  return today;
}

/**
 * Stage 1: cards of a view filtered by the view's filter and sorted, then
 * narrowed by the reader's quick filters (lib/logic/quick-filter.ts),
 * independent of search text (§1.2.4). A card the reader made while the
 * chips are on stays whatever they are (board-quick.svelte.ts). Today is the
 * device's, so that the periods and "Overdue" move on at midnight.
 */
export function filteredCardsOfBoardView(source: BoardCardsSource, view: BoardView): Card[] {
  const { workspace, board } = source;
  const meId = workspace.me?.id;
  const context = {
    usernameOf: (userId: string) => workspace.userName(userId),
    locale: workspace.locale,
    timeZone: workspace.regional.timeZone,
    weekStart: workspace.regional.weekStart,
    today: todayOf(workspace).date,
    meId,
    doneWhen: board?.doneWhen,
    cardTitle: (cardId: string) => source.cardsById.get(cardId)?.title,
  };
  const filtered = filteredCardsOfView(
    source.realCards,
    board?.cardProperties ?? [],
    viewQuery(view),
    context,
  );
  // A public link's reader has an empty identifier: nobody is signed in to own a card or make one.
  const reader = meId || undefined;
  return applyQuickFilters(
    filtered,
    quickFilters.chosen(source.boardId, reader),
    board,
    { ...context, meId: reader },
    quickFilters.made(source, reader),
  );
}

/** Stage 2: narrows `filtered` cards by `searchText` (§1.2.4). */
export function narrowBoardCards(
  source: Pick<BoardCardsSource, 'board'>,
  filtered: readonly Card[],
  searchText: string,
): Card[] {
  return searchText
    ? narrowCardsBySearch(filtered, source.board?.cardProperties ?? [], searchText)
    : (filtered as Card[]);
}

/** The cards a view shows, in its order, for a search text: both stages. */
export function cardsOfBoardView(
  source: BoardCardsSource,
  view: BoardView,
  searchText = '',
): Card[] {
  return narrowBoardCards(source, filteredCardsOfBoardView(source, view), searchText);
}
