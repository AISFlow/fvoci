/**
 * The reader's workspace: who they are, the people around them, the boards
 * they can see, their sidebar, the single `activeContainer` slot, and their
 * settings. One per app.
 */
import { setDisplaySettings } from '../display.svelte.ts';
import { DEFAULT_ACCOUNT_SETTINGS } from '../logic/account-settings.ts';
import { resolveRegional, type Regional } from '../logic/regional.ts';
import { UndoManager } from '../logic/undo.ts';
import type { Container } from '../logic/wire.ts';
import { browserTimeZone, setRegionalSettings } from '../regional.ts';

import {
  isBackendError,
  sessionOver,
  type AccountSettings,
  type AccountSettingsPatch,
  type Backend,
  type BoardDraft,
  type Category,
  type Change,
  type CopiedContainerOutput,
  type CopyToWorkspaceInput,
  type DownloadedFile,
  type ImportFormat,
  type ImportSummary,
  type User,
  type WorkspaceInfo,
} from './backend.ts';
import { BoardStore } from './board.svelte.ts';
import type { CourseStore } from './course.svelte.ts';
import type { DiaryStore } from './diary.svelte.ts';
import { readerOf } from './memberships.svelte.ts';
import { hiddenContainersOf, sidebarOf, type SidebarCategory } from './order.ts';
import { SidebarWrites } from './sidebar-writes.ts';
import { Transfers } from './transfers.svelte.ts';
import { UserNames, type Learning } from './user-names.ts';
import type { WikiStore } from './wiki.svelte.ts';
import { WorkspaceCaches, type CacheLimits } from './workspace-caches.ts';
import { WorkspaceCategories } from './workspace-categories.ts';
import * as entry from './workspace-entry.ts';
import {
  applyWorkspaceChange,
  buildChangeTarget,
  changeWorkspaceSettings,
  deleteWorkspaceContainer,
  exportContainerArchive,
  exportWorkspaceArchive,
  importWorkspaceArchive,
  runUndoAction,
  type WorkspaceChangeTarget,
} from './workspace-sync.ts';
import type { LoadStatus, Problem, ProblemAction } from './workspace-types.ts';
import {
  buildVisitHost,
  enterWorkspaceVisit,
  openOrResumeBoard,
  refreshVisitSidebar,
  rememberVisit,
  type WorkspaceVisitHost,
} from './workspace-visit.ts';

export type { LoadStatus, Problem, ProblemAction };
export type ActiveContainer = BoardStore | WikiStore | CourseStore | DiaryStore;
export interface WorkspaceOptions {
  backend: Backend;
  locale: () => string;
  lastWorkspace?: (userId: string) => string | undefined;
  goneUserName?: () => string;
  cacheLimits?: CacheLimits;
}

export class Workspace {
  readonly backend: Backend;
  readonly #locale: () => string;
  readonly #lastWorkspace: ((userId: string) => string | undefined) | undefined;

  status = $state<LoadStatus>('idle');
  me = $state.raw<User | undefined>(undefined);
  workspaces = $state.raw<WorkspaceInfo[]>([]);
  info = $state.raw<WorkspaceInfo | undefined>(undefined);
  closed = $state.raw<WorkspaceInfo | undefined>(undefined);
  entered = $state<entry.EnterStatus>('none');
  asked = $state<string | undefined>(undefined);
  showing = false;
  users = $state.raw<User[]>([]);
  workspaceUsers = $state.raw<User[]>([]);
  containers = $state.raw<Container[]>([]);
  categories = $state.raw<Category[]>([]);
  settings = $state.raw<AccountSettings>(DEFAULT_ACCOUNT_SETTINGS);
  trashRetentionDays = $state(30);
  problems = $state.raw<Problem[]>([]);

  board = $state.raw<BoardStore | undefined>(undefined);
  activeContainer = $state.raw<ActiveContainer | undefined>(undefined);

  readonly undo = new UndoManager();
  canUndo = $state(false);
  canRedo = $state(false);
  undoBusy = $state(false);

