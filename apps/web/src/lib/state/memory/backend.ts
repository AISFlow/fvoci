/**
 * In-memory backend matching the server contract for tests and standalone mode.
 * Lists are scrambled (`scramble`) so screens cannot rely on backend order.
 * Helpers for tests: `as`, `failNext`, `latency`.
 */
import type { CardPreview } from '../../editor/card-preview.ts';
import type { FileDetails } from '../../editor/files.ts';
import type { DocNode } from '../../editor/schema.ts';
import type { NameChange } from '../../logic/person-names.ts';
import type {
  BlockPatch,
  BoardView,
  Container,
  ContainerMember,
  ContainerPatch,
} from '../../logic/wire.ts';
import {
  BackendError,
  type AccountSettings,
  type AccountSettingsPatch,
  type AccountToday,
  type Attachment,
  type Backend,
  type BoardDraft,
  type BoardViewWriteResult,
  type BodyDoc,
  type BodyDocHears,
  type BodyWrite,
  type CardDetail,
  type CardDraft,
  type CardMovesInput,
  type CardMovesOutput,
  type CardPatch,
  type CardSummary,
  type CardTelling,
  type Category,
  type CategoryDraft,
  type CategoryPatch,
  type ChangeListener,
  type Comment,
  type CommentVersion,
  type ContainerPermissions,
  type ContainerRole,
  type CopiedContainerOutput,
  type CopyToWorkspaceInput,
  type DeletedItem,
  type DownloadedFile,
  type DownloadOptions,
  type ImportOptions,
  type ImportSummary,
  type Paging,
  type PublicLink,
  type PublicLinkChange,
  type UploadedFile,
  type UploadOptions,
  type User,
  type UserOrganization,
  type ViewDraft,
  type WorkspaceInfo,
  type WorkspaceToday,
} from '../backend.ts';
import { BlockTurns } from '../write.ts';

import { changeName, keepSettings, settingsOf } from './account.ts';
import * as blockOps from './block-ops.ts';
import * as boardAuth from './board-authority.ts';
import { openMemoryRoom } from './bodies.ts';
import { MemoryBodyVersions } from './body-versions.ts';
import { MemoryCategories } from './categories.ts';
import { versionsOf } from './comments.ts';
import { memberFileUrl, memberRole, memoryPermissionsOf, reads } from './container-access.ts';
import * as containerOps from './container-ops.ts';
import { MemoryContentSearch } from './content-search.ts';
import { MemoryCourses } from './courses.ts';
import { MemoryDiaries } from './diaries.ts';
import { MemoryFileVersions } from './file-versions.ts';
import { readableBlob, readableFile, storeUpload } from './files.ts';
import { MemoryNotifications } from './notifications.ts';
import { pause } from './pause.ts';
import { MemoryPresence } from './presence.ts';
import { MemoryReferences } from './references.ts';
import { fail } from './refuse.ts';
import { nextScreenState } from './screen-state.ts';
import { searchBoardsOf } from './search.ts';
import { visibleTemplates } from './seed-templates.ts';
import { copy, LIMITS, memberOf, scramble } from './shapes.ts';
import * as spaces from './spaces.ts';
import { MemoryTime } from './time.ts';
import { deletedCardsOf, deletedContainersOf, mayManageCards } from './trash.ts';
import { MemoryWatches } from './unit-watch.ts';
import { userFound, usersAmong, usersNamed } from './users.ts';
import { MemoryWikis } from './wikis.ts';
import type { World } from './world.ts';

export { LIMITS, memberOf };

type MethodName = keyof Backend | 'writeCardBody';

