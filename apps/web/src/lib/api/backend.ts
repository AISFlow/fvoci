/**
 * The backend that calls the server: the data boundary
 * (`lib/state/backend.ts`) over the server's API (`contracts/openapi.json`).
 */
import type { CardPreview } from '../editor/card-preview.ts';
import type { DocNode } from '../editor/schema.ts';
import type {
  BlockPatch,
  BoardView,
  Container,
  ContainerMember,
  ContainerPatch,
} from '../logic/wire.ts';
import type {
  Account,
  AccountSettings,
  AccountSettingsPatch,
  AccountToday,
  Attachment,
  Backend,
  BoardDraft,
  BoardViewWriteResult,
  BodyDoc,
  BodyDocHears,
  BodyWrite,
  CardDetail,
  CardDraft,
  CardMovesInput,
  CardMovesOutput,
  CardPatch,
  CardPropertiesPatch,
  CardSummary,
  CardTelling,
  Category,
  CategoryDraft,
  CategoryPatch,
  ChangeListener,
  Comment,
  CommentVersion,
  ContainerPermissions,
  ContainerRole,
  CopiedContainerOutput,
  CopyToWorkspaceInput,
  DownloadedFile,
  DownloadOptions,
  ImportOptions,
  PublicLink,
  PublicLinkChange,
  UploadedFile,
  UploadOptions,
  User,
  ViewDraft,
  WorkspaceToday,
} from '../state/backend.ts';
import { openRoomLater } from '../state/doc-opener.ts';
import { BlockTurns } from '../state/write.ts';

import { ServerBlocks } from './block-requests.ts';
import { ServerBulk } from './bulk-requests.ts';
import { ServerContainers } from './container-requests.ts';
import { ClientConfig, ServerContentSearch } from './content-search.ts';
import { settingsChangeOf, settingsOf, userOf, type Schemas } from './convert.ts';
import { Http } from './http.ts';
import { LiveChanges } from './live.ts';
import { ServerRequests, underWorkspace } from './requests.ts';
import { ServerSidebar } from './sidebar-requests.ts';
import { ServerTransfers } from './upload-slot.ts';

export class ServerBackend extends ServerRequests implements Backend, Account {
  readonly #http: Http;
  readonly #turns = new BlockTurns();
  readonly #live: LiveChanges;
  readonly presence: Backend['presence'];
  readonly search: Backend['search'];
  readonly #config: ClientConfig;
  readonly #sidebar: ServerSidebar;
  readonly #containers: ServerContainers;
  readonly #blocks: ServerBlocks;
  readonly #bulk: ServerBulk;
  readonly #transfers: ServerTransfers;

