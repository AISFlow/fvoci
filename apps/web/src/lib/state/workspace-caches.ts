/**
 * Coordinates the three navigation caches owned by a `Workspace`: sidebar
 * snapshots (`WorkspaceCache`), recent board stores (`BoardCache`), and
 * last-opened wiki pages (`WikiCache`). Security and isolation invalidations
 * fan out to all three from one place.
 */
import type { Container } from '../logic/wire.ts';

import {
  isBackendError,
  ssoOrgOf,
  type Backend,
  type Category,
  type User,
  type WorkspaceInfo,
} from './backend.ts';
import {
  BoardCache,
  MAX_CACHED_BOARDS,
  isCacheableBoard,
  type CacheableBoard,
} from './board-cache.ts';
import { MAX_CACHED_WIKIS, WikiCache } from './wiki-cache.ts';
import type { SidebarWrites } from './sidebar-writes.ts';
import {
  MAX_CACHED_WORKSPACES,
  WorkspaceCache,
  mergeWorkspaceUsers,
  type WorkspaceSnapshot,
} from './workspace-cache.ts';
import * as entry from './workspace-entry.ts';
import { reread } from './write.ts';

export interface CacheLimits {
  workspaces?: number;
  boards?: number;
  wikis?: number;
}

export class WorkspaceCaches<B extends CacheableBoard = CacheableBoard> {
  readonly workspaces: WorkspaceCache;
  readonly boards: BoardCache<B>;
  readonly wikis: WikiCache;

  constructor(limits: CacheLimits = {}) {
    this.workspaces = new WorkspaceCache(limits.workspaces ?? MAX_CACHED_WORKSPACES);
    this.boards = new BoardCache<B>(limits.boards ?? MAX_CACHED_BOARDS);
    this.wikis = new WikiCache(limits.wikis ?? MAX_CACHED_WIKIS);
  }

  /** Immediately discards every cached workspace, board and wiki entry. */
  clear(): void {
    this.workspaces.clear();
    this.boards.clear();
    this.wikis.clear();
  }

  evictWorkspace(workspaceId: string, skipBoard?: B): void {
    this.workspaces.delete(workspaceId);
    this.boards.deleteWorkspace(workspaceId, skipBoard);
    this.wikis.deleteWorkspace(workspaceId);
  }

  evictOrg(workspaces: readonly WorkspaceInfo[], orgId: string, skipBoard?: B): void {
    this.workspaces.deleteOrg(workspaces, orgId);
    this.boards.deleteOrg(workspaces, orgId, skipBoard);
    this.wikis.deleteOrg(workspaces, orgId);
  }

  evictPersonal(workspaces: readonly WorkspaceInfo[], skipBoard?: B): void {
    this.workspaces.deletePersonal(workspaces);
    this.boards.deletePersonal(workspaces, skipBoard);
    this.wikis.deletePersonal(workspaces);
  }

  evictContainer(containerId: string, skipBoard?: B): void {
    this.boards.delete(containerId, skipBoard);
    this.wikis.delete(containerId);
  }

  pruneWorkspaces(workspaces: readonly WorkspaceInfo[], skipBoard?: B): void {
    this.workspaces.pruneWorkspaces(workspaces);
    this.boards.pruneWorkspaces(workspaces, skipBoard);
    this.wikis.pruneWorkspaces(workspaces);
  }

  pruneContainers(workspaceId: string, containers: readonly Container[], skipBoard?: B): void {
    const allowed = new Set(containers.map((container) => container.id));
    this.boards.pruneContainers(workspaceId, allowed, skipBoard);
    this.wikis.pruneContainers(workspaceId, allowed);
  }

