/**
 * Creating, moving, and duplicating board views using single-transaction
 * server fractional-indexing endpoints (`writeView`, `moveView`, `duplicateView`).
 *
 * While a move is shown and not yet answered, what the backend says of the
 * board's views waits (`receiveView`): the echo of an earlier move, coming
 * after a later one was shown, would put the views back for a moment, and
 * a list put back moves its lines, and the focus on the line moved with
 * them is lost. Once every move is answered, what waited is taken, where it
 * is newer than what the answers gave.
 */
import { movedTo } from '../logic/list-places.ts';
import type { BoardView } from '../logic/wire.ts';

import { isBackendError, type BoardViewWriteResult, type ViewDraft } from './backend.ts';
import { deleteBlockForUndo, performBoardAction } from './board-sync.ts';
import type { BoardStore } from './board.svelte.ts';
import { placesOf } from './order.ts';
import { put, take } from './write.ts';

interface MoveTurnQueue {
  tail: Promise<unknown>;
  inFlight: number;
  /** Moves shown whose turn has not come yet (`moveView`, before its write is queued). */
  shown: number;
  /** What the backend said of views while moves were under way, by identifier. */
  readonly held: Map<string, BoardView>;
}

const moveQueues = new WeakMap<BoardStore, MoveTurnQueue>();

function queueOf(board: BoardStore): MoveTurnQueue {
  let queue = moveQueues.get(board);
  if (!queue) {
    queue = { tail: Promise.resolve(), inFlight: 0, shown: 0, held: new Map() };
    moveQueues.set(board, queue);
  }
  return queue;
}

/** What waited while moves were under way, taken once none is: of the views still shown. */
function release(board: BoardStore, queue: MoveTurnQueue): void {
  if (queue.inFlight > 0 || queue.shown > 0 || queue.held.size === 0) return;
  let next = board.views;
  for (const view of queue.held.values()) {
    if (next.some((shown) => shown.id === view.id)) next = take(next, view);
  }
  queue.held.clear();
  board.views = next;
}

/**
 * A version of a view from the backend: shown unless a write of it is under
 * way (`writes`) or, for a view already shown, a move of the board's views
 * is, when it waits. A view not shown yet (a new one) comes at once.
 */
export function receiveView(
  board: BoardStore,
  writes: { receive(view: BoardView): boolean },
  view: BoardView,
): void {
  if (!writes.receive(view)) return;
  const queue = moveQueues.get(board);
  const moving = queue !== undefined && (queue.inFlight > 0 || queue.shown > 0);
  if (!moving || !board.views.some((shown) => shown.id === view.id)) {
    board.views = take(board.views, view);
    return;
  }
  const waiting = queue.held.get(view.id);
  if (!waiting || waiting.updatedAt <= view.updatedAt) queue.held.set(view.id, view);
}

function takeViewWrite(board: BoardStore, written: BoardViewWriteResult): BoardView {
  let next = board.views;
  for (const companion of written.companionViews) next = take(next, companion);
  next = take(next, written.view);
  board.views = next;
  return written.view;
}

function applyOptimisticMove(board: BoardStore, viewId: string, step: -1 | 1): void {
  const shown = board.orderedViews;
  const from = shown.findIndex((view) => view.id === viewId);
  const to = from + step;
  if (from < 0 || to < 0 || to >= shown.length) return;
  const { place, keep } = movedTo(placesOf(shown), viewId, to);
  let next = board.views;
  for (const item of keep) {
    const cur = next.find((v) => v.id === item.id);
    if (cur && cur.fields.order !== item.place) {
      next = put(next, { ...cur, fields: { ...cur.fields, order: item.place } });
    }
  }
  const target = next.find((v) => v.id === viewId);
  if (target) next = put(next, { ...target, fields: { ...target.fields, order: place } });
  board.views = next;
}