export class MemoryBackend implements Backend {
  readonly world: World;
  readonly userId: string;
  readonly presence: MemoryPresence;
  readonly wiki = new MemoryWikis(this);
  readonly diary = new MemoryDiaries(this);
  readonly course = new MemoryCourses(this);
  readonly bodyVersions: MemoryBodyVersions;
  readonly search: MemoryContentSearch;
  readonly references = new MemoryReferences(this, () => this.#enter('references'));
  readonly fileVersions = new MemoryFileVersions(this, () => this.#enter('fileVersions'));
  readonly notifications = new MemoryNotifications(this, () => this.#enter('notifications'));
  readonly watches = new MemoryWatches(this, () => this.#enter('watches'));
  readonly time = new MemoryTime(this, () => this.#enter('time'));
  readonly #categories = new MemoryCategories(this, (method, workspaceId) =>
    this.#enterWorkspace(method, workspaceId),
  );
  latency = 0;
  readonly #failures = new Map<MethodName, BackendError>();
  readonly #viewTurns = new BlockTurns();

  constructor(world: World, userId: string) {
    this.world = world;
    this.userId = userId;
    this.presence = new MemoryPresence(world, userId);
    this.bodyVersions = new MemoryBodyVersions(world, userId, () => this.#enter('bodyVersions'));
    this.search = new MemoryContentSearch(world, userId, () => this.#enter('search'));
  }

  as(userId: string): MemoryBackend {
    if (!this.world.users.has(userId)) throw new Error(`no user ${userId}`);
    return new MemoryBackend(this.world, userId);
  }

  failNext(method: MethodName, error: BackendError): void {
    this.#failures.set(method, error);
  }

  async #enterWorkspace(method: MethodName, workspaceId: string): Promise<void> {
    await this.#enter(method);
    spaces.checkWorkspace(this.world, this.userId, workspaceId);
  }

  async #enter(method: MethodName, signal?: AbortSignal): Promise<void> {
    await pause(this.latency);
    if (signal?.aborted) throw new BackendError(0, { detail: 'the call was given up' });
    const failure = this.#failures.get(method);
    if (failure) {
      this.#failures.delete(method);
      throw failure;
    }
  }

  async me(): Promise<User> {
    await this.#enter('me');
    return copy(this.world.users.get(this.userId) ?? fail(401, 'no session'));
  }
  async patchMe(patch: NameChange): Promise<User> {
    await this.#enter('patchMe');
    return copy(changeName(this.world, this.userId, patch));
  }
  async confirmPassword(password: string): Promise<void> {
    await this.#enter('confirmPassword');
    spaces.confirmPassword(this.world, this.userId, password);
  }
  async workspaces(): Promise<WorkspaceInfo[]> {
    await this.#enter('workspaces');
    return spaces.workspacesOf(this.world, this.userId);
  }
  async organizations(): Promise<UserOrganization[]> {
    await this.#enter('organizations');
    return spaces.organizationsOf(this.world, this.userId);
  }
  async users(workspaceId: string): Promise<User[]> {
    await this.#enterWorkspace('users', workspaceId);
    return usersAmong(this.world, spaces.workersOf(this.world, workspaceId));
  }
  async usersById(workspaceId: string, userIds: string[]): Promise<User[]> {
    await this.#enterWorkspace('usersById', workspaceId);
    return usersNamed(this.world, this.userId, workspaceId, userIds);
  }
  async searchUsers(workspaceId: string, query: string): Promise<User[]> {
    await this.#enter('searchUsers');
    const wanted = query.trim().toLowerCase();
    return (await this.users(workspaceId))
      .filter((u) => userFound(u, wanted))
      .sort((a, b) => a.createdAt - b.createdAt)
      .slice(0, 10);
  }
  async settings(): Promise<AccountSettings> {
    await this.#enter('settings');
    return copy(settingsOf(this.world, this.userId));
  }
  async patchSettings(patch: AccountSettingsPatch): Promise<AccountSettings> {
    await this.#enter('patchSettings');
    return copy(keepSettings(this.world, this.userId, patch));
  }
  async screenState(): Promise<Record<string, string>> {
    await this.#enter('screenState');
    return { ...this.world.screenState.get(this.userId) };
  }
  async patchScreenState(updated: Record<string, string>, deleted: string[] = []): Promise<void> {
    await this.#enter('patchScreenState');
    const current = this.world.screenState.get(this.userId) ?? {};
    this.world.screenState.set(this.userId, nextScreenState(current, updated, deleted));
  }
  async boards(workspaceId: string): Promise<Container[]> {
    await this.#enterWorkspace('boards', workspaceId);
    const mine = [...this.world.containers.values()].filter(
      (b) =>
        b.workspaceId === workspaceId &&
        !b.isTemplate &&
        (b.kind ?? 'board') === 'board' &&
        reads(this.world, this.userId, b.id),
    );
    return scramble(mine).map(copy);
  }
  async trashRetentionDays(): Promise<number> {
    await this.#enter('trashRetentionDays');
    return 30;
  }
  async myPermissions(id: string): Promise<ContainerPermissions> {
    await this.#enter('myPermissions');
    return memoryPermissionsOf(this.world, this.userId, id);
  }
  async copyContainerToWorkspace(
    id: string,
    req: CopyToWorkspaceInput,
  ): Promise<CopiedContainerOutput> {
    await this.#enter('copyContainerToWorkspace');
    return containerOps.copyMemoryContainerToWorkspace(this, id, req);
  }
  async accountToday(): Promise<AccountToday> {
    await this.#enter('accountToday');
    return boardAuth.memoryAccountToday(this);
  }
  async workspaceToday(workspaceId: string): Promise<WorkspaceToday> {
    await this.#enterWorkspace('workspaceToday', workspaceId);
    return boardAuth.memoryWorkspaceToday(this, workspaceId);
  }
  async container(id: string): Promise<Container> {
    await this.#enter('container');
    return copy(containerOps.containerInWorld(this.world, id));
  }
  async createBoard(
    w: string,
    d: BoardDraft,
    v: ViewDraft,
  ): Promise<{ board: Container; view: BoardView }> {
    await this.#enterWorkspace('createBoard', w);
    return containerOps.createMemoryBoard(this, w, d, v);
  }
  async patchContainer(id: string, patch: ContainerPatch): Promise<Container> {
    await this.#enter('patchContainer');
    return containerOps.patchMemoryContainer(this, id, patch);
  }
  async reorderCardProperties(boardId: string, propertyIds: string[]): Promise<string[]> {
    await this.#enter('reorderCardProperties');
    return containerOps.reorderMemoryCardProperties(this, boardId, propertyIds);
  }
  async deleteContainer(id: string): Promise<void> {
    await this.#enter('deleteContainer');
    containerOps.deleteMemoryContainer(this, id);
  }
  async restoreContainer(id: string): Promise<Container> {
    await this.#enter('restoreContainer');
    return containerOps.restoreMemoryContainer(this, id);
  }
  async deletedContainers(workspaceId: string, paging?: Paging): Promise<DeletedItem[]> {
    await this.#enterWorkspace('deletedContainers', workspaceId);
    return deletedContainersOf(this.world, this.userId, workspaceId, paging);
  }
  async searchBoards(workspaceId: string, query: string): Promise<Container[]> {
    await this.#enterWorkspace('searchBoards', workspaceId);
    return searchBoardsOf(this.world, this.userId, workspaceId, query);
  }
  async templates(workspaceId: string): Promise<Container[]> {
    await this.#enterWorkspace('templates', workspaceId);
    return visibleTemplates(this.world, this.userId, workspaceId).map(copy);
  }
  async duplicateContainer(id: string, asTemplate: boolean, to: string): Promise<Container> {
    await this.#enterWorkspace('duplicateContainer', to);
    return containerOps.duplicateMemoryContainer(this, id, asTemplate, to);
  }
  async onboard(workspaceId: string): Promise<string> {
    await this.#enterWorkspace('onboard', workspaceId);
    return containerOps.onboardMemoryWorkspace(this, workspaceId);
  }

  async exportContainer(id: string, options?: DownloadOptions): Promise<DownloadedFile> {
    await this.#enter('exportContainer', options?.signal);
    return containerOps.exportMemoryContainer(this, id, options);
  }
  async exportWorkspace(workspaceId: string, options?: DownloadOptions): Promise<DownloadedFile> {
    await this.#enter('exportWorkspace', options?.signal);
    return containerOps.exportMemoryWorkspace(this, workspaceId, options);
  }
  async importArchive(w: string, file: File, options?: ImportOptions): Promise<ImportSummary> {
    await this.#enter('importArchive', options?.signal);
    return containerOps.importMemoryArchive(this, w, file, options);
  }

  async joinContainer(id: string): Promise<ContainerMember> {
    await this.#enter('joinContainer');
    return containerOps.joinMemoryContainer(this, id);
  }
  async members(id: string): Promise<ContainerMember[]> {
    await this.#enter('members');
    containerOps.containerInWorld(this.world, id, 'shared');
    return scramble(this.world.members.get(id) ?? []).map(copy);
  }
  async addMember(id: string, userId: string, role: ContainerRole): Promise<ContainerMember> {
    await this.#enter('addMember');
    return containerOps.addMemoryMember(this, id, userId, role);
  }
  async setMemberRole(id: string, userId: string, role: ContainerRole): Promise<ContainerMember> {
    await this.#enter('setMemberRole');
    return containerOps.setMemoryMemberRole(this, id, userId, role);
  }
  async removeMember(id: string, userId: string): Promise<void> {
    await this.#enter('removeMember');
    containerOps.removeMemoryMember(this, id, userId);
  }
  async publicLink(id: string): Promise<PublicLink> {
    await this.#enter('publicLink');
    containerOps.containerInWorld(this.world, id, 'shared');
    return copy(this.world.publicLinks.get(id) ?? { enabled: false, token: '' });
  }
  async setPublicLink(id: string, change: PublicLinkChange): Promise<PublicLink> {
    await this.#enter('setPublicLink');
    return containerOps.setMemoryPublicLink(this, id, change);
  }

  categories(workspaceId: string): Promise<Category[]> {
    return this.#categories.categories(workspaceId);
  }
  createCategory(workspaceId: string, draft: CategoryDraft): Promise<Category> {
    return this.#categories.createCategory(workspaceId, draft);
  }
  patchCategory(workspaceId: string, categoryId: string, patch: CategoryPatch): Promise<Category> {
    return this.#categories.patchCategory(workspaceId, categoryId, patch);
  }
  deleteCategory(workspaceId: string, categoryId: string): Promise<void> {
    return this.#categories.deleteCategory(workspaceId, categoryId);
  }
  reorderCategories(workspaceId: string, categoryIds: string[]): Promise<void> {
    return this.#categories.reorderCategories(workspaceId, categoryIds);
  }
  moveContainerToCategory(w: string, id: string, c: string, index: number): Promise<void> {
    return this.#categories.moveContainerToCategory(w, id, c, index);
  }
  setContainerHidden(w: string, c: string, id: string, hidden: boolean): Promise<void> {
    return this.#categories.setContainerHidden(w, c, id, hidden);
  }

  async views(boardId: string): Promise<BoardView[]> {
    await this.#enter('views');
    return blockOps.listMemoryViews(this, boardId);
  }
  async createView(boardId: string, draft: ViewDraft): Promise<BoardView> {
    await this.#enter('createView');
    return blockOps.createMemoryView(this, boardId, draft);
  }
  async writeView(boardId: string, draft: ViewDraft): Promise<BoardViewWriteResult> {
    await this.#enter('writeView');
    return boardAuth.writeMemoryView(this, boardId, draft);
  }
  async moveView(b: string, v: string, after: string | null): Promise<BoardViewWriteResult> {
    await this.#enter('moveView');
    return boardAuth.moveMemoryView(this, b, v, after);
  }
  async duplicateView(
    b: string,
    v: string,
    title?: string,
    after?: string | null,
  ): Promise<BoardViewWriteResult> {
    await this.#enter('duplicateView');
    return boardAuth.duplicateMemoryView(this, b, v, title, after);
  }
  patchView(boardId: string, viewId: string, patch: BlockPatch): Promise<BoardView> {
    return this.#viewTurns.patchView(viewId, patch, async (sent) => {
      await this.#enter('patchView');
      return blockOps.patchMemoryView(this, boardId, viewId, sent);
    });
  }
  async cards(boardId: string): Promise<CardSummary[]> {
    await this.#enter('cards');
    return blockOps.listMemoryCards(this, boardId);
  }
  async cardPreviews(boardId: string): Promise<Map<string, CardPreview>> {
    await this.#enter('cardPreviews');
    return blockOps.listMemoryCardPreviews(this, boardId);
  }
  async card(boardId: string, cardId: string): Promise<CardDetail> {
    await this.#enter('card');
    return blockOps.readMemoryCard(this, boardId, cardId);
  }
  async createCard(boardId: string, draft: CardDraft): Promise<CardSummary> {
    await this.#enter('createCard');
    return blockOps.createMemoryCard(this, boardId, draft);
  }
  async duplicateCard(boardId: string, cardId: string, asTemplate?: boolean): Promise<CardSummary> {
    await this.#enter('duplicateCard');
    return boardAuth.duplicateMemoryCard(this, boardId, cardId, asTemplate);
  }
  async moveViewCards(b: string, v: string, move: CardMovesInput): Promise<CardMovesOutput> {
    await this.#enter('moveViewCards');
    return boardAuth.moveMemoryViewCards(this, b, v, move);
  }
  async batchDeleteCards(boardId: string, cardIds: readonly string[]): Promise<void> {
    await this.#enter('batchDeleteCards');
    boardAuth.batchDeleteMemoryCards(this, boardId, cardIds);
  }
  async batchRestoreCards(boardId: string, cardIds: readonly string[]): Promise<CardSummary[]> {
    await this.#enter('batchRestoreCards');
    return boardAuth.batchRestoreMemoryCards(this, boardId, cardIds);
  }
  async patchCard(b: string, c: string, patch: CardPatch, _t?: CardTelling): Promise<CardSummary> {
    await this.#enter('patchCard');
    return blockOps.patchMemoryCard(this, b, c, patch);
  }
  async writeCardBody(boardId: string, cardId: string, write: BodyWrite): Promise<CardDetail> {
    await this.#enter('writeCardBody');
    return blockOps.writeMemoryCardBody(this, boardId, cardId, write);
  }
  async comments(boardId: string, cardId: string): Promise<Comment[]> {
    await this.#enter('comments');
    return blockOps.listMemoryComments(this, boardId, cardId);
  }
  async createComment(boardId: string, cardId: string, body: DocNode): Promise<Comment> {
    await this.#enter('createComment');
    return blockOps.createMemoryComment(this, boardId, cardId, body);
  }
  async patchComment(b: string, c: string, write: BodyWrite, _cardId?: string): Promise<Comment> {
    await this.#enter('patchComment');
    return blockOps.patchMemoryComment(this, b, c, write);
  }
  async deleteComment(b: string, cardId: string, c: string): Promise<void> {
    await this.#enter('deleteComment');
    boardAuth.deleteMemoryComment(this, b, cardId, c);
  }
  async restoreComment(b: string, cardId: string, c: string): Promise<Comment> {
    await this.#enter('restoreComment');
    return boardAuth.restoreMemoryComment(this, b, cardId, c);
  }
  async commentVersions(b: string, c: string, _cardId?: string): Promise<CommentVersion[]> {
    await this.#enter('commentVersions');
    memberRole(this.world, this.userId, b);
    return versionsOf(this.world, b, c);
  }
  async attachments(boardId: string, cardId: string): Promise<Attachment[]> {
    await this.#enter('attachments');
    return blockOps.listMemoryAttachments(this, boardId, cardId);
  }
  async createAttachment(b: string, c: string, file: UploadedFile): Promise<Attachment> {
    await this.#enter('createAttachment');
    return blockOps.createMemoryAttachment(this, b, c, file);
  }

  async deleteBlock(boardId: string, blockId: string): Promise<void> {
    await this.#enter('deleteBlock');
    blockOps.deleteMemoryBlock(this, boardId, blockId);
  }
  async restoreBlock(boardId: string, blockId: string): Promise<void> {
    await this.#enter('restoreBlock');
    blockOps.restoreMemoryBlock(this, boardId, blockId);
  }
  async deletedCards(boardId: string, paging?: Paging): Promise<DeletedItem[]> {
    await this.#enter('deletedCards');
    mayManageCards(this.world, this.userId, boardId, 'access denied to the trash of the board');
    return deletedCardsOf(this.world, boardId, paging);
  }

  async upload(id: string, file: File, options: UploadOptions = {}): Promise<UploadedFile> {
    await this.#enter('upload', options.signal);
    containerOps.containerInWorld(this.world, id);
    return storeUpload(this, id, file, options, (signal) => this.#enter('upload', signal));
  }
  async fileBytes(id: string, fileId: string): Promise<Blob> {
    await this.#enter('fileBytes');
    return readableBlob(this.world, this.userId, id, fileId);
  }
  async fileDetails(id: string, fileId: string): Promise<FileDetails> {
    await this.#enter('fileDetails');
    const { name, size } = readableFile(this.world, this.userId, id, fileId);
    return { name, size };
  }
  fileUrl(id: string, fileId: string): string | undefined {
    return memberFileUrl(this.world, this.userId, id, fileId);
  }
  watchContainer(containerId: string, listener: ChangeListener): () => void {
    return this.world.watch({ containerId, userId: this.userId, listener });
  }
  watchSelf(listener: ChangeListener): () => void {
    return this.world.watch({ containerId: undefined, userId: this.userId, listener });
  }
  openDoc(_workspaceId: string, containerId: string, cardId: string, hears: BodyDocHears): BodyDoc {
    return openMemoryRoom(this.world, this, containerId, cardId, hears);
  }
}