  constructor(basePath: string, http = new Http(basePath)) {
    super(http);
    this.#http = http;
    this.#config = new ClientConfig(http);
    this.search = new ServerContentSearch(http, this.#config);
    const inWorkspace = <T>(
      path: Parameters<ServerRequests['inWorkspace']>[0],
      params?: Record<string, string>,
      options?: Parameters<ServerRequests['inWorkspace']>[2],
      wsId?: string,
    ): Promise<T> => this.inWorkspace<T>(path, params, options, wsId);
    this.#sidebar = new ServerSidebar(http, this.#turns, () => this.currentUserId());
    this.#blocks = new ServerBlocks(inWorkspace, this.#turns);
    this.#bulk = new ServerBulk(inWorkspace, this.#turns);
    this.#transfers = new ServerTransfers(http, this.#config);
    this.#containers = new ServerContainers({
      http,
      turns: this.#turns,
      inWorkspace,
      learnPlaces: (containers) => {
        this.learnPlaces(containers);
      },
      learnView: (view) => {
        this.#blocks.learnView(view);
      },
    });
    const socket = new URL(`${basePath}/ws`, location.href);
    socket.protocol = socket.protocol === 'https:' ? 'wss:' : 'ws:';
    this.#live = new LiveChanges(socket.href, {
      members: (containerId) => this.members(containerId),
      learnContainer: (container) => {
        this.learnPlaces([container]);
      },
      learnBlock: (block) => {
        this.#blocks.learnLiveBlock(block);
      },
    });
    this.presence = this.#live.presence;
  }

  async signIn(login: string, password: string): Promise<void> {
    this.forgetCachedUser();
    const request: Schemas['LoginRequest'] = { username: login, email: login, password };
    await this.#http.request('/api/v1/login', {}, { method: 'POST', json: request });
    this.#live.restart();
  }

  async signOut(): Promise<void> {
    this.forgetCachedUser();
    await this.#http.request('/api/v1/logout', {}, { method: 'POST' });
  }

  async users(workspaceId: string): Promise<User[]> {
    const users = await this.#http.request<Schemas['User'][]>(underWorkspace('/users'), {
      workspaceId,
    });
    return users.map(userOf);
  }

  async searchUsers(workspaceId: string, query: string): Promise<User[]> {
    const users = await this.#http.request<Schemas['User'][]>(
      underWorkspace('/users'),
      { workspaceId },
      { query: { search: query } },
    );
    return users.map(userOf);
  }

  async settings(): Promise<AccountSettings> {
    return settingsOf(
      await this.#http.request<Schemas['UserSettings']>('/api/v1/users/me/settings'),
    );
  }

  async patchSettings(patch: AccountSettingsPatch): Promise<AccountSettings> {
    const settings = await this.#http.request<Schemas['UserSettings']>(
      '/api/v1/users/me/settings',
      {},
      { method: 'PATCH', json: settingsChangeOf(patch) },
    );
    return settingsOf(settings);
  }

  async screenState(): Promise<Record<string, string>> {
    return this.#http.request<Record<string, string>>('/api/v1/users/me/client-state');
  }

  async patchScreenState(updated: Record<string, string>, deleted: string[] = []): Promise<void> {
    const patch: Schemas['ClientStatePatch'] = {
      updatedFields: updated,
      deletedFields: deleted,
    };
    await this.#http.request('/api/v1/users/me/client-state', {}, { method: 'PATCH', json: patch });
  }

  createBoard(
    w: string,
    d: BoardDraft,
    v: ViewDraft,
  ): Promise<{ board: Container; view: BoardView }> {
    return this.#containers.createBoard(w, d, v);
  }
  reorderCardProperties(boardId: string, propertyIds: string[]): Promise<string[]> {
    return this.#containers.reorderCardProperties(boardId, propertyIds);
  }
  patchContainer(id: string, patch: ContainerPatch): Promise<Container> {
    return this.#containers.patchContainer(id, patch);
  }
  searchBoards(workspaceId: string, query: string): Promise<Container[]> {
    return this.#containers.searchBoards(workspaceId, query);
  }
  templates(workspaceId: string): Promise<Container[]> {
    return this.#containers.templates(workspaceId);
  }
  duplicateContainer(id: string, asTemplate: boolean, to: string): Promise<Container> {
    return this.#containers.duplicateContainer(id, asTemplate, to);
  }
  onboard(workspaceId: string): Promise<string> {
    return this.#containers.onboard(workspaceId);
  }
  deleteContainer(id: string): Promise<void> {
    return this.#containers.deleteContainer(id);
  }
  restoreContainer(id: string): Promise<Container> {
    return this.#containers.restoreContainer(id);
  }
  members(id: string): Promise<ContainerMember[]> {
    return this.#containers.members(id);
  }
  addMember(id: string, userId: string, role: ContainerRole): Promise<ContainerMember> {
    return this.#containers.addMember(id, userId, role);
  }
  setMemberRole(id: string, userId: string, role: ContainerRole): Promise<ContainerMember> {
    return this.#containers.setMemberRole(id, userId, role);
  }
  removeMember(id: string, userId: string): Promise<void> {
    return this.#containers.removeMember(id, userId);
  }
  publicLink(id: string): Promise<PublicLink> {
    return this.#containers.publicLink(id);
  }
  setPublicLink(id: string, change: PublicLinkChange): Promise<PublicLink> {
    return this.#containers.setPublicLink(id, change);
  }
  joinContainer(id: string): Promise<ContainerMember> {
    return this.#containers.joinContainer(id);
  }
  async trashRetentionDays(): Promise<number> {
    return (await this.#config.read()).trashRetentionDays;
  }
  myPermissions(containerId: string): Promise<ContainerPermissions> {
    return this.#containers.myPermissions(containerId);
  }
  copyContainerToWorkspace(
    containerId: string,
    request: CopyToWorkspaceInput,
  ): Promise<CopiedContainerOutput> {
    return this.#containers.copyContainerToWorkspace(containerId, request);
  }
  async accountToday(): Promise<AccountToday> {
    const today = await this.#http.request<Schemas['AccountToday']>('/api/v1/users/me/today');
    // What the hub changes of these (a card done) is asked for under the space each is in.
    this.learnPlaces([
      ...today.cards.map((card) => ({ id: card.boardId, workspaceId: card.workspace.workspaceId })),
      ...today.units.map((unit) => ({
        id: unit.courseId,
        workspaceId: unit.workspace.workspaceId,
      })),
    ]);
    return today;
  }
  workspaceToday(workspaceId: string): Promise<WorkspaceToday> {
    return this.#http.request<Schemas['WorkspaceToday']>(underWorkspace('/today'), {
      workspaceId,
    });
  }

  categories(workspaceId: string): Promise<Category[]> {
    return this.#sidebar.categories(workspaceId);
  }
  createCategory(workspaceId: string, draft: CategoryDraft): Promise<Category> {
    return this.#sidebar.createCategory(workspaceId, draft);
  }
  patchCategory(workspaceId: string, id: string, patch: CategoryPatch): Promise<Category> {
    return this.#sidebar.patchCategory(workspaceId, id, patch);
  }
  deleteCategory(workspaceId: string, categoryId: string): Promise<void> {
    return this.#sidebar.deleteCategory(workspaceId, categoryId);
  }
  reorderCategories(workspaceId: string, categoryIds: string[]): Promise<void> {
    return this.#sidebar.reorderCategories(workspaceId, categoryIds);
  }
  moveContainerToCategory(w: string, id: string, c: string, index: number): Promise<void> {
    return this.#sidebar.moveContainerToCategory(w, id, c, index);
  }
  setContainerHidden(w: string, c: string, id: string, hidden: boolean): Promise<void> {
    return this.#sidebar.setContainerHidden(w, c, id, hidden);
  }

  views(boardId: string): Promise<BoardView[]> {
    return this.#blocks.views(boardId);
  }
  writeView(boardId: string, draft: ViewDraft): Promise<BoardViewWriteResult> {
    return this.#blocks.writeView(boardId, draft);
  }
  createView(boardId: string, draft: ViewDraft): Promise<BoardView> {
    return this.#blocks.createView(boardId, draft);
  }
  moveView(boardId: string, viewId: string, after: string | null): Promise<BoardViewWriteResult> {
    return this.#blocks.moveView(boardId, viewId, after);
  }
  duplicateView(
    boardId: string,
    viewId: string,
    title?: string,
    after?: string | null,
  ): Promise<BoardViewWriteResult> {
    return this.#blocks.duplicateView(boardId, viewId, title, after);
  }
  patchView(boardId: string, viewId: string, patch: BlockPatch): Promise<BoardView> {
    return this.#blocks.patchView(boardId, viewId, patch);
  }
  deleteView(boardId: string, viewId: string): Promise<void> {
    return this.#blocks.deleteView(boardId, viewId);
  }
  restoreView(boardId: string, viewId: string): Promise<void> {
    return this.#blocks.restoreView(boardId, viewId);
  }
  cards(boardId: string): Promise<CardSummary[]> {
    return this.#blocks.cards(boardId);
  }
  cardPreviews(boardId: string): Promise<Map<string, CardPreview>> {
    return this.#blocks.cardPreviews(boardId);
  }
  card(boardId: string, cardId: string): Promise<CardDetail> {
    return this.#blocks.card(boardId, cardId);
  }
  createCard(boardId: string, draft: CardDraft): Promise<CardSummary> {
    return this.#blocks.createCard(boardId, draft);
  }
  duplicateCard(boardId: string, cardId: string, asTemplate?: boolean): Promise<CardSummary> {
    return this.#blocks.duplicateCard(boardId, cardId, asTemplate);
  }
  moveViewCards(
    boardId: string,
    viewId: string,
    move: CardMovesInput,
    telling?: CardTelling,
  ): Promise<CardMovesOutput> {
    return this.#blocks.moveViewCards(boardId, viewId, move, telling);
  }
  batchPatchCards(b: string, ids: readonly string[], p: CardPropertiesPatch, t?: CardTelling) {
    return this.#bulk.batchPatchCards(b, ids, p, t);
  }
  batchDeleteCards(boardId: string, cardIds: readonly string[]): Promise<void> {
    return this.#blocks.batchDeleteCards(boardId, cardIds);
  }
  batchRestoreCards(boardId: string, cardIds: readonly string[]): Promise<CardSummary[]> {
    return this.#blocks.batchRestoreCards(boardId, cardIds);
  }
  patchCard(b: string, c: string, patch: CardPatch, telling?: CardTelling): Promise<CardSummary> {
    return this.#blocks.patchCard(b, c, patch, telling);
  }
  comments(boardId: string, cardId: string): Promise<Comment[]> {
    return this.#blocks.comments(boardId, cardId);
  }
  createComment(boardId: string, cardId: string, body: DocNode): Promise<Comment> {
    return this.#blocks.createComment(boardId, cardId, body);
  }
  patchComment(
    boardId: string,
    commentId: string,
    write: BodyWrite,
    cardId: string,
  ): Promise<Comment> {
    return this.#blocks.patchComment(boardId, commentId, write, cardId);
  }
  deleteComment(boardId: string, cardId: string, commentId: string): Promise<void> {
    return this.#blocks.deleteComment(boardId, cardId, commentId);
  }
  restoreComment(boardId: string, cardId: string, commentId: string): Promise<Comment> {
    return this.#blocks.restoreComment(boardId, cardId, commentId);
  }
  commentVersions(boardId: string, commentId: string, cardId: string): Promise<CommentVersion[]> {
    return this.#blocks.commentVersions(boardId, commentId, cardId);
  }
  attachments(boardId: string, cardId: string): Promise<Attachment[]> {
    return this.#blocks.attachments(boardId, cardId);
  }
  createAttachment(boardId: string, cardId: string, file: UploadedFile): Promise<Attachment> {
    return this.#blocks.createAttachment(boardId, cardId, file);
  }
  deleteAttachment(boardId: string, cardId: string, attachmentId: string): Promise<void> {
    return this.#blocks.deleteAttachment(boardId, cardId, attachmentId);
  }
  restoreAttachment(boardId: string, cardId: string, attachmentId: string): Promise<void> {
    return this.#blocks.restoreAttachment(boardId, cardId, attachmentId);
  }

  upload(containerId: string, file: File, options?: UploadOptions): Promise<UploadedFile> {
    const { workspaceId } = this.workspaceOf({ containerId });
    return this.#transfers.upload(workspaceId, containerId, file, options);
  }
  exportContainer(id: string, options?: DownloadOptions): Promise<DownloadedFile> {
    return this.#transfers.exportContainer(this.workspaceOf({ containerId: id }), options);
  }
  exportWorkspace(workspaceId: string, options?: DownloadOptions): Promise<DownloadedFile> {
    return this.#transfers.exportWorkspace(workspaceId, options);
  }
  importArchive(workspaceId: string, file: File, options?: ImportOptions) {
    return this.#transfers.importArchive(workspaceId, file, options);
  }

  watchContainer(containerId: string, listener: ChangeListener): () => void {
    return this.#live.watch(containerId, listener);
  }
  watchSelf(listener: ChangeListener): () => void {
    return this.#live.watch(undefined, listener);
  }
  openDoc(workspaceId: string, _boardId: string, cardId: string, hears: BodyDocHears): BodyDoc {
    return openRoomLater(this.#live.rooms.line(workspaceId, cardId), hears);
  }
  override enter(workspaceId: string): void {
    super.enter(workspaceId);
    this.#live.enter(workspaceId);
  }
}