async function runQueuedMove(board: BoardStore, viewId: string, after: string): Promise<void> {
  const queue = queueOf(board);
  queue.inFlight += 1;
  const turn = queue.tail.then(async () => {
    try {
      const written = await board.workspace.backend.moveView(board.boardId, viewId, after);
      if (queue.inFlight === 1) takeViewWrite(board, written);
    } catch (error) {
      if (queue.inFlight === 1) {
        await board.rereadViews().catch(() => undefined);
      }
      throw error;
    } finally {
      queue.inFlight -= 1;
      release(board, queue);
    }
  });
  queue.tail = turn.catch(() => undefined);
  await turn;
}

/**
 * Moves a view one slot up (`-1`) or down (`1`) in `board.orderedViews` via
 * `POST /boards/{boardId}/views/{viewId}/move`, showing the new order
 * synchronously, serializing overlapping moves per board, and re-reading on refusal.
 */
export async function moveView(board: BoardStore, viewId: string, step: -1 | 1): Promise<boolean> {
  const shown = board.orderedViews;
  const from = shown.findIndex((view) => view.id === viewId);
  const to = from + step;
  if (from < 0 || to < 0 || to >= shown.length) return false;

  const without = shown.filter((view) => view.id !== viewId);
  const forwardAfter = to === 0 ? '' : (without[to - 1]?.id ?? '');
  const backwardAfter = from === 0 ? '' : (without[from - 1]?.id ?? '');

  const queue = queueOf(board);
  // Shown now and queued maybe later (after a join): what the backend says waits from now on.
  queue.shown += 1;
  let shownWaits = true;
  const queued = (): void => {
    if (shownWaits) queue.shown -= 1;
    shownWaits = false;
  };
  applyOptimisticMove(board, viewId, step);
  let firstCall = true;
  let ok = false;
  try {
    await performBoardAction(
      board,
      async () => {
        if (!firstCall) applyOptimisticMove(board, viewId, step);
        firstCall = false;
        // Counted in flight as it is called, before it waits for its turn.
        const turn = runQueuedMove(board, viewId, forwardAfter);
        queued();
        await turn;
        ok = true;
      },
      async () => {
        applyOptimisticMove(board, viewId, -step as -1 | 1);
        await runQueuedMove(board, viewId, backwardAfter);
      },
      'moveView',
    );
  } finally {
    queued();
    release(board, queue);
  }
  return ok;
}

/** Adds a view at the end of `board.orderedViews` via `POST /boards/{boardId}/views`. */
export function addViewAtEnd(board: BoardStore, draft: ViewDraft): Promise<BoardView | undefined> {
  return performBoardAction(
    board,
    async () => {
      const written = await board.workspace.backend.writeView(board.boardId, {
        ...draft,
        after: null,
      });
      return takeViewWrite(board, written);
    },
    async (created) => {
      await deleteBlockForUndo(board, created.id);
      board.views = board.views.filter((existing) => existing.id !== created.id);
    },
    'createView',
  );
}

/** Duplicates `viewId` under `title` right after `viewId` via `POST .../views/{viewId}/duplicate`. */
export function duplicateView(
  board: BoardStore,
  viewId: string,
  title: string,
): Promise<BoardView | undefined> {
  if (!board.view(viewId)) return Promise.resolve(undefined);
  return performBoardAction(
    board,
    async () => {
      const written = await board.workspace.backend.duplicateView(
        board.boardId,
        viewId,
        title,
        viewId,
      );
      return takeViewWrite(board, written);
    },
    async (copied) => {
      try {
        await deleteBlockForUndo(board, copied.id);
      } catch (error) {
        if (!isBackendError(error) || error.status !== 404) throw error;
      }
      board.views = board.views.filter((existing) => existing.id !== copied.id);
    },
    'createView',
  );
}

/** Optimistically applies a single view replacement for callers holding a `BoardView`. */
export function showBoardView(board: BoardStore, view: BoardView): void {
  board.views = put(board.views, view);
}
