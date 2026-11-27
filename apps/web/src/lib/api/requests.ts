/**
 * The workspace-scoped HTTP requests of the server backend: boards, wikis,
 * diaries, courses, files, trash, references, versions, and notifications.
 * Pure account/auth calls live in `account-requests.ts`, and `ServerBackend`
 * (`backend.ts`) adds live changes and the remaining board writes.
 */
import type { FileDetails } from '../editor/files.ts';
import { isFileId } from '../editor/schema.ts';
import type { Container } from '../logic/wire.ts';
import {
  BackendError,
  type DeletedItem,
  type Paging,
  type TrashReads,
  type User,
} from '../state/backend.ts';
import type { ApiTokens } from '../state/api-tokens.ts';
import type { BodyVersions } from '../state/body-versions.ts';
import type { EmailChange } from '../state/email-change.ts';
import type { FileVersions } from '../state/file-versions.ts';
import type { People } from '../state/people-types.ts';
import type { References } from '../state/references.ts';

import { AccountRequests } from './account-requests.ts';
import { ServerApiTokens } from './api-tokens.ts';
import { ServerBodyVersions } from './body-versions.ts';
import { containerOf, userOf, type Schemas } from './convert.ts';
import { ServerCourses } from './courses.ts';
import { ServerDiaries } from './diaries.ts';
import { ServerEmailChange } from './email-change.ts';
import { ServerFileVersions } from './file-versions.ts';
import type { ApiPath, Http, RequestOptions } from './http.ts';
import { ServerNotifications } from './notifications.ts';
import { ServerPeople } from './people.ts';
import { ServerReferences } from './references.ts';
import { ServerTime } from './time.ts';
import { ServerWatches } from './watches.ts';
import { ServerWikis } from './wikis.ts';

/** A path under a workspace, as the contract writes it after `/api/v1/w/{workspaceId}`. */
export type WorkspacePath = ApiPath extends infer P
  ? P extends `/api/v1/w/{workspaceId}${infer Rest}`
    ? Rest
    : never
  : never;

/**
 * The API path of `path` under a workspace: every address under
 * `/api/v1/w/{workspaceId}` is made from this.
 */
export function underWorkspace<P extends WorkspacePath>(path: P): `/api/v1/w/{workspaceId}${P}` {
  return `/api/v1/w/{workspaceId}${path}`;
}

/** The trash as the contract lists it: a board's `kind` where it says one. */
function deletedItemsOf(items: readonly Schemas['DeletedItem'][]): DeletedItem[] {
  return items.map(({ kind, ...item }) => (kind ? { ...item, kind } : item));
}

/** How many identifiers `getWorkspaceUsersById` takes at once (its `maxItems`). */
export const USERS_AT_ONCE = 200;

export class ServerRequests extends AccountRequests implements TrashReads {
  readonly #http: Http;
  /** Who is in a workspace, and the links that let others in. */
  readonly people: People;
  /** The wikis and their pages, under the workspace of each wiki (`workspaceOf`). */
  readonly wiki: ServerWikis;
  /** The diaries of the own space and their entries, under the workspace of each diary. */
  readonly diary: ServerDiaries;
  /** The courses and their chapters and units, under the workspace of each course. */
  readonly course: ServerCourses;
  /** The versions kept of the bodies of cards. */
  readonly bodyVersions: BodyVersions;
  readonly fileVersions: FileVersions;
  /** What bodies name, as the reader may see it. */
  readonly references: References;
  readonly emailChange: EmailChange;
  readonly apiTokens: ApiTokens;
  /** What the reader was told, of every workspace. */
  readonly notifications: ServerNotifications;
  /** What the reader watches, the cards, pages and units they are told of. */
  readonly watches: ServerWatches;
  /** Personal time tracking entries and active timer of the signed-in reader. */
  readonly time: ServerTime;
  /** The workspace the reader is in (`enter`): the paths of its boards are under it. */
  #workspaceId: string | undefined;
  /**
   * The workspace of each board read (`learnPlaces`): a board's paths are
   * under its own, also when the reader has entered another since a request
   * on it began (an upload waiting for its turn, a write after a reading).
   */
  readonly #places = new Map<string, string>();

  constructor(http: Http) {
    super(http);
    this.#http = http;
    this.people = new ServerPeople(http);
    this.emailChange = new ServerEmailChange(http);
    this.apiTokens = new ServerApiTokens(http);
    this.notifications = new ServerNotifications(http);
    this.time = new ServerTime(http);
    this.bodyVersions = new ServerBodyVersions((path, params, options) =>
      this.inWorkspace(path, params, options),
    );
    this.fileVersions = new ServerFileVersions(
      (path, params, options) => this.inWorkspace(path, params, options),
      (path, params, signal) => this.inWorkspaceBytes(path, params, signal),
    );
    this.references = new ServerReferences((path, params, options, workspaceId) =>
      this.inWorkspace(path, params, options, workspaceId),
    );
    this.watches = new ServerWatches((path, params, options, workspaceId) =>
      this.inWorkspace(path, params, options, workspaceId),
    );
    const request = this.#inContainer.bind(this);
    const learn = (containers: readonly Schemas['Container'][]): void => {
      this.learnPlaces(containers);
    };
    this.wiki = new ServerWikis(request, learn);
    this.diary = new ServerDiaries(request, learn);
    this.course = new ServerCourses(request, learn);
  }

