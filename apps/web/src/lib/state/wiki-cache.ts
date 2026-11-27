/**
 * Bounded LRU memory of the last opened (or first) page identifier for each
 * wiki, scoped to one signed-in user and workspace. Opening a wiki without a
 * page in the URL requests this page immediately in parallel with the page
 * list, falling back to the list if the page was deleted or access changed.
 */
import { isOwnSpace, type WorkspaceInfo } from './backend.ts';

/** How many wikis' last-opened page identifiers are remembered in memory. */
export const MAX_CACHED_WIKIS = 20;

interface Entry {
  readonly userId: string;
  readonly workspaceId: string;
  readonly wikiId: string;
  readonly pageId: string;
}

export class WikiCache {
  readonly #max: number;
  readonly #entries = new Map<string, Entry>();
  #ownerId: string | undefined;

  constructor(max = MAX_CACHED_WIKIS) {
    this.#max = Math.max(1, max);
  }

  get size(): number {
    return this.#entries.size;
  }

  has(wikiId: string): boolean {
    return this.#entries.has(wikiId);
  }

  /** Returns the remembered page ID for `(userId, workspaceId, wikiId)` and promotes LRU. */
  get(userId: string, workspaceId: string, wikiId: string): string | undefined {
    if (this.#ownerId !== undefined && this.#ownerId !== userId) {
      this.clear();
      return undefined;
    }
    const entry = this.#entries.get(wikiId);
    if (!entry) return undefined;
    if (entry.userId !== userId) {
      this.clear();
      return undefined;
    }
    if (entry.workspaceId !== workspaceId) {
      this.#entries.delete(wikiId);
      return undefined;
    }
    this.#entries.delete(wikiId);
    this.#entries.set(wikiId, entry);
    return entry.pageId;
  }

  set(userId: string, workspaceId: string, wikiId: string, pageId: string): void {
    if (this.#ownerId !== undefined && this.#ownerId !== userId) {
      this.clear();
    }
    this.#ownerId = userId;
    this.#entries.delete(wikiId);
    this.#entries.set(wikiId, { userId, workspaceId, wikiId, pageId });
    while (this.#entries.size > this.#max) {
      const oldest = this.#entries.keys().next().value;
      if (oldest === undefined) break;
      this.#entries.delete(oldest);
    }
  }

  delete(wikiId: string): void {
    this.#entries.delete(wikiId);
  }

  /** Forgets the remembered page of `wikiId` if it matches `pageId`. */
  deletePage(wikiId: string, pageId: string): void {
    if (this.#entries.get(wikiId)?.pageId === pageId) this.#entries.delete(wikiId);
  }

  deleteWorkspace(workspaceId: string): void {
    for (const [id, entry] of [...this.#entries]) {
      if (entry.workspaceId === workspaceId) this.#entries.delete(id);
    }
  }

  deleteOrg(workspaces: readonly WorkspaceInfo[], orgId: string): void {
    const ids = new Set(workspaces.filter((one) => one.orgId === orgId).map((one) => one.id));
    for (const [id, entry] of [...this.#entries]) {
      if (ids.has(entry.workspaceId)) this.#entries.delete(id);
    }
  }

  deletePersonal(workspaces: readonly WorkspaceInfo[]): void {
    const ids = new Set(workspaces.filter(isOwnSpace).map((one) => one.id));
    for (const [id, entry] of [...this.#entries]) {
      if (ids.has(entry.workspaceId)) this.#entries.delete(id);
    }
  }

  pruneWorkspaces(workspaces: readonly WorkspaceInfo[]): void {
    const open = new Set(
      workspaces
        .filter((workspace) => !workspace.ssoRequired && !workspace.passwordRequired)
        .map((workspace) => workspace.id),
    );
    for (const [id, entry] of [...this.#entries]) {
      if (!open.has(entry.workspaceId)) this.#entries.delete(id);
    }
  }

  pruneContainers(workspaceId: string, allowedIds: ReadonlySet<string>): void {
    for (const [id, entry] of [...this.#entries]) {
      if (entry.workspaceId === workspaceId && !allowedIds.has(id)) {
        this.#entries.delete(id);
      }
    }
  }

  clear(): void {
    this.#entries.clear();
    this.#ownerId = undefined;
  }
}