  /** Evicts caches affected by an SSO or personal-space password lock error. */
  evictForClosing(
    host: { readonly workspaces: readonly WorkspaceInfo[]; readonly board: B | undefined },
    error: unknown,
    closing: { readonly entered: 'sso' | 'password'; readonly closed: WorkspaceInfo | undefined },
  ): void {
    if (closing.closed) this.evictWorkspace(closing.closed.id, host.board);
    if (closing.entered === 'password') {
      this.evictPersonal(host.workspaces, host.board);
      return;
    }
    const orgId = ssoOrgOf(error);
    if (orgId !== undefined) this.evictOrg(host.workspaces, orgId, host.board);
  }

  /** Evicts caches when entering or revalidating a workspace ends in missing/sso/password. */
  evictForEntered(
    workspaces: readonly WorkspaceInfo[],
    workspaceId: string,
    read: Exclude<entry.Entered, { status: 'ready' }>,
    skipBoard?: B,
  ): void {
    if (read.status === 'missing') this.evictWorkspace(workspaceId, skipBoard);
    else if (read.status === 'password') this.evictPersonal(workspaces, skipBoard);
    else if (read.closed.orgId) this.evictOrg(workspaces, read.closed.orgId, skipBoard);
    else this.evictWorkspace(workspaceId, skipBoard);
  }

  /** Parks a reusable open board into the LRU cache, or closes and drops it. */
  parkBoard(host: {
    board: (B & { park(): void }) | undefined;
    readonly me: User | undefined;
    readonly info: WorkspaceInfo | undefined;
  }): void {
    const open = host.board;
    host.board = undefined;
    if (!open) return;
    if (isCacheableBoard(open) && host.me && host.info) {
      open.park();
      this.boards.track(host.me.id, host.info.id, open);
    } else {
      this.boards.delete(open.boardId);
      open.close();
    }
  }

  /** Removes and closes an open board without parking it in cache. */
  dropBoard(host: { board: B | undefined }): void {
    const open = host.board;
    host.board = undefined;
    if (!open) return;
    this.boards.delete(open.boardId);
    open.close();
  }

  /** Registers an active board in the cache when it is eligible. */
  trackBoard(
    host: {
      readonly board: B | undefined;
      readonly me: User | undefined;
      readonly info: WorkspaceInfo | undefined;
    },
    store: B,
  ): void {
    if (host.board === store && isCacheableBoard(store) && host.me && host.info) {
      this.boards.track(host.me.id, host.info.id, store);
    }
  }
}

/** Host interface for background stale-while-revalidate and sidebar refreshes. */
export interface CachedEntryHost<B extends CacheableBoard> {
  readonly backend: Backend;
  readonly board: B | undefined;
  readonly me: User | undefined;
  workspaces: WorkspaceInfo[];
  info: WorkspaceInfo | undefined;
  closed: WorkspaceInfo | undefined;
  entered: entry.EnterStatus;
  asked: string | undefined;
  users: User[];
  workspaceUsers: User[];
  containers: Container[];
  categories: Category[];
  ends(error: unknown, reading?: boolean): boolean;
  report(action: 'load', error: unknown): void;
}

export interface SidebarRefreshHooks {
  readonly sidebarWrites: SidebarWrites;
  isSession(session: number): boolean;
  isCurrent(reading: number, workspace: WorkspaceInfo): boolean;
  refreshAgain(): void;
  rememberWorkspace(): void;
  leaveWithoutCaching(asked: string | undefined): void;
}

export function rememberWorkspaceSnapshot<B extends CacheableBoard>(
  caches: WorkspaceCaches<B>,
  host: CachedEntryHost<B>,
  cacheable: boolean,
): void {
  if (!cacheable || !host.me || !host.info || host.entered !== 'ready') return;
  caches.workspaces.set({
    userId: host.me.id,
    workspaceId: host.info.id,
    users: host.users,
    workspaceUsers: host.workspaceUsers,
    containers: host.containers,
    categories: host.categories,
  });
}

/**
 * Re-reads boards and sidebar categories after missed live events (`resync`) or
 * sidebar write races. Discards stale completions and evicts caches on 403/404.
 */
