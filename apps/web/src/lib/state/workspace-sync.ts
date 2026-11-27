/**
 * Live change handling, settings updates, and archive helpers for `Workspace`
 * (`workspace.svelte.ts`), extracted to keep `Workspace` focused and <= 500 lines.
 */
import { DEFAULT_ACCOUNT_SETTINGS } from '../logic/account-settings.ts';
import type { Container } from '../logic/wire.ts';

import {
  type AccountSettings,
  type AccountSettingsPatch,
  type Backend,
  type Category,
  type Change,
  type DownloadedFile,
  type ImportFormat,
  type ImportSummary,
  type WorkspaceInfo,
} from './backend.ts';
import type { CacheableBoard } from './board-cache.ts';
import type { UndoManager } from '../logic/undo.ts';
import { withoutContainer } from './order.ts';
import type { SidebarWrites } from './sidebar-writes.ts';
import type { Transfers } from './transfers.svelte.ts';
import type { UserNames } from './user-names.ts';
import type { WorkspaceCaches } from './workspace-caches.ts';
import type { ProblemAction } from './workspace-types.ts';

export interface WorkspaceChangeTarget<B extends CacheableBoard> {
  readonly meId: string | undefined;
  readonly info: WorkspaceInfo | undefined;
  readonly board: B | undefined;
  containers: Container[];
  categories: Category[];
  readonly caches: WorkspaceCaches<B>;
  readonly sidebarWrites: SidebarWrites;
  readonly userNames: UserNames;
  takeSettings(settings: AccountSettings): void;
  setInfoRole(role: WorkspaceInfo['role']): void;
  reloadWorkspaces(): void;
  invalidateVisitCache(): void;
  refresh(): void;
}

export function applyWorkspaceChange<B extends CacheableBoard>(
  target: WorkspaceChangeTarget<B>,
  change: Change,
): void {
  switch (change.kind) {
    case 'container': {
      if (change.container.workspaceId !== target.info?.id) break;
      const known = target.containers.find((board) => board.id === change.container.id);
      if (known && known.updatedAt > change.container.updatedAt) break;
      const others = target.containers.filter((board) => board.id !== change.container.id);
      target.containers = change.container.isTemplate ? others : [...others, change.container];
      break;
    }
    case 'containerDeleted':
      target.caches.evictContainer(change.containerId, target.board);
      target.containers = target.containers.filter((board) => board.id !== change.containerId);
      target.categories = withoutContainer(target.categories, change.containerId);
      break;
    case 'categories':
      if (target.sidebarWrites.accepts(change.categories, target.info?.id)) {
        target.categories = change.categories;
      }
      break;
    case 'settings':
      target.takeSettings(change.settings);
      break;
    case 'resync':
      if (target.info) {
        target.invalidateVisitCache();
        target.caches.workspaces.delete(target.info.id);
      }
      target.refresh();
      void target.userNames.reread();
      break;
    case 'user':
      void target.userNames.reread([change.userId]);
      break;
    case 'workspaceRole':
      if (change.userId === target.meId) {
        if (
          change.workspaceId === target.info?.id &&
          (change.role === 'admin' || change.role === 'member' || change.role === 'guest')
        ) {
          target.setInfoRole(change.role);
        }
        target.reloadWorkspaces();
      }
      break;
    default:
      break;
  }
}

export async function changeWorkspaceSettings(
  current: AccountSettings,
  backend: Backend,
  takeSettings: (settings: AccountSettings) => void,
  report: (action: ProblemAction, error: unknown) => void,
  patch: AccountSettingsPatch,
): Promise<void> {
  const optimistic: Record<string, unknown> = { ...current };
  for (const [name, value] of Object.entries(patch)) {
    optimistic[name] = value ?? DEFAULT_ACCOUNT_SETTINGS[name as keyof AccountSettings];
  }
  takeSettings(optimistic as unknown as AccountSettings);
  try {
    takeSettings(await backend.patchSettings(patch));
  } catch (error) {
    takeSettings(current);
    report('save', error);
  }
}

export function exportContainerArchive(
  transfers: Transfers,
  backend: Backend,
  containers: readonly Container[],
  containerId: string,
): Promise<DownloadedFile | undefined> {
  const board = containers.find((one) => one.id === containerId);
  return transfers.run('export', `board ${containerId}`, board?.title ?? '', (options) =>
    backend.exportContainer(containerId, options),
  );
}

export function exportWorkspaceArchive(
  transfers: Transfers,
  backend: Backend,
  workspace: WorkspaceInfo | undefined,
): Promise<DownloadedFile | undefined> {
  if (!workspace) return Promise.resolve(undefined);
  return transfers.run('export', `workspace ${workspace.id}`, workspace.title, (options) =>
    backend.exportWorkspace(workspace.id, options),
  );
}

export async function importWorkspaceArchive<B extends CacheableBoard>(
  transfers: Transfers,
  backend: Backend,
  caches: WorkspaceCaches<B>,
  currentWorkspace: () => WorkspaceInfo | undefined,
  refresh: () => Promise<void>,
  file: File,
  format: ImportFormat = 'fvoci',
): Promise<ImportSummary | undefined> {
  const workspace = currentWorkspace();
  if (!workspace) return undefined;
  const done = await transfers.run(
    'import',
    `import ${format} ${file.name} ${file.size} ${file.lastModified}`,
    file.name,
    (options) => backend.importArchive(workspace.id, file, { ...options, format }),
  );
  if (currentWorkspace()?.id === workspace.id) await refresh();
  else caches.workspaces.delete(workspace.id);
  return done;
}

export function buildChangeTarget<B extends CacheableBoard>(
  store: {
    readonly me: { readonly id: string } | undefined;
    info: WorkspaceInfo | undefined;
    readonly board: B | undefined;
    containers: Container[];
    categories: Category[];
    reloadWorkspaces(): Promise<void>;
  },
  caches: WorkspaceCaches<B>,
  sidebarWrites: SidebarWrites,
  userNames: UserNames,
  takeSettings: (settings: AccountSettings) => void,
  invalidateVisitCache: () => void,
  refresh: () => void,
): WorkspaceChangeTarget<B> {
  return {
    get meId() {
      return store.me?.id;
    },
    get info() {
      return store.info;
    },
    get board() {
      return store.board;
    },
    get containers() {
      return store.containers;
    },
    set containers(next) {
      store.containers = next;
    },
    get categories() {
      return store.categories;
    },
    set categories(next) {
      store.categories = next;
    },
    caches,
    sidebarWrites,
    userNames,
    takeSettings,
    setInfoRole(role) {
      if (store.info) store.info = { ...store.info, role };
    },
    reloadWorkspaces() {
      void store.reloadWorkspaces();
    },
    invalidateVisitCache,
    refresh,
  };
}

export async function deleteWorkspaceContainer(
  undo: UndoManager,
  backend: Backend,
  apply: (change: Change) => void,
  restore: (containerId: string) => Promise<void>,
  report: (action: ProblemAction, error: unknown) => void,
  containerId: string,
): Promise<void> {
  try {
    await undo.perform(
      async () => {
        await backend.deleteContainer(containerId);
        apply({ kind: 'containerDeleted', containerId });
      },
      () => restore(containerId),
      'deleteContainer',
    );
  } catch (error) {
    report('save', error);
  }
}

export async function runUndoAction(
  run: () => Promise<unknown>,
  report: (action: ProblemAction, error: unknown) => void,
): Promise<void> {
  try {
    await run();
  } catch (error) {
    report('undo', error);
  }
}
