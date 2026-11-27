/**
 * The data boundary: everything a screen can read, write and listen to.
 * Screens use stores, and stores use one `Backend` (`lib/api/` or `memory/`).
 */
import type { CardPreview } from '../editor/card-preview.ts';
import type { FileDetails } from '../editor/files.ts';
import type { DocNode } from '../editor/schema.ts';
import type { AccountSettings, AccountSettingsPatch } from '../logic/account-settings.ts';
import type { NameChange, PersonName } from '../logic/person-names.ts';
import type { UserNames } from '../logic/users.ts';
import type {
  Block,
  BlockPatch,
  Container,
  ContainerMember,
  ContainerPatch,
  BoardView,
  Card,
} from '../logic/wire.ts';

import type { PresenceOfContainers } from './presence.ts';
import type { DocRooms } from './body-doc.ts';
import type { VersionsOfFiles } from './file-versions.ts';
import type { OfBodies } from './references.ts';
import type { TrashReads } from './trash-reads.ts';
import type { WatchesOfBackend } from './watch-types.ts';
import type { ChangeListener } from './changes.ts';
import type { ContentSearchOfBackend } from './content-search.ts';
import type { Category, CategoryDraft, CategoryPatch } from './category-types.ts';
import type { NotificationsOfBackend } from './notification-types.ts';
import type { WikisOfBackend } from './wiki-types.ts';
import type { DiariesOfBackend } from './diary-types.ts';
import type { CoursesOfBackend } from './course-types.ts';
import type { ImportOptions, ImportSummary } from './import-types.ts';
import type { TimeOfBackend } from './time-types.ts';
import type { ServerAuthorityOfBackend } from './authority-types.ts';
import type { UserOrganization, WorkspaceInfo } from './workspace-types.ts';

export * from './authority-types.ts';
export type * from './account-types.ts';
export type { DeletedItem, Paging, TrashReads } from './trash-reads.ts';
export * from './content-search.ts';
export type * from './notification-types.ts';
export type { BodyDoc, BodyDocHears, BodyDocState, BodyDocStatus, DocRooms } from './body-doc.ts';
export { isCardBlock, isViewBlock, type Change, type ChangeListener } from './changes.ts';
export type * from './category-types.ts';
export type * from './wiki-types.ts';
export type * from './diary-types.ts';
export type * from './course-types.ts';
export {
  isOwnSpace,
  type OrgKind,
  type OrgRole,
  type UserOrganization,
  type WorkspaceInfo,
} from './workspace-types.ts';

export type * from './import-types.ts';

// ----------------------------------------------------------------- errors

export * from './problems.ts';

// ------------------------------------------------------------------ model

export interface User extends UserNames {
  id: string;
  /** Of the reader's own user only where the backend has accounts: nobody is told another's. */
  email: string;
  /**
   * Only on the reader's own user: the address the account waits to change
   * to, until the link mailed there is opened (`Account.emailChange`).
   */
  pendingEmail?: string | undefined;
  /**
   * The parts of the name, which of them the user is shown by
   * (`displayName` is made of them), the title and the credentials: whoever
   * sees the user sees these. Left out where nothing told them.
   */
  name?: PersonName;
  /** Only on the reader's own user. */
  isAdmin?: boolean;
  /**
   * Only on the reader's own user, where the backend has accounts: false for
   * one made through an organization's provider until its person sets one.
   */
  hasPassword?: boolean;
  createdAt: number;
  updatedAt: number;
  /**
   * Present when the account waits to be deleted (or is deactivated); omitted while in use.
   * An account that was erased has an empty `username`, `createdAt` and `updatedAt` 0, and no `deletedAt`.
   */
  deletedAt?: number;
}

/**
 * The reader's account settings: the time zone, the first day of the week
 * and the hour cycle, the four of the display (theme, density, how a card
 * opens, haptics), the language of the interface, and what the reader is
 * told of. The answer always holds every member.
 */
export type { AccountSettings, AccountSettingsPatch };

/** A role on a board: one of them, the highest a member has. */
export type ContainerRole = 'admin' | 'editor' | 'commenter' | 'viewer';

/**
 * A board's public link: whether it is on, and the token a reader without
 * an account reads the board with (empty while it is off and was never on).
 */
export interface PublicLink {
  enabled: boolean;
  token: string;
}

export interface PublicLinkChange {
  enabled: boolean;
  /** Makes a new token: the old link stops working. */
  regenerate?: boolean;
}

