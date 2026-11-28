/**
 * The place of a board view where a body is only read (render.ts,
 * BodyView.svelte), and what the app gives to draw the view into it: an
 * element `div[data-board-view]` holding the view's identifier
 * (`data-view-id`). The editor draws the same place (board-view.ts). This
 * module imports nothing of the editor: a body read stays light.
 *
 * What is drawn into the place comes from `BoardViewHost.mount`, which the
 * app gives, so that nothing here knows of boards or of their state. Without
 * it, and for a board view that names no view (one emptied for a reader by
 * link), the place says that the view cannot be seen.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

import { isBlockId } from './schema-rules.ts';

export const BOARD_VIEW_ATTRIBUTE = 'data-board-view';
export const VIEW_ID_ATTRIBUTE = 'data-view-id';

/** Draws the view `viewId` into `place`, and gives back what stops drawing it. */
export type MountBoardView = (place: HTMLElement, viewId: string) => () => void;

/** What the app gives a body that may show board views: how to draw one, and how to put one in. */
export interface BoardViewHost {
  mount: MountBoardView;
  /** Asks the reader for a view (a board, then one of its views); nothing if they chose none. */
  choose: () => Promise<string | undefined>;
  /** The view an address names, if it is the address of a view of the workspace. */
  viewAt: (address: string) => string | undefined;
  /** Whether the reader may see the view. */
  seen: (viewId: string) => Promise<boolean>;
}

/** The view a board view names, or nothing for one that names none. */
function viewIdOf(value: unknown): string | undefined {
  return typeof value === 'string' && isBlockId(value) ? value : undefined;
}

/** Says, in `place`, that the view cannot be seen. */
function drawUnseen(place: HTMLElement): () => void {
  const said = place.ownerDocument.createElement('p');
  said.className = 'doc-board-view-unseen';
  said.textContent = m.board_view_unseen();
  place.replaceChildren(said);
  return () => {
    said.remove();
  };
}

/** Draws the view into its place, or says it cannot be seen; gives back what stops it. */
export function drawView(place: HTMLElement, viewId: unknown, mount: MountBoardView | undefined) {
  const id = viewIdOf(viewId);
  return id && mount ? mount(place, id) : drawUnseen(place);
}

/**
 * Draws the views of the board views under `root`, a body drawn for reading
 * (render.ts), and gives back what stops drawing them. `_placed` are what was
 * placed in `root`, so that an attachment draws again when they change.
 */
export function mountBoardViews(
  root: HTMLElement,
  mount: MountBoardView | undefined,
  ..._placed: unknown[]
): () => void {
  const stops = [...root.querySelectorAll<HTMLElement>(`div[${BOARD_VIEW_ATTRIBUTE}]`)].map(
    (place) => drawView(place, place.getAttribute(VIEW_ID_ATTRIBUTE), mount),
  );
  return () => {
    for (const stop of stops) stop();
  };
}

/** The place of a board view drawn for reading, as the editor draws it. */
export function drawBoardViewPlace(page: Document, viewId: unknown): HTMLElement {
  const place = page.createElement('div');
  place.className = 'doc-board-view';
  place.setAttribute(BOARD_VIEW_ATTRIBUTE, '');
  place.setAttribute(VIEW_ID_ATTRIBUTE, typeof viewId === 'string' ? viewId : '');
  return place;
}
