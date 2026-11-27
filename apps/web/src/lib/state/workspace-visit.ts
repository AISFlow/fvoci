/**
 * Workspace entry, cached-visit revalidation, and sidebar refresh flow for
 * `Workspace` (`workspace.svelte.ts`), extracted to keep `Workspace` concise.
 */
import type { WorkspaceInfo } from './backend.ts';
import type { CacheableBoard } from './board-cache.ts';
import type { SidebarWrites } from './sidebar-writes.ts';
import type { WorkspaceSnapshot } from './workspace-cache.ts';
import {
  enterCachedWorkspace,
  refreshWorkspaceSidebar,
  rememberWorkspaceSnapshot,
  type WorkspaceCaches,
} from './workspace-caches.ts';
import * as entry from './workspace-entry.ts';
import type { Workspace } from './workspace.svelte.ts';

export interface WorkspaceVisitHost<B extends CacheableBoard> {
  readonly store: Workspace & { board: B | undefined };
  readonly caches: WorkspaceCaches<B>;
  readonly sidebarWrites: SidebarWrites;
  sessionGen(): number;
  nextEntering(): number;
  currentEntering(): number;
  nextRefreshing(): number;
  currentRefreshing(): number;
  visitEvictionGen(): number;
  enteringWorkspaceId(): string | undefined;
  setEnteringWorkspaceId(id: string | undefined): void;
  pendingResync(): boolean;
  setPendingResync(pending: boolean): void;
  cacheableVisit(): boolean;
  setCacheableVisit(cacheable: boolean): void;
  leave(asked: string | undefined, cache: boolean): void;
}

export function rememberVisit<B extends CacheableBoard>(host: WorkspaceVisitHost<B>): void {
  rememberWorkspaceSnapshot(host.caches, host.store, host.cacheableVisit());
}

export function buildVisitHost<B extends CacheableBoard>(
  store: Workspace & { board: B | undefined },
  caches: WorkspaceCaches<B>,
  sidebarWrites: SidebarWrites,
  hooks: Omit<WorkspaceVisitHost<B>, 'store' | 'caches' | 'sidebarWrites'>,
): WorkspaceVisitHost<B> {
  return { store, caches, sidebarWrites, ...hooks };
}

export async function openOrResumeBoard<
  B extends CacheableBoard & {
    readonly status: string;
    readonly missing: boolean;
    load(): Promise<void>;
    resume(): Promise<void>;
  },
>(
  store: {
    board: B | undefined;
    activeContainer: unknown;
    readonly me: Workspace['me'];
    readonly info: Workspace['info'];
    closeBoard(): void;
  },
  caches: WorkspaceCaches<B>,
  boardId: string,
  make: () => B,
): Promise<B> {
  if (store.board?.boardId === boardId && store.board.status !== 'failed' && !store.board.missing) {
    store.activeContainer = store.board;
    return store.board;
  }
  store.closeBoard();
  const cached =
    store.me && store.info ? caches.boards.take(store.me.id, store.info.id, boardId) : undefined;
  if (cached) {
    store.board = cached;
    store.activeContainer = cached;
    await cached.resume();
    caches.trackBoard(store, cached);
    return cached;
  }
  const created = make();
  store.board = created;
  store.activeContainer = created;
  await created.load();
  caches.trackBoard(store, created);
  return created;
}

function flushPendingResync<B extends CacheableBoard>(
  host: WorkspaceVisitHost<B>,
  turn: number,
  workspaceId: string,
): void {
  if (!host.pendingResync()) return;
  host.setPendingResync(false);
  if (turn === host.currentEntering() && host.store.info?.id === workspaceId) {
    void refreshVisitSidebar(host);
  }
}