  /** A request under the workspace of the container `wikiId`, `diaryId`, or `courseId` names. */
  #inContainer<T>(
    path: WorkspacePath,
    params: Record<string, string>,
    options: RequestOptions = {},
    workspaceId?: string,
  ): Promise<T> {
    return this.inWorkspace<T>(
      path,
      { ...params, containerId: params.wikiId ?? params.diaryId ?? params.courseId ?? '' },
      options,
      workspaceId,
    );
  }

  /** The requests that name no workspace go to this one from now on. */
  enter(workspaceId: string): void {
    this.#workspaceId = workspaceId;
  }

  /**
   * Keeps the workspace of each board, for the paths of the board
   * (`workspaceOf`). A built-in template is every workspace's: its paths are
   * under the one the reader is in.
   */
  protected learnPlaces(
    boards: readonly { id: string; workspaceId: string; isBuiltin?: boolean }[],
  ): void {
    for (const board of boards) {
      if (!board.isBuiltin) this.#places.set(board.id, board.workspaceId);
    }
  }

  /**
   * The parameters of a path under `/w/{workspaceId}` with the workspace
   * filled in: the container's own when `containerId` or `boardId` names
   * one read before, else the one the reader is in, unless `workspaceId` names another.
   */
  protected workspaceOf<P extends Record<string, string>>(
    params: P,
    workspaceId = this.#placeOf(params.containerId || params.boardId) || this.#workspaceId,
  ): P & { workspaceId: string } {
    if (workspaceId === undefined) {
      throw new BackendError(500, { detail: 'no workspace was entered' });
    }
    return { ...params, workspaceId };
  }

  /** The workspace of the container `id`, if it was read before. */
  #placeOf(id: string | undefined): string | undefined {
    return id ? this.#places.get(id) : undefined;
  }

  /** A request to `path` under the workspace (`workspaceOf`). */
  protected inWorkspace<T>(
    path: WorkspacePath,
    params: Record<string, string> = {},
    options: RequestOptions = {},
    workspaceId?: string,
  ): Promise<T> {
    return this.#http.request<T>(
      underWorkspace(path),
      this.workspaceOf(params, workspaceId),
      options,
    );
  }

  /** The bytes at `path` under the workspace (a file). */
  protected inWorkspaceBytes(
    path: WorkspacePath,
    params: Record<string, string>,
    signal?: AbortSignal,
  ): Promise<Blob> {
    return this.#http.bytes(underWorkspace(path), this.workspaceOf(params), {}, signal);
  }

  /** The address of `path` under the workspace. */
  protected inWorkspaceUrl(path: WorkspacePath, params: Record<string, string>): string {
    return this.#http.url(underWorkspace(path), this.workspaceOf(params));
  }

  async boards(workspaceId: string): Promise<Container[]> {
    const boards = await this.#http.request<Schemas['Container'][]>(underWorkspace('/boards'), {
      workspaceId,
    });
    this.learnPlaces(boards);
    return boards.map(containerOf);
  }

  async container(containerId: string): Promise<Container> {
    const board = containerOf(
      await this.inWorkspace<Schemas['Container']>('/containers/{containerId}', {
        containerId,
      }),
    );
    this.learnPlaces([board]);
    return board;
  }

  /**
   * The people of a workspace by their identifiers, each asked for once. The
   * server leaves out whom the reader does not see, and takes at most
   * `USERS_AT_ONCE` identifiers in a request: a longer list is asked for in parts.
   */
  async usersById(workspaceId: string, userIds: string[]): Promise<User[]> {
    const ids = [...new Set(userIds)];
    const parts: string[][] = [];
    for (let at = 0; at < ids.length; at += USERS_AT_ONCE) {
      parts.push(ids.slice(at, at + USERS_AT_ONCE));
    }
    const found = await Promise.all(
      parts.map((json) =>
        this.#http.request<Schemas['User'][]>(
          underWorkspace('/users'),
          { workspaceId },
          { method: 'POST', json },
        ),
      ),
    );
    return found.flat().map(userOf);
  }

  /** The bytes of a file (`getFile`), until `signal` gives the reading up. */
  fileBytes(containerId: string, fileId: string, signal?: AbortSignal): Promise<Blob> {
    const params = { containerId, fileId };
    return this.inWorkspaceBytes('/containers/{containerId}/files/{fileId}', params, signal);
  }

  /** The name and size of a file (`getFileRecord`). */
  async fileDetails(containerId: string, fileId: string): Promise<FileDetails> {
    const params = { containerId, fileId };
    const info = await this.inWorkspace<Schemas['FileRecord']>(
      '/containers/{containerId}/files/{fileId}/info',
      params,
    );
    return { name: info.name, size: info.size };
  }

  /** The file's bytes, from the page's own origin: the session's cookie goes along. */
  fileUrl(containerId: string, fileId: string): string | undefined {
    if (!isFileId(fileId)) return undefined;
    const workspaceId = this.#placeOf(containerId) || this.#workspaceId;
    if (!workspaceId) return undefined;
    return this.#http.url(underWorkspace('/containers/{containerId}/files/{fileId}'), {
      workspaceId,
      containerId,
      fileId,
    });
  }

  async deletedCards(boardId: string, paging?: Paging): Promise<DeletedItem[]> {
    return deletedItemsOf(
      await this.inWorkspace<Schemas['DeletedItem'][]>(
        '/boards/{boardId}/trash/cards',
        { boardId },
        { query: { ...paging } },
      ),
    );
  }

  async deletedContainers(workspaceId: string, paging?: Paging): Promise<DeletedItem[]> {
    const items = await this.#http.request<Schemas['DeletedItem'][]>(
      underWorkspace('/trash/containers'),
      { workspaceId },
      { query: { ...paging } },
    );
    return deletedItemsOf(items);
  }
}
