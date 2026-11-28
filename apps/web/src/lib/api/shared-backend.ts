/**
 * The backend of a reader by a public link: a board's views, cards, comments
 * and files, or a wiki's tree, pages and files, read by
 * `/api/v1/shared/{containerId}` with the link's read token alone. No session goes along (`credentials: 'omit'`): a
 * reader who is signed in reads as anyone else does, and nothing of their
 * account mixes in. Nothing is written: every change is refused here,
 * before a request is made. A visit connection counts this reader anonymously;
 * content is read again to see what is new, and no presence roster is received.
 *
 * The board is the only one of a place of its own (`SHARED_PLACE`), with
 * nobody in it: the server names no person to such a reader.
 */
import { SharedPresence } from './shared-presence.ts';
import type { BoardView } from '../logic/wire.ts';
import { DEFAULT_ACCOUNT_SETTINGS } from '../logic/account-settings.ts';
import {
  BackendError,
  READ_ONLY_PERMISSIONS,
  type AccountSettings,
  type Attachment,
  type Backend,
  type CardDetail,
  type CardSummary,
  type Comment,
  type User,
  type WorkspaceInfo,
} from '../state/backend.ts';
import type { CardPreview } from '../editor/card-preview.ts';
import type { FileDetails } from '../editor/files.ts';
import { isFileId } from '../editor/schema.ts';
import { pagesTogether } from '../state/page-reads.ts';
import { summaryOfBody } from '../state/body-summary.ts';
import type { Courses } from '../state/course-types.ts';
import type { LinkedBlock } from '../state/references.ts';
import type { Wikis } from '../state/wiki-types.ts';
import type { Container } from '../logic/wire.ts';

import {
  attachmentOf,
  containerOf,
  bodyOf,
  cardOf,
  cardOfBlock,
  commentOf,
  previewsOf,
  viewOf,
  type Schemas,
} from './convert.ts';
import { unitDetailOf, unitOf } from './courses.ts';
import { Http, type ApiPath } from './http.ts';
import { SHARED_UNIT_REVIEWS } from './unit-reviews.ts';
import { pageDetailOf, pageOf } from './wikis.ts';

/** The place the shared board is shown in: no workspace of anybody's. */
export const SHARED_PLACE = 'shared';

/** The reader by the link: nobody. */
const READER: User = {
  id: '',
  username: '',
  displayName: '',
  email: '',
  createdAt: 0,
  updatedAt: 0,
};

/** A change, which a reader by the link does not make. */
function refused(): Promise<never> {
  return Promise.reject(
    new BackendError(403, { detail: 'a reader by the link reads only', reason: 'read_only' }),
  );
}

const nothing = (): Promise<never[]> => Promise.resolve([]);
const done = (): Promise<void> => Promise.resolve();
const unwatched = (): (() => void) => () => undefined;

/** A path of the shared board's, as the contract writes it after `/api/v1/shared/{containerId}`. */
type SharedPath = ApiPath extends infer P
  ? P extends `/api/v1/shared/{containerId}${infer Rest}`
    ? Rest
    : never
  : never;