export async function refreshWorkspaceSidebar<B extends CacheableBoard>(
  host: CachedEntryHost<B>,
  caches: WorkspaceCaches<B>,
  hooks: SidebarRefreshHooks,
  session: number,
  reading: number,
  workspace: WorkspaceInfo,
): Promise<void> {
  const before = host.containers.map((board) => board.id);
  const begun = hooks.sidebarWrites.begun;
  try {
    const [boards, categories] = await Promise.all([
      entry.modulesOf(host.backend, workspace),
      host.backend.categories(workspace.id),
    ]);
    if (!hooks.isSession(session) || !hooks.isCurrent(reading, workspace)) return;
    host.containers = reread(host.containers, boards, before);
    caches.pruneContainers(workspace.id, host.containers, host.board);
    const verdict = hooks.sidebarWrites.reading(begun);
    if (verdict === 'show') host.categories = categories;
    else if (verdict === 'again') hooks.refreshAgain();
    hooks.rememberWorkspace();
  } catch (error) {
    if (!hooks.isSession(session)) return;
    if (isBackendError(error) && error.status === 401) {
      host.report('load', error);
      return;
    }
    if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
      caches.evictWorkspace(workspace.id, host.board);
      if (hooks.isCurrent(reading, workspace)) {
        hooks.leaveWithoutCaching(host.asked);
        host.entered = 'missing';
      }
    }
  }
}

export interface CachedEntryHooks {
  readonly sidebarWrites: SidebarWrites;
  isSession(session: number): boolean;
  isCurrent(turn: number, visit: WorkspaceInfo): boolean;
  refreshSidebar(): void;
  rememberWorkspace(): void;
  leaveWithoutCaching(asked: string): void;
}

/**
 * Shows a cached workspace snapshot immediately, subscribes live updates, and
 * revalidates in the background (stale-while-revalidate). Access revocations
 * (401/403/404, SSO, password lock) evict caches and close the workspace at once.
 */
export async function enterCachedWorkspace<B extends CacheableBoard>(
  host: CachedEntryHost<B>,
  caches: WorkspaceCaches<B>,
  hooks: CachedEntryHooks,
  session: number,
  turn: number,
  workspace: WorkspaceInfo,
  cached: WorkspaceSnapshot,
): Promise<void> {
  const visit = { ...workspace };
  [host.info, host.closed, host.entered] = [visit, undefined, 'ready'];
  host.users = cached.users;
  host.workspaceUsers = cached.workspaceUsers;
  host.containers = cached.containers;
  host.categories = cached.categories;
  host.backend.enter?.(workspace.id);
  const before = host.containers.map((one) => one.id);
  const usersBefore = new Map(host.users.map((user) => [user.id, user]));
  const begun = hooks.sidebarWrites.begun;
  try {
    const read = await entry.enterWorkspace(host.backend, visit, false);
    if (!hooks.isSession(session)) return;
    if (read.status !== 'ready') {
      caches.evictForEntered(host.workspaces, visit.id, read, host.board);
      if (!hooks.isCurrent(turn, visit)) return;
      host.workspaces = entry.withClosed(host.workspaces, read.closed);
      hooks.leaveWithoutCaching(visit.id);
      [host.closed, host.entered] = [read.closed, read.status];
      return;
    }
    if (!hooks.isCurrent(turn, visit)) return;
    host.containers = reread(host.containers, read.containers, before);
    const verdict = hooks.sidebarWrites.reading(begun);
    if (verdict === 'show') host.categories = read.categories;
    else if (verdict === 'again') hooks.refreshSidebar();
    const users = mergeWorkspaceUsers(host.users, cached.workspaceUsers, read.users, usersBefore);
    host.users = users;
    host.workspaceUsers = users.slice(0, read.users.length);
    hooks.rememberWorkspace();
    caches.pruneContainers(visit.id, host.containers, host.board);
  } catch (error) {
    // Background revalidation: ignore stale sessions; end current session or close workspace on 401.
    if (!hooks.isSession(session)) return;
    host.ends(error, true);
  }
}
