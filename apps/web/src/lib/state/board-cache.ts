/**
 * Bounded LRU cache of recent `BoardStore` instances (`MAX_CACHED_BOARDS`),
 * scoped to one signed-in user and workspace. Switching between boards or
 * between a board and a wiki resumes the cached store immediately and refreshes
 * in the background, while parked stores hold no active listeners, open cards,
 * doc rooms, or timers.
 */
import { isOwnSpace, type WorkspaceInfo } from './backend.ts';

/** How many `BoardStore` instances stay alive in memory at once. */
export const MAX_CACHED_BOARDS = 2;

/** What the cache needs of a board store, without a circular runtime import. */
export interface CacheableBoard {
  readonly boardId: string;
  readonly status: string;
  readonly missing: boolean;
  readonly joinable: boolean;
  readonly role: string;
  readonly evicted: boolean;
  isClosed(): boolean;
  markEvicted(): void;
  close(): void;
}

/** Whether `board` is ready, accessible, non-evicted, open, and has an active membership role. */
export function isCacheableBoard(board: CacheableBoard): boolean {
  return (
    board.status === 'ready' &&
    !board.missing &&
    !board.joinable &&
    board.role !== '' &&
    !board.isClosed() &&
    !board.evicted
  );
}

interface Entry<B extends CacheableBoard> {
  readonly userId: string;
  readonly workspaceId: string;
  readonly boardId: string;
  readonly store: B;
}

export class BoardCache<B extends CacheableBoard = CacheableBoard> {
  readonly #max: number;
  readonly #entries = new Map<string, Entry<B>>();
  #ownerId: string | undefined;

  constructor(max = MAX_CACHED_BOARDS) {
    this.#max = Math.max(1, max);
  }

  get size(): number {
    return this.#entries.size;
  }

  has(boardId: string): boolean {
    return this.#entries.has(boardId);
  }

  /**
   * Returns a reusable cached board for `(userId, workspaceId, boardId)` and
   * promotes it to most-recently-used. Mismatched user clears everything;
   * mismatched workspace or non-ready store evicts and closes that entry.
   */
  take(userId: string, workspaceId: string, boardId: string): B | undefined {
    if (this.#ownerId !== undefined && this.#ownerId !== userId) {
      this.clear();
      return undefined;
    }
    const entry = this.#entries.get(boardId);
    if (!entry) return undefined;
    if (entry.userId !== userId) {
      this.clear();
      return undefined;
    }
    if (entry.workspaceId !== workspaceId || !isCacheableBoard(entry.store)) {
      this.#evict(boardId);
      return undefined;
    }
    this.#entries.delete(boardId);
    this.#entries.set(boardId, entry);
    return entry.store;
  }

  /**
   * Tracks `store` as most-recently-used, evicting and closing the oldest
   * inactive entries when the cache exceeds its cap.
   */
  track(userId: string, workspaceId: string, store: B): void {
    if (this.#ownerId !== undefined && this.#ownerId !== userId) {
      this.clear();
    }
    this.#ownerId = userId;
    const existing = this.#entries.get(store.boardId);
    if (existing && existing.store !== store) existing.store.close();
    this.#entries.delete(store.boardId);
    this.#entries.set(store.boardId, {
      userId,
      workspaceId,
      boardId: store.boardId,
      store,
    });
    while (this.#entries.size > this.#max) {
      const oldest = [...this.#entries.keys()].find((id) => id !== store.boardId);
      if (oldest === undefined) break;
      this.#evict(oldest);
    }
  }

  /**
   * Removes `boardId` from the cache. When `skip` matches the stored instance
   * (the currently active board), it is marked evicted without calling `close()`
   * so it stays live for the current screen but is closed rather than re-cached on leave.
   */
  delete(boardId: string, skip?: B): void {
    const entry = this.#entries.get(boardId);
    if (!entry) {
      if (skip?.boardId === boardId) skip.markEvicted();
      return;
    }
    this.#entries.delete(boardId);
    if (entry.store === skip) skip.markEvicted();
    else entry.store.close();
  }

  deleteWorkspace(workspaceId: string, skip?: B): void {
    for (const [id, entry] of [...this.#entries]) {
      if (entry.workspaceId === workspaceId) {
        this.#entries.delete(id);
        if (entry.store === skip) skip.markEvicted();
        else entry.store.close();
      }
    }
  }

  deleteOrg(workspaces: readonly WorkspaceInfo[], orgId: string, skip?: B): void {
    const ids = new Set(workspaces.filter((one) => one.orgId === orgId).map((one) => one.id));
    for (const [id, entry] of [...this.#entries]) {
      if (ids.has(entry.workspaceId)) {
        this.#entries.delete(id);
        if (entry.store === skip) skip.markEvicted();
        else entry.store.close();
      }
    }
  }

  deletePersonal(workspaces: readonly WorkspaceInfo[], skip?: B): void {
    const ids = new Set(workspaces.filter(isOwnSpace).map((one) => one.id));
    for (const [id, entry] of [...this.#entries]) {
      if (ids.has(entry.workspaceId)) {
        this.#entries.delete(id);
        if (entry.store === skip) skip.markEvicted();
        else entry.store.close();
      }
    }
  }

  pruneWorkspaces(workspaces: readonly WorkspaceInfo[], skip?: B): void {
    const open = new Set(
      workspaces
        .filter((workspace) => !workspace.ssoRequired && !workspace.passwordRequired)
        .map((workspace) => workspace.id),
    );
    for (const [id, entry] of [...this.#entries]) {
      if (!open.has(entry.workspaceId)) {
        this.#entries.delete(id);
        if (entry.store === skip) skip.markEvicted();
        else entry.store.close();
      }
    }
  }

  pruneContainers(workspaceId: string, allowedIds: ReadonlySet<string>, skip?: B): void {
    for (const [id, entry] of [...this.#entries]) {
      if (entry.workspaceId === workspaceId && !allowedIds.has(id)) {
        this.#entries.delete(id);
        if (entry.store === skip) skip.markEvicted();
        else entry.store.close();
      }
    }
  }

  /** Closes every cached board store and empties the cache. */
  clear(): void {
    const stores = [...this.#entries.values()].map((entry) => entry.store);
    this.#entries.clear();
    this.#ownerId = undefined;
    for (const store of stores) store.close();
  }

  #evict(boardId: string): void {
    const entry = this.#entries.get(boardId);
    if (!entry) return;
    this.#entries.delete(boardId);
    entry.store.close();
  }
}