/** A card as a list has it: no body, but whether there is one and how its tasks stand. */
export interface CardSummary extends Card {
  hasBody: boolean;
  tasksTotal: number;
  tasksDone: number;
  /**
   * What a gallery shows of the body: only on a card that came in a change
   * message, whose summary of the body carries it (`cardPreviews` reads it
   * for a list).
   */
  preview?: CardPreview;
}

/** A card read on its own: with its body. */
export interface CardDetail extends CardSummary {
  body: DocNode | null;
}

export interface Comment extends Block {
  type: 'comment';
  body: DocNode | null;
  /** When its text was last changed; absent while it never was. Who changed it is `modifiedBy`. */
  editedAt?: number;
}

/** A text a comment had before it was changed: the first is the one it was posted with. */
export interface CommentVersion {
  /** Counted from 1 for the first text; versions no longer kept leave gaps. */
  version: number;
  body: DocNode | null;
  writtenBy: string;
  writtenAt: number;
}

/** A file attached to a card. It is a child block of the card; `fileId` names the bytes. */
export interface Attachment extends Block {
  type: 'attachment';
  fileId: string;
  name: string;
  size: number;
  contentType: string;
}

/** What a card is made from. The server gives it its identifier and times. */
export interface CardDraft {
  parentId?: string;
  title?: string;
  icon?: string;
  properties?: Card['fields']['properties'];
  isTemplate?: boolean;
  body?: DocNode | null;
  /**
   * Said before (a copy, a card made again by redo): it tells nobody whom it
   * names or gives the card to, and makes nobody a watcher but its writer.
   */
  quiet?: boolean;
}

/** How a change of a card tells of itself. */
export interface CardTelling {
  /**
   * Said before (a change taken back by undo, or made again by redo): it
   * tells nobody whom it gives the card to, nor makes them a watcher.
   */
  quiet?: boolean;
}

export interface CardPatch {
  parentId?: string | null;
  title?: string;
  icon?: string;
  /** Properties to set. */
  updatedProperties?: Card['fields']['properties'];
  /** Properties to clear. */
  deletedProperties?: string[];
}

export interface ViewDraft {
  title: string;
  fields: BoardView['fields'];
  after?: string | null;
}

export interface BoardDraft {
  title: string;
  description?: string;
  icon?: string;
  visibility?: Container['visibility'];
  cardProperties?: Container['cardProperties'];
  scaffold?: boolean;
}

/** A write of a body: the document and the version it was based on. */
export interface BodyWrite {
  body: DocNode | null;
  /** The `updatedAt` of the comment as the writer read it. */
  readAt: number;
}

export interface UploadProgress {
  sentBytes: number;
  totalBytes: number;
}

export interface UploadOptions {
  onProgress?: (progress: UploadProgress) => void;
  /** Aborting gives up the upload; what was sent is discarded by the server. */
  signal?: AbortSignal;
}

export interface DownloadOptions {
  /** `sentBytes` counts what has arrived; `totalBytes` is 0 when the length is not known. */
  onProgress?: (progress: UploadProgress) => void;
  signal?: AbortSignal;
}

/** Bytes to be saved, under the name they come with. */
export interface DownloadedFile {
  bytes: Blob;
  name: string;
}

export interface UploadedFile {
  fileId: string;
  name: string;
  size: number;
  contentType: string;
}

// ---------------------------------------------------------------- backend