export class SharedBackend implements Backend {
  readonly presence: Backend['presence'];
  /** No versions through a public link: they are for the board's members. */
  readonly bodyVersions: Backend['bodyVersions'] = {
    list: nothing,
    read: refused,
    restore: refused,
    keep: refused,
  };
  /** Nobody is told anything through a public link. */
  readonly notifications = undefined;
  /** What was done to a card is for the board's members, not a reader by the link. */
  readonly cardActivity = undefined;
  readonly watches = undefined;
  readonly favorites = undefined;
  /** Nothing is taken out in Markdown through a public link: the board is read where it is. */
  readonly markdown = undefined;
  /** A reaction names who reacted, which a reader by the link is not told. */
  readonly reactions = undefined;
  /** Personal time tracking is strictly owner-only and never available on public links. */
  readonly time = undefined;
  /** No search through a public link: the board is all there is to read. */
  readonly search: Backend['search'] = {
    enabled: () => Promise.resolve(false),
    find: nothing,
  };
  /**
   * What a body names is told of the board that was received and of nothing
   * else: the server gave the link's reader no identifier of anything else
   * (it empties them), and takes no question about references from them.
   */
  readonly references: Backend['references'] = {
    resolve: async (_workspaceId, ids) => {
      const cards = await this.cards(this.#containerId);
      return cards.filter((card) => ids.includes(card.id)).map((card) => this.#linked(card));
    },
    to: nothing,
    search: nothing,
    numbered: () => Promise.resolve(undefined),
  };
  /**
   * A wiki's link reads its tree and its pages, naming nobody, and nothing
   * else of a wiki (no comments, no versions, no trash, no other wiki);
   * nothing is changed.
   */
  readonly wiki: Wikis = {
    wikis: nothing,
    pages: async (wikiId) => {
      this.#only(wikiId);
      const nodes = await this.#read<Schemas['PageNode'][]>('/pages');
      return nodes.map((node) => pageOf(node, wikiId));
    },
    page: async (wikiId, pageId) => {
      this.#only(wikiId);
      return pageDetailOf(await this.#read<Schemas['Page']>('/pages/{pageId}', { pageId }));
    },
    comments: nothing,
    createComment: refused,
    patchComment: refused,
    deleteComment: refused,
    restoreComment: refused,
    commentVersions: nothing,
    bodyVersions: this.bodyVersions,
    deletedPages: nothing,
    createWiki: refused,
    createPage: refused,
    patchPage: refused,
    deletePage: refused,
    restorePage: refused,
    movePage: refused,
    duplicatePage: refused,
  };
  /**
   * A course's link reads its outline and its units, naming nobody, and nothing
   * else of a course (no comments, no versions, no subscriptions, no trash, no
   * other course); nothing is changed.
   */
  readonly course: Courses = {
    courses: nothing,
    units: async (courseId) => {
      this.#only(courseId);
      const nodes = await this.#read<Schemas['UnitNode'][]>('/units');
      return nodes.map((node) => unitOf(node, courseId));
    },
    unit: async (courseId, unitId) => {
      this.#only(courseId);
      return unitDetailOf(await this.#read<Schemas['Unit']>('/units/{unitId}', { unitId }));
    },
    comments: nothing,
    createComment: refused,
    patchComment: refused,
    deleteComment: refused,
    restoreComment: refused,
    commentVersions: nothing,
    bodyVersions: this.bodyVersions,
    watching: nothing,
    watchUnit: refused,
    unwatchUnit: refused,
    deletedUnits: nothing,
    createCourse: refused,
    createUnit: refused,
    patchUnit: refused,
    deleteUnit: refused,
    restoreUnit: refused,
    moveUnit: refused,
    myProgress: nothing,
    putUnitProgress: refused,
    progressSummary: () => Promise.resolve({ totalUnits: 0, members: [] }),
    reviews: SHARED_UNIT_REVIEWS,
    schedule: () =>
      Promise.resolve({
        schedule: null,
        today: '1970-01-01',
        todayCount: 0,
        overdueCount: 0,
        units: [],
      }),
    putSchedule: refused,
    deleteSchedule: refused,
    redistributeSchedule: refused,
    redistributeMySchedule: refused,
    putUnitSchedule: refused,
    learningToday: () => Promise.resolve({ today: '1970-01-01', todayUnits: [], overdueUnits: [] }),
    meetings: {
      list: nothing,
      create: refused,
      patch: refused,
      delete: refused,
      putAttendance: refused,
    },
    exams: {
      list: nothing,
      create: refused,
      patch: refused,
      delete: refused,
      putMyScore: refused,
      deleteMyScore: refused,
      setResultsPublished: refused,
    },
  };
  readonly #http: Http;
  readonly #containerId: string;
  readonly #readToken: string;
  readonly #settings: AccountSettings;
  #containerPromise: Promise<Container> | undefined;
  #cardsPromise: Promise<CardSummary[]> | undefined;

  /**
   * `basePath` as `ServerBackend` takes it; the board or wiki and the token are the
   * link's. `settings` are the device's (its theme and density), which the
   * reader keeps: nobody's account has any here.
   */
  constructor(
    basePath: string,
    containerId: string,
    readToken: string,
    settings: AccountSettings = DEFAULT_ACCOUNT_SETTINGS,
  ) {
    this.#http = new Http(basePath, 'omit');
    this.presence = new SharedPresence(basePath, containerId, readToken);
    this.#containerId = containerId;
    this.#readToken = readToken;
    this.#settings = settings;
  }

  /** Every address of the reader's is made here: the board's, with the token. */
  #read<T>(
    path: SharedPath,
    params: Record<string, string> = {},
    query: Record<string, string | number> = {},
  ): Promise<T> {
    return this.#http.request<T>(
      `/api/v1/shared/{containerId}${path}`,
      { containerId: this.#containerId, ...params },
      { query: { ...query, readToken: this.#readToken } },
    );
  }

  #linked(card: CardSummary): LinkedBlock {
    const { id, title, containerId } = card;
    return {
      id,
      kind: 'card',
      title,
      icon: card.fields.icon ?? '',
      containerId,
      containerKind: 'board',
      ...(card.number === undefined ? {} : { number: card.number }),
    };
  }

  /** Only the link's board or wiki is there: another is as one that does not exist. */
  #only(containerId: string): void {
    if (containerId !== this.#containerId)
      throw new BackendError(404, { detail: 'board not found' });
  }

  me(): Promise<User> {
    return Promise.resolve(READER);
  }

  workspaces(): Promise<WorkspaceInfo[]> {
    return Promise.resolve([{ id: SHARED_PLACE, title: '' }]);
  }

  settings(): Promise<AccountSettings> {
    return Promise.resolve({ ...this.#settings });
  }

  screenState(): Promise<Record<string, string>> {
    return Promise.resolve({});
  }

  async container(containerId: string): Promise<Container> {
    this.#only(containerId);
    this.#containerPromise ??= this.#read<Schemas['Container']>('')
      .then((raw) => ({ ...containerOf(raw), workspaceId: SHARED_PLACE }))
      .catch((error: unknown) => {
        this.#containerPromise = undefined;
        throw error;
      });
    return { ...(await this.#containerPromise) };
  }

  async views(boardId: string): Promise<BoardView[]> {
    this.#only(boardId);
    const blocks = await this.#read<Schemas['Block'][]>('/blocks', {}, { type: 'view' });
    return blocks.map(viewOf);
  }

  async cards(boardId: string): Promise<CardSummary[]> {
    this.#only(boardId);
    this.#cardsPromise ??= pagesTogether((paging) =>
      this.#read<Schemas['Card'][]>('/cards', {}, { ...paging }),
    )
      .then((raw) => raw.map(cardOf))
      .catch((error: unknown) => {
        this.#cardsPromise = undefined;
        throw error;
      });
    return [...(await this.#cardsPromise)];
  }

  async cardPreviews(boardId: string): Promise<Map<string, CardPreview>> {
    this.#only(boardId);
    const cards = await pagesTogether((paging) =>
      this.#read<Schemas['Card'][]>('/cards', {}, { ...paging, preview: 'true' }),
    );
    return previewsOf(cards);
  }

  /** A card with its body: its block, which carries the body a list of cards does not. */
  async card(boardId: string, cardId: string): Promise<CardDetail> {
    this.#only(boardId);
    const [block] = await this.#read<Schemas['Block'][]>(
      '/blocks',
      {},
      { blockId: cardId, type: 'card' },
    );
    // A block of another kind in the place of a card is no card.
    if (block?.type !== 'card') throw new BackendError(404, { detail: 'card not found' });
    const body = bodyOf(block.body);
    return { ...cardOfBlock(block, summaryOfBody(body)), body };
  }

  async comments(boardId: string, cardId: string): Promise<Comment[]> {
    return (await this.#children(boardId, cardId, 'comment')).map(commentOf);
  }

  async attachments(boardId: string, cardId: string): Promise<Attachment[]> {
    return (await this.#children(boardId, cardId, 'attachment')).map(attachmentOf);
  }

  #children(boardId: string, parentId: string, type: string): Promise<Schemas['Block'][]> {
    this.#only(boardId);
    return this.#read<Schemas['Block'][]>('/blocks', {}, { parentId, type });
  }

  fileBytes(containerId: string, fileId: string, signal?: AbortSignal): Promise<Blob> {
    this.#only(containerId);
    return this.#http.bytes(
      '/api/v1/shared/{containerId}/files/{fileId}',
      { containerId, fileId },
      { readToken: this.#readToken },
      signal,
    );
  }

  async fileDetails(containerId: string, fileId: string): Promise<FileDetails> {
    this.#only(containerId);
    const info = await this.#read<Schemas['FileRecord']>('/files/{fileId}/info', { fileId });
    return { name: info.name, size: info.size };
  }

  fileUrl(containerId: string, fileId: string): string | undefined {
    if (containerId !== this.#containerId || !isFileId(fileId)) return undefined;
    return this.#http.url(
      '/api/v1/shared/{containerId}/files/{fileId}',
      { containerId, fileId },
      { readToken: this.#readToken },
    );
  }

  // Nobody is named to the reader, and the place has no sidebar.
  organizations: Backend['organizations'] = nothing;
  users: Backend['users'] = nothing;
  usersById: Backend['usersById'] = nothing;
  members: Backend['members'] = nothing;
  boards: Backend['boards'] = nothing;
  categories: Backend['categories'] = nothing;
  templates: Backend['templates'] = nothing;
  searchBoards: Backend['searchBoards'] = nothing;
  searchUsers: Backend['searchUsers'] = nothing;
  commentVersions: Backend['commentVersions'] = nothing;
  deletedContainers: Backend['deletedContainers'] = nothing;
  deletedCards: Backend['deletedCards'] = nothing;
  trashRetentionDays: Backend['trashRetentionDays'] = () => Promise.resolve(30);
  myPermissions: Backend['myPermissions'] = () => Promise.resolve({ ...READ_ONLY_PERMISSIONS });
  accountToday: Backend['accountToday'] = () =>
    Promise.resolve({
      today: '1970-01-01',
      lockedWorkspaceCount: 0,
      cards: [],
      hasMoreCards: false,
      units: [],
      hasMoreUnits: false,
      reviews: [],
      hasMoreReviews: false,
      meetings: [],
      hasMoreMeetings: false,
    });
  workspaceToday: Backend['workspaceToday'] = refused;

  // What is on the screen alone: kept for the visit.
  patchScreenState: Backend['patchScreenState'] = done;

  // Nothing changes while the board is read.
  watchContainer: Backend['watchContainer'] = unwatched;
  watchSelf: Backend['watchSelf'] = unwatched;

  // Every change is refused before it is sent.
  patchMe: Backend['patchMe'] = refused;
  confirmPassword: Backend['confirmPassword'] = refused;
  patchSettings: Backend['patchSettings'] = refused;
  createBoard: Backend['createBoard'] = refused;
  patchContainer: Backend['patchContainer'] = refused;
  reorderCardProperties: Backend['reorderCardProperties'] = refused;
  duplicateContainer: Backend['duplicateContainer'] = refused;
  onboard: Backend['onboard'] = refused;
  deleteContainer: Backend['deleteContainer'] = refused;
  restoreContainer: Backend['restoreContainer'] = refused;
  addMember: Backend['addMember'] = refused;
  setMemberRole: Backend['setMemberRole'] = refused;
  removeMember: Backend['removeMember'] = refused;
  publicLink: Backend['publicLink'] = refused;
  setPublicLink: Backend['setPublicLink'] = refused;
  joinContainer: Backend['joinContainer'] = refused;
  createCategory: Backend['createCategory'] = refused;
  patchCategory: Backend['patchCategory'] = refused;
  deleteCategory: Backend['deleteCategory'] = refused;
  reorderCategories: Backend['reorderCategories'] = refused;
  moveContainerToCategory: Backend['moveContainerToCategory'] = refused;
  setContainerHidden: Backend['setContainerHidden'] = refused;
  createView: Backend['createView'] = refused;
  writeView: Backend['writeView'] = refused;
  moveView: Backend['moveView'] = refused;
  duplicateView: Backend['duplicateView'] = refused;
  patchView: Backend['patchView'] = refused;
  deleteView: Backend['deleteView'] = refused;
  restoreView: Backend['restoreView'] = refused;
  createCard: Backend['createCard'] = refused;
  duplicateCard: Backend['duplicateCard'] = refused;
  moveViewCards: Backend['moveViewCards'] = refused;
  batchPatchCards: Backend['batchPatchCards'] = refused;
  batchDeleteCards: Backend['batchDeleteCards'] = refused;
  batchRestoreCards: Backend['batchRestoreCards'] = refused;
  patchCard: Backend['patchCard'] = refused;
  createComment: Backend['createComment'] = refused;
  patchComment: Backend['patchComment'] = refused;
  deleteComment: Backend['deleteComment'] = refused;
  restoreComment: Backend['restoreComment'] = refused;
  createAttachment: Backend['createAttachment'] = refused;
  deleteAttachment: Backend['deleteAttachment'] = refused;
  restoreAttachment: Backend['restoreAttachment'] = refused;
  copyContainerToWorkspace: Backend['copyContainerToWorkspace'] = refused;
  upload: Backend['upload'] = refused;
  exportContainer: Backend['exportContainer'] = refused;
  exportWorkspace: Backend['exportWorkspace'] = refused;
  importArchive: Backend['importArchive'] = refused;
}