export async function enterWorkspaceVisit<B extends CacheableBoard>(
  host: WorkspaceVisitHost<B>,
  workspaceId: string,
  again = false,
): Promise<void> {
  const { store, caches } = host;
  const [turn, session] = [host.nextEntering(), host.sessionGen()];
  host.leave(workspaceId, true);
  const workspace = store.workspaces.find((one) => one.id === workspaceId);
  if (!workspace) {
    caches.evictWorkspace(workspaceId);
    store.entered = 'missing';
    return;
  }
  const cached = !again && store.me ? caches.workspaces.get(store.me.id, workspaceId) : undefined;
  if (cached && entry.isOpen(workspace)) {
    await enterCachedVisit(host, session, turn, workspace, cached);
    return;
  }
  store.info = { ...workspace };
  store.entered = 'loading';
  host.setEnteringWorkspaceId(workspaceId);
  const evictionGen = host.visitEvictionGen();
  let read: entry.Entered;
  try {
    read = await entry.enterWorkspace(store.backend, workspace);
  } finally {
    if (turn === host.currentEntering()) host.setEnteringWorkspaceId(undefined);
  }
  if (session !== host.sessionGen()) return;
  if (read.status !== 'ready') {
    caches.evictForEntered(store.workspaces, workspaceId, read, store.board);
  }
  if (turn !== host.currentEntering()) return;
  [store.entered, store.info, store.closed] = [read.status, read.info, read.closed];
  store.workspaces = entry.withClosed(store.workspaces, read.closed);
  if (read.status === 'ready') {
    ({ users: store.users, containers: store.containers, categories: store.categories } = read);
    store.workspaceUsers = read.users;
    if (host.visitEvictionGen() === evictionGen) {
      host.setCacheableVisit(true);
      rememberVisit(host);
    }
    caches.pruneContainers(workspaceId, store.containers, store.board);
    flushPendingResync(host, turn, workspaceId);
  }
}

async function enterCachedVisit<B extends CacheableBoard>(
  host: WorkspaceVisitHost<B>,
  session: number,
  turn: number,
  workspace: WorkspaceInfo,
  cached: WorkspaceSnapshot,
): Promise<void> {
  host.setEnteringWorkspaceId(workspace.id);
  const evictionGen = host.visitEvictionGen();
  host.setCacheableVisit(true);
  try {
    await enterCachedWorkspace(
      host.store,
      host.caches,
      {
        sidebarWrites: host.sidebarWrites,
        isSession: (s) => s === host.sessionGen(),
        isCurrent: (t, visit) => t === host.currentEntering() && host.store.info === visit,
        refreshSidebar: () => {
          host.setPendingResync(true);
        },
        rememberWorkspace: () => {
          if (host.visitEvictionGen() !== evictionGen) return;
          host.setCacheableVisit(true);
          rememberVisit(host);
        },
        leaveWithoutCaching: (asked) => {
          host.leave(asked, false);
        },
      },
      session,
      turn,
      workspace,
      cached,
    );
  } finally {
    if (turn === host.currentEntering()) {
      host.setEnteringWorkspaceId(undefined);
      flushPendingResync(host, turn, workspace.id);
    }
  }
}

export async function refreshVisitSidebar<B extends CacheableBoard>(
  host: WorkspaceVisitHost<B>,
): Promise<void> {
  const { store, caches } = host;
  const workspace = store.info;
  if (store.status !== 'ready' || !workspace) return;
  if (host.enteringWorkspaceId() === workspace.id) {
    host.setPendingResync(true);
    return;
  }
  const [session, reading] = [host.sessionGen(), host.nextRefreshing()];
  await refreshWorkspaceSidebar(
    store,
    caches,
    {
      sidebarWrites: host.sidebarWrites,
      isSession: (s) => s === host.sessionGen(),
      isCurrent: (r, ws) => r === host.currentRefreshing() && store.info === ws,
      refreshAgain: () => void refreshVisitSidebar(host),
      rememberWorkspace: () => {
        host.setCacheableVisit(true);
        rememberVisit(host);
      },
      leaveWithoutCaching: (asked) => {
        host.leave(asked, false);
      },
    },
    session,
    reading,
    workspace,
  );
}