export interface Backend
  extends
    DocRooms,
    TrashReads,
    PresenceOfContainers,
    OfBodies,
    WikisOfBackend,
    DiariesOfBackend,
    CoursesOfBackend,
    ContentSearchOfBackend,
    NotificationsOfBackend,
    WatchesOfBackend,
    TimeOfBackend,
    VersionsOfFiles,
    ServerAuthorityOfBackend {
  // The reader and the people around them.
  me(): Promise<User>;
  /**
   * Changes what the patch names of the reader's name, as the server reads it
   * (`patchMe`); the user as they are now, `displayName` made anew.
   */
  patchMe(patch: NameChange): Promise<User>;
  /**
   * Gives the account's password in the session the reader has: their own
   * space, closed to it until then (`passwordRequired`), opens to it for as
   * long as it lasts, and it is the same session to everything else. A wrong
   * password is 400 `invalid_credentials`, too many tries 429, an account
   * without a password 409.
   */
  confirmPassword(password: string): Promise<void>;
  workspaces(): Promise<WorkspaceInfo[]>;
  /** Shared organizations the reader is a member of, including ones with no workspaces left. */
  organizations(): Promise<UserOrganization[]>;
  /** The people of a workspace: who can be mentioned or set as a person. */
  users(workspaceId: string): Promise<User[]>;
  /**
   * The people of a workspace with these identifiers: for the names `users`
   * did not bring (the server's list of a workspace is short). Whom the reader
   * does not see there is left out, as one who does not exist.
   */
  usersById(workspaceId: string, userIds: string[]): Promise<User[]>;

  // Account settings and the pocket of screen state.
  settings(): Promise<AccountSettings>;
  patchSettings(patch: AccountSettingsPatch): Promise<AccountSettings>;
  /** Small things a screen wants to find again on another device. Keys and values are text. */
  screenState(): Promise<Record<string, string>>;
  patchScreenState(updated: Record<string, string>, deleted?: string[]): Promise<void>;

  // Boards.
  boards(workspaceId: string): Promise<Container[]>;
  container(containerId: string): Promise<Container>;
  /**
   * Makes a board and its first view together, all or none: a board without
   * a view has nothing to show. The reader becomes its admin, and a board
   * that is no template is filed under their default category.
   */
  createBoard(
    workspaceId: string,
    draft: BoardDraft,
    firstView: ViewDraft,
  ): Promise<{ board: Container; view: BoardView }>;
  patchContainer(containerId: string, patch: ContainerPatch): Promise<Container>;
  /**
   * Puts the board's card properties in an order: every property once, or
   * 400 and nothing changes. Who may change the properties may (editors).
   * The board's listeners are told the board.
   */
  reorderCardProperties(boardId: string, propertyIds: string[]): Promise<string[]>;
  /**
   * The templates the reader may make a board from: the workspace's they may
   * see, then the built-in ones every workspace is offered.
   */
  templates(workspaceId: string): Promise<Container[]>;
  /**
   * Copies a board with its views and cards (not its comments) into the
   * workspace: a board from a template, or a template from a board. The reader is
   * the copy's admin, and it is private. The workspace is said because a built-in
   * template belongs to none of the reader's.
   */
  duplicateContainer(
    containerId: string,
    asTemplate: boolean,
    workspaceId: string,
  ): Promise<Container>;
  /** Copies the welcome board for the reader; answers its identifier. Each call makes another copy. */
  onboard(workspaceId: string): Promise<string>;
  /**
   * The boards the reader may find whose title has every word of `query` in
   * it, whatever the case: those of `boards`, hidden ones too. No templates.
   */
  searchBoards(workspaceId: string, query: string): Promise<Container[]>;
  /** Moves the board to the trash. */
  deleteContainer(containerId: string): Promise<void>;
  /** Takes the board back out of the trash, filed where each person had it (`TrashReads`). */
  restoreContainer(containerId: string): Promise<Container>;
  members(containerId: string): Promise<ContainerMember[]>;
  /**
   * Makes a person of the workspace a member of the board, with the role. For the
   * board's admins. Someone who is a member already keeps their role.
   */
  addMember(containerId: string, userId: string, role: ContainerRole): Promise<ContainerMember>;
  /** Gives a member another role. The board's last admin stays one (409, `last_admin`). */
  setMemberRole(containerId: string, userId: string, role: ContainerRole): Promise<ContainerMember>;
  /** Takes a member off the board. Not its last admin (409, `last_admin`). */
  removeMember(containerId: string, userId: string): Promise<void>;
  /** The board's public link. For the board's admins. */
  publicLink(containerId: string): Promise<PublicLink>;
  /** Turns the public link on or off, or makes it anew. */
  setPublicLink(containerId: string, change: PublicLinkChange): Promise<PublicLink>;
  /** People of the workspace whose name has `query` in it, whatever the case; ten at most. */
  searchUsers(workspaceId: string, query: string): Promise<User[]>;
  /**
   * Makes the reader a member of an open board of their workspace, with the
   * board's minimum role. Until then such a board is readable but its views,
   * cards and members are refused (403).
   */
  joinContainer(containerId: string): Promise<ContainerMember>;

  // The reader's sidebar.
  categories(workspaceId: string): Promise<Category[]>;
  /** A new category goes first. */
  createCategory(workspaceId: string, draft: CategoryDraft): Promise<Category>;
  /** The default category keeps its name whatever the patch says. */
  patchCategory(workspaceId: string, categoryId: string, patch: CategoryPatch): Promise<Category>;
  /** Its boards go to the default category, which cannot be deleted. */
  deleteCategory(workspaceId: string, categoryId: string): Promise<void>;
  /** The reader's order of all their categories. */
  reorderCategories(workspaceId: string, categoryIds: string[]): Promise<void>;
  /**
   * Files a board under a category, at `index` in the category's own list
   * (`Category.containerIds`, without the board).
   */
  moveContainerToCategory(
    workspaceId: string,
    containerId: string,
    categoryId: string,
    index: number,
  ): Promise<void>;
  /** Hides a board of a category from the reader's sidebar, or shows it again. */
  setContainerHidden(
    workspaceId: string,
    categoryId: string,
    containerId: string,
    hidden: boolean,
  ): Promise<void>;

  // Views.
  views(boardId: string): Promise<BoardView[]>;
  createView(boardId: string, draft: ViewDraft): Promise<BoardView>;
  patchView(boardId: string, viewId: string, patch: BlockPatch): Promise<BoardView>;

  // Cards.
  cards(boardId: string): Promise<CardSummary[]>;
  /**
   * What a gallery shows of each card's body, by card: the first image and
   * the beginning of the text (`getCards` with `preview`). Asked only for a
   * gallery that is shown.
   */
  cardPreviews(boardId: string): Promise<Map<string, CardPreview>>;
  card(boardId: string, cardId: string): Promise<CardDetail>;
  createCard(boardId: string, draft: CardDraft): Promise<CardSummary>;
  patchCard(
    boardId: string,
    cardId: string,
    patch: CardPatch,
    telling?: CardTelling,
  ): Promise<CardSummary>;

  // Comments and attachments of a card.
  comments(boardId: string, cardId: string): Promise<Comment[]>;
  createComment(boardId: string, cardId: string, body: DocNode): Promise<Comment>;
  /**
   * Replaces the text of a comment. Rejects with 409 when the comment changed
   * since `readAt`, and with 400 when the body is empty: a comment has a text.
   */
  patchComment(
    boardId: string,
    commentId: string,
    write: BodyWrite,
    cardId?: string,
  ): Promise<Comment>;
  /**
   * What a comment said before each change, the first text first; empty for
   * one never changed. For the board's members (a reader by its link has none).
   */
  commentVersions(boardId: string, commentId: string, cardId?: string): Promise<CommentVersion[]>;
  attachments(boardId: string, cardId: string): Promise<Attachment[]>;
  createAttachment(boardId: string, cardId: string, file: UploadedFile): Promise<Attachment>;

  // Any block of a board: a view, a card, a comment, an attachment.
  /** Moves the block and everything under it to the trash. */
  deleteBlock(boardId: string, blockId: string): Promise<void>;
  /** Takes the block and what was deleted with it back out of the trash. */
  restoreBlock(boardId: string, blockId: string): Promise<void>;

  // Files.
  /**
   * Uploads a file of a board. The server takes it in parts (an upload
   * session); the parts and the retrying of one are the implementation's
   * business.
   */
  upload(containerId: string, file: File, options?: UploadOptions): Promise<UploadedFile>;
  /** The bytes of a file; `signal` gives the reading up. */
  fileBytes(containerId: string, fileId: string, signal?: AbortSignal): Promise<Blob>;
  /** The name and size of a file of the board or wiki (404 for none of it). */
  fileDetails(containerId: string, fileId: string): Promise<FileDetails>;
  /** Where the bytes of a file can be fetched by an `<img>` or a link. */
  fileUrl(containerId: string, fileId: string): string | undefined;

  // Archives of boards.
  /** An archive of the board with its blocks, files and members. For its members. */
  exportContainer(containerId: string, options?: DownloadOptions): Promise<DownloadedFile>;
  /** An archive of every board of the workspace the reader may see. */
  exportWorkspace(workspaceId: string, options?: DownloadOptions): Promise<DownloadedFile>;
  /**
   * Makes the boards of an archive in the workspace, the reader their admin. An
   * archive over the server's limit is refused before anything is sent (413,
   * with the limit); one that is no archive of this app is 400. The boards
   * are made one after the other: a failure keeps those before it. Members of
   * the boards who are not in the workspace are left out; it says how many.
   */
  importArchive(workspaceId: string, file: File, options?: ImportOptions): Promise<ImportSummary>;

  // Changes as they happen.
  /** Listens to the changes of one board or wiki. Returns the function that stops listening. */
  watchContainer(containerId: string, listener: ChangeListener): () => void;
  /**
   * Listens to the changes of the reader's own things: the boards they can
   * see and the sidebar of the workspace entered (`enter`), and their settings.
   */
  watchSelf(listener: ChangeListener): () => void;
  /**
   * The workspace the reader works in now, as the address names it: the
   * changes of its boards and sidebar are the ones `watchSelf` hears. A
   * backend that hears every workspace's (the one in memory) has none.
   */
  enter?(workspaceId: string): void;
}
