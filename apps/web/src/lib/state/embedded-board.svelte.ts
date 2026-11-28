/**
 * The boards that the board views of the bodies on screen show
 * (board-view.ts; src/app/EmbeddedView.svelte draws one): one store for each
 * board, however many views of it are shown, and apart from the board the
 * reader has open and from the boards the workspace keeps to go back to
 * (`MAX_CACHED_BOARDS`). A body that shows views of five boards puts none of
 * those out of that cache, and opening one of the boards does not take the
 * store of its views away.
 *
 * A store is loaded when the first view of its board is held (EmbeddedView
 * holds it once it comes into sight), follows the board's changes as an open
 * board does (`watchContainer`: a card changed by anyone, the view's filter
 * changed on the board), and is closed `RELEASE_AFTER_MS` after the last view
 * of it is let go. A body drawn anew (an editor made again, a body read and
 * then edited) holds it again meanwhile, and nothing is loaded twice; but a
 * store whose reading failed (the network, the server) is read again when a
 * view of it is held again, while one of a board that is gone is not.
 *
 * What draws and puts in a board view (`host`) is the app's: it is set for the
 * workspace by src/app/embedded/BoardViewHost.svelte, and read by the bodies that may
 * show board views (a card's, a page's, a unit's).
 */
import type { BoardViewHost } from '../editor/board-view-read.ts';
import type { BoardView, Container } from '../logic/wire.ts';

import { BoardStore } from './board.svelte.ts';
import { orderViews } from './order.ts';
import type { Workspace } from './workspace.svelte.ts';

/** How long a store is kept after the last view of its board is let go. */
export const RELEASE_AFTER_MS = 30_000;

/** A store, who holds it, and the closing that waits once nobody does. */
interface Held {
  readonly store: BoardStore;
  holders: number;
  closing: ReturnType<typeof setTimeout> | undefined;
}

/** A store held for one view: `release` lets it go, once. */
export interface HeldBoard {
  readonly store: BoardStore;
  release: () => void;
}

export class EmbeddedBoards {
  /** What draws and puts in the board views of the workspace's bodies; none until the app gives it. */
  host = $state.raw<BoardViewHost | undefined>(undefined);
  readonly #workspace: Workspace;
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- who holds what, never drawn: a store's own state is
  readonly #held = new Map<string, Held>();

  constructor(workspace: Workspace) {
    this.#workspace = workspace;
  }

  /** The store of the board `boardId`, loaded when it is first held. */
  hold(boardId: string): HeldBoard {
    let held = this.#held.get(boardId);
    if (!held) {
      const store = new BoardStore(this.#workspace, boardId);
      held = { store, holders: 0, closing: undefined };
      this.#held.set(boardId, held);
      void store.load();
    } else if (held.store.status === 'failed' && !held.store.missing) {
      void held.store.load();
    }
    clearTimeout(held.closing);
    held.closing = undefined;
    held.holders += 1;
    const it = held;
    let released = false;
    return {
      store: it.store,
      release: () => {
        if (released) return;
        released = true;
        it.holders -= 1;
        if (it.holders > 0) return;
        it.closing = setTimeout(() => {
          if (it.holders > 0 || this.#held.get(boardId) !== it) return;
          this.#held.delete(boardId);
          it.store.close();
        }, RELEASE_AFTER_MS);
      },
    };
  }

  /** The boards whose stores are kept now. */
  get boardIds(): string[] {
    return [...this.#held.keys()];
  }
}

const registries = new WeakMap<Workspace, EmbeddedBoards>();

/** The boards the board views of `workspace`'s bodies show. */
export function embeddedBoards(workspace: Workspace): EmbeddedBoards {
  let found = registries.get(workspace);
  if (!found) {
    found = new EmbeddedBoards(workspace);
    registries.set(workspace, found);
  }
  return found;
}

/** The boards of the workspace a view may be shown from: the reader's, in the order of the workspace. */
export function boardsToShow(workspace: Workspace): Container[] {
  return workspace.containers.filter(
    (container) => (container.kind ?? 'board') === 'board' && !container.isTemplate,
  );
}

/** The views of a board, in the board's order: to choose one to show. */
export async function viewsOfBoard(workspace: Workspace, boardId: string): Promise<BoardView[]> {
  return orderViews(await workspace.backend.views(boardId), workspace.locale);
}
