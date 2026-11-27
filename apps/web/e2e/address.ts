/**
 * The app's addresses, as the tests write and read them: the one place that
 * knows their shape (`src/app/router.svelte.ts` is the app's). The tests
 * against the real server use it too (`e2e-live/support.ts`).
 *
 * The world in memory has one workspace its people share, made first by the
 * seed, so its identifier is the seed's first; each reader's own space is
 * made after, when it is first asked for.
 */

/** The workspace the people of the world in memory share. */
export const WORKSPACE = '01930000-0000-7000-8000-000000000001';

const ID = '[0-9a-f-]{36}';

/** What an address names, from the left; an empty string for what it does not. */
export interface Place {
  workspaceId: string;
  boardId: string;
  viewId: string;
  cardId: string;
}

/** The address of a workspace, a board of it, a view, a card on that view: `/w/…/boards/…`. */
export function addressOf(place: Partial<Place> & { workspaceId: string }): string {
  const parts = [place.boardId, place.viewId, place.cardId].filter((part) => part);
  const encoded = parts.map((part) => `/${encodeURIComponent(part ?? '')}`).join('');
  return `/w/${encodeURIComponent(place.workspaceId)}${parts.length > 0 ? '/boards' : ''}${encoded}`;
}

/** The address of a board of the world in memory, a view of it, a card on that view. */
export function boardPath(boardId: string, viewId?: string, cardId?: string): string {
  return addressOf({ workspaceId: WORKSPACE, boardId, viewId, cardId });
}

/** The address of a view of a board, and nothing after it. */
export function viewAddress(boardId: string, viewId: string): RegExp {
  return new RegExp(`${boardPath(boardId, viewId)}$`);
}

/** Any view of any board: what the address settles on when a board is shown. */
export const ON_A_VIEW = new RegExp(`/w/${ID}/boards/${ID}/${ID}$`);
/** Any card open over a view. */
export const ON_A_CARD = new RegExp(`/w/${ID}/boards/${ID}/${ID}/[^/]+$`);
/** The workspace of the world, a board and a view of it, and maybe a card: an address settled. */
export const SETTLED = new RegExp(`/w/${WORKSPACE}/boards/${ID}/${ID}(/[^/]+)?$`);
/** Any board, with or without a view. */
export const ON_A_BOARD = new RegExp(`/w/${ID}/boards/${ID}(/${ID})?$`);

/** What an address of the app names. */
export function placeOf(address: string): Place {
  const { pathname } = new URL(address, 'http://here');
  const [, workspaceId = '', boardId = '', viewId = '', cardId = ''] =
    /\/w\/([^/]+)(?:\/boards\/([^/]+)(?:\/([^/]+)(?:\/([^/]+))?)?)?\/?$/.exec(pathname) ?? [];
  const decode = (part: string): string => decodeURIComponent(part);
  return {
    workspaceId: decode(workspaceId),
    boardId: decode(boardId),
    viewId: decode(viewId),
    cardId: decode(cardId),
  };
}

/** The view an address names. */
export function viewIdOf(address: string): string {
  return placeOf(address).viewId;
}