  readonly sidebar = $derived<SidebarCategory[]>(
    sidebarOf(this.categories, this.containers, this.locale, readerOf(this)),
  );
  readonly hiddenContainers = $derived<Container[]>(
    hiddenContainersOf(this.categories, this.containers, this.sidebar, this.locale),
  );
  readonly regional = $derived<Regional>(
    resolveRegional(this.settings, this.locale, browserTimeZone()),
  );
  readonly #usersById = $derived<Record<string, User | undefined>>(
    Object.fromEntries(this.users.map((user) => [user.id, user])),
  );

  #stopWatching: (() => void) | undefined;
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- lifecycle disposers, not rendered
  readonly #disposers = new Set<() => void>();
  #problemCount = 0;
  #entering = 0;
  #refreshing = 0;
  #sessionGen = 0;
  #visitEvictionGen = 0;
  readonly #caches: WorkspaceCaches<BoardStore>;
  #enteringWorkspaceId: string | undefined;
  #pendingResync = false;
  #cacheableVisit = false;
  readonly #userNames: UserNames;
  readonly transfers = new Transfers(this);
  readonly #sidebarWrites: SidebarWrites;
  readonly #categoryOps: WorkspaceCategories;
  readonly #visit: WorkspaceVisitHost<BoardStore>;
  readonly #changeTarget: WorkspaceChangeTarget<BoardStore>;

  constructor(options: WorkspaceOptions) {
    this.backend = options.backend;
    this.#locale = options.locale;
    this.#lastWorkspace = options.lastWorkspace;
    this.#caches = new WorkspaceCaches<BoardStore>(options.cacheLimits);
    this.#userNames = new UserNames(this, options.goneUserName);
    this.#sidebarWrites = new SidebarWrites(() => void this.#refresh());
    this.#categoryOps = new WorkspaceCategories(
      this,
      this.#sidebarWrites,
      () => void this.#refresh(),
    );
    this.#visit = buildVisitHost(this, this.#caches, this.#sidebarWrites, {
      sessionGen: () => this.#sessionGen,
      nextEntering: () => ++this.#entering,
      currentEntering: () => this.#entering,
      nextRefreshing: () => ++this.#refreshing,
      currentRefreshing: () => this.#refreshing,
      visitEvictionGen: () => this.#visitEvictionGen,
      enteringWorkspaceId: () => this.#enteringWorkspaceId,
      setEnteringWorkspaceId: (id) => (this.#enteringWorkspaceId = id),
      pendingResync: () => this.#pendingResync,
      setPendingResync: (p) => (this.#pendingResync = p),
      cacheableVisit: () => this.#cacheableVisit,
      setCacheableVisit: (c) => (this.#cacheableVisit = c),
      leave: this.#leave.bind(this),
    });
    this.#changeTarget = buildChangeTarget(
      this,
      this.#caches,
      this.#sidebarWrites,
      this.#userNames,
      this.#takeSettings.bind(this),
      () => (this.#cacheableVisit = false),
      () => void this.#refresh(),
    );
    this.undo.onStateDidChange = () => {
      [this.canUndo, this.canRedo, this.undoBusy] = [
        this.undo.canUndo,
        this.undo.canRedo,
        this.undo.busy,
      ];
    };
  }

  get locale(): string {
    return this.#locale();
  }
  user(userId: string): User | undefined {
    return this.#usersById[userId];
  }
  userName(userId: string): string | undefined {
    const u = this.#usersById[userId] ?? (userId === this.me?.id ? this.me : undefined);
    return u && this.#userNames.nameOf(u);
  }
  onClose(dispose: () => void): () => void {
    this.#disposers.add(dispose);
    return () => void this.#disposers.delete(dispose);
  }
  activateContainer(store: WikiStore | CourseStore | DiaryStore): () => void {
    this.closeBoard();
    this.activeContainer = store;
    return () => {
      if (this.activeContainer === store) this.activeContainer = undefined;
    };
  }

  async load(workspaceId?: string, leaves?: (settings: AccountSettings) => boolean): Promise<void> {
    const session = ++this.#sessionGen;
    this.#entering += 1;
    this.#refreshing += 1;
    this.#caches.clear();
    this.#leave(undefined, false);
    this.workspaces = [];
    this.status = 'loading';
    try {
      const [me, workspaces, settings, retentionDays] = await Promise.all([
        this.backend.me(),
        this.backend.workspaces(),
        this.backend.settings(),
        this.backend.trashRetentionDays().catch(() => 30),
      ]);
      if (session !== this.#sessionGen) return;
      this.me = me;
      this.workspaces = workspaces;
      this.trashRetentionDays = retentionDays;
      this.#takeSettings(settings);
      if (leaves?.(settings)) return;
      this.#stopWatching?.();
      this.#stopWatching = this.backend.watchSelf(this.#apply.bind(this));
      const chosen = workspaceId ?? this.firstChoice();
      if (chosen === undefined) this.#leave(undefined, false);
      else await enterWorkspaceVisit(this.#visit, chosen, true);
      if (session === this.#sessionGen) this.status = 'ready';
    } catch (error) {
      if (session !== this.#sessionGen) return;
      const over = sessionOver(error);
      if (over) {
        this.#over(over);
        return;
      }
      this.status = 'failed';
      this.report('load', error);
    }
  }

  firstChoice(): string | undefined {
    return entry.firstChoiceOf(this.workspaces, this.me && this.#lastWorkspace?.(this.me.id));
  }

  async enter(workspaceId: string, again = false): Promise<void> {
    if (!again && this.asked === workspaceId && this.entered !== 'failed') return;
    const [turn, session] = [this.#entering + 1, this.#sessionGen];
    try {
      await enterWorkspaceVisit(this.#visit, workspaceId, again);
    } catch (error) {
      if (session !== this.#sessionGen || this.ends(error, true) || turn !== this.#entering) return;
      this.entered = 'failed';
      this.report('load', error);
    }
  }

  async reloadWorkspaces(): Promise<void> {
    const session = this.#sessionGen;
    try {
      const workspaces = await this.backend.workspaces();
      if (session !== this.#sessionGen) return;
      this.workspaces = workspaces;
      this.pruneWorkspaces();
    } catch (error) {
      if (session === this.#sessionGen) this.ends(error);
    }
  }

  #leave(asked: string | undefined, cache = false): void {
    if (cache) {
      rememberVisit(this.#visit);
      this.closeBoard();
    } else {
      this.#dropActiveContainers();
    }
    [this.#cacheableVisit, this.#pendingResync] = [false, false];
    this.undo.clear();
    [this.asked, this.info, this.closed, this.entered] = [asked, undefined, undefined, 'none'];
    [this.users, this.workspaceUsers, this.containers, this.categories] = [[], [], [], []];
  }

  #dropActiveContainers(): void {
    const active = this.activeContainer;
    this.activeContainer = undefined;
    if (active && active !== this.board) active.close();
    this.#caches.dropBoard(this);
  }

  #over(status: 'signedOut' | 'leaving'): void {
    this.#sessionGen += 1;
    this.status = status;
    this.#leave(undefined, false);
    this.workspaces = [];
    this.close();
  }

  close(): void {
    [this.#entering, this.#refreshing, this.#pendingResync, this.#enteringWorkspaceId] = [
      this.#entering + 1,
      this.#refreshing + 1,
      false,
      undefined,
    ];
    this.#stopWatching?.();
    this.#stopWatching = undefined;
    this.#dropActiveContainers();
    this.#caches.clear();
    for (const dispose of [...this.#disposers]) dispose();
  }

  #takeSettings(settings: AccountSettings): void {
    this.settings = settings;
    setRegionalSettings(settings);
    setDisplaySettings(settings);
  }

  #apply(change: Change): void {
    applyWorkspaceChange(this.#changeTarget, change);
  }
  #refresh(): Promise<void> {
    return refreshVisitSidebar(this.#visit);
  }

  report(action: ProblemAction, error: unknown): void {
    if (this.ends(error, action === 'load')) return;
    if (isBackendError(error) && error.status === 403) {
      if (this.info) {
        this.#cacheableVisit = false;
        this.#caches.workspaces.delete(this.info.id);
      }
      if (this.board) this.#caches.boards.delete(this.board.boardId, this.board);
    }
    this.#problemCount += 1;
    this.problems = [
      ...this.problems,
      { id: this.#problemCount, action, error: isBackendError(error) ? error : undefined },
    ];
    if (error !== undefined && !isBackendError(error)) console.error(error);
  }

  ends(error: unknown, reading = false): boolean {
    const closing = entry.closedBy(this.workspaces, this.info, error);
    if (closing) {
      if (reading && this.showing && closing.entered === 'password') return true;
      this.#caches.evictForClosing(this, error, closing);
      this.workspaces = closing.workspaces;
      if (!closing.closed) return true;
      this.#entering += 1;
      this.#leave(this.asked, false);
      [this.closed, this.entered] = [closing.closed, closing.entered];
      return true;
    }
    const over = sessionOver(error);
    if (over) this.#over(over);
    return over !== undefined;
  }

  dismissProblem(id: number): void {
    this.problems = this.problems.filter((problem) => problem.id !== id);
  }
  async learnUsers(userIds: readonly string[], learning: Learning = {}): Promise<void> {
    await this.#userNames.learn(userIds, learning);
  }
  searchUsers(query: string): Promise<User[]> {
    return this.#userNames.search(query);
  }
  changeSettings(patch: AccountSettingsPatch): Promise<void> {
    const [take, report] = [this.#takeSettings.bind(this), this.report.bind(this)];
    return changeWorkspaceSettings(this.settings, this.backend, take, report, patch);
  }
  openBoard(boardId: string): Promise<BoardStore> {
    return openOrResumeBoard(this, this.#caches, boardId, () => new BoardStore(this, boardId));
  }
  closeBoard(): void {
    const wasBoard = this.board;
    this.#caches.parkBoard(this);
    if (wasBoard && this.activeContainer === wasBoard) this.activeContainer = undefined;
  }

  #invalidateVisit(): void {
    this.#visitEvictionGen += 1;
    this.#refreshing += 1;
    this.#cacheableVisit = false;
    this.board?.markEvicted();
  }
  evictWorkspace(workspaceId: string): void {
    if (this.info?.id === workspaceId || this.#enteringWorkspaceId === workspaceId)
      this.#invalidateVisit();
    this.#caches.evictWorkspace(workspaceId, this.board);
  }
  evictOrg(orgId: string): void {
    const entering = this.workspaces.find((one) => one.id === this.#enteringWorkspaceId);
    if (this.info?.orgId === orgId || entering?.orgId === orgId) this.#invalidateVisit();
    this.#caches.evictOrg(this.workspaces, orgId, this.board);
  }
  evictContainer(containerId: string): void {
    this.#caches.evictContainer(containerId, this.board);
  }
  pruneWorkspaces(): void {
    const current = this.info && this.workspaces.find((one) => one.id === this.info?.id);
    if (this.info && (!current || !entry.isOpen(current))) this.#invalidateVisit();
    this.#caches.pruneWorkspaces(this.workspaces, this.board);
  }
  rememberedWikiPage(wikiId: string): string | undefined {
    const { me, info } = this;
    return me && info ? this.#caches.wikis.get(me.id, info.id, wikiId) : undefined;
  }
  rememberWikiPage(wikiId: string, pageId: string): void {
    if (this.me && this.info) this.#caches.wikis.set(this.me.id, this.info.id, wikiId, pageId);
  }
  forgetWikiPage(wikiId: string, pageId?: string): void {
    if (pageId === undefined) this.#caches.wikis.delete(wikiId);
    else this.#caches.wikis.deletePage(wikiId, pageId);
  }
  createBoard(d: BoardDraft, viewTitle = '', status = 'Status'): Promise<Container | undefined> {
    return entry.createBoardOf(this, d, viewTitle, status);
  }
  searchBoards(query: string): Promise<Container[]> {
    return entry.searchBoardsOf(this, query);
  }
  templates(): Promise<Container[]> {
    return entry.templatesOf(this);
  }
  receiveContainer(container: Container): void {
    this.#apply({ kind: 'container', container });
  }
  duplicateContainer(containerId: string): Promise<Container | undefined> {
    return entry.duplicateContainerOf(this, containerId, false);
  }
  createTemplateFromBoard(boardId: string): Promise<Container | undefined> {
    return entry.duplicateContainerOf(this, boardId, true);
  }
  copyContainerToWorkspace(id: string, req: CopyToWorkspaceInput): Promise<CopiedContainerOutput> {
    return entry.copyContainerToWorkspaceOf(this, id, req);
  }
  onboard(): Promise<string | undefined> {
    return entry.onboardOf(this);
  }
  exportContainer(containerId: string): Promise<DownloadedFile | undefined> {
    return exportContainerArchive(this.transfers, this.backend, this.containers, containerId);
  }
  exportWorkspace(): Promise<DownloadedFile | undefined> {
    return exportWorkspaceArchive(this.transfers, this.backend, this.info);
  }
  importArchive(file: File, format: ImportFormat = 'fvoci'): Promise<ImportSummary | undefined> {
    return importWorkspaceArchive(
      this.transfers,
      this.backend,
      this.#caches,
      () => this.info,
      () => this.#refresh(),
      file,
      format,
    );
  }
  deleteContainer(containerId: string): Promise<void> {
    return deleteWorkspaceContainer(
      this.undo,
      this.backend,
      this.#apply.bind(this),
      this.restoreContainer.bind(this),
      this.report.bind(this),
      containerId,
    );
  }
  async restoreContainer(containerId: string): Promise<void> {
    const container = await this.backend.restoreContainer(containerId);
    await this.#refresh();
    this.#apply({ kind: 'container', container });
  }
  createCategory(name: string): Promise<void> {
    return this.#categoryOps.create(name);
  }
  renameCategory(id: string, name: string): Promise<void> {
    return this.#categoryOps.rename(id, name);
  }
  setCategoryCollapsed(id: string, collapsed: boolean): Promise<void> {
    return this.#categoryOps.setCollapsed(id, collapsed);
  }
  deleteCategory(id: string): Promise<void> {
    return this.#categoryOps.remove(id);
  }
  reorderCategories(ids: readonly string[]): Promise<void> {
    return this.#categoryOps.reorder(ids);
  }
  moveContainer(containerId: string, categoryId: string, index: number): Promise<void> {
    return this.#categoryOps.moveContainer(containerId, categoryId, index);
  }
  hideContainer(containerId: string): Promise<void> {
    return this.#categoryOps.hideContainer(containerId);
  }
  unhideContainer(containerId: string): Promise<void> {
    return this.#categoryOps.unhideContainer(containerId);
  }
  undoLast(): Promise<void> {
    return runUndoAction(() => this.undo.undo(), this.report.bind(this));
  }
  redoLast(): Promise<void> {
    return runUndoAction(() => this.undo.redo(), this.report.bind(this));
  }
}
