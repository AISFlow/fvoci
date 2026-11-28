/**
 * The reader's stars in the workspace they are in (`Backend.favorites`): read
 * once for the sidebar, the search and every ★ switch of the workspace, one
 * list of the reader's (`workspaceFavorites`). A star put on or taken off here
 * shows at once, and goes back if the server refuses it; a change made on
 * another of the reader's devices (`favorites`), a container that goes, or a
 * connection that comes back (`resync`) reads the list again. Nothing of it is
 * kept past the workspace: a list read for one reader or workspace is never
 * shown for another, and whoever shows it reads it again for the reader and
 * workspace of now (`scope`): entered from a page of none, or from another.
 */
import { SvelteMap } from 'svelte/reactivity';

import type { Favorite } from './favorite-types.ts';
import { isBackendError } from './problems.ts';
import type { Workspace } from './workspace.svelte.ts';

/** Whose list, and where. */
interface Where {
  readonly workspaceId: string;
  readonly meId: string;
}

const keyOf = (where: Where): string => `${where.workspaceId}/${where.meId}`;

export class WorkspaceFavorites {
  readonly #workspace: Workspace;
  /** The list, and for whom and where it was read. */
  #held = $state.raw<{ readonly key: string; readonly list: readonly Favorite[] } | undefined>(
    undefined,
  );
  /** The stars put on (true) or taken off (false) that the list does not say yet. */
  readonly #switched = new SvelteMap<string, boolean>();
  #reading: { readonly key: string; readonly done: Promise<void> } | undefined;
  #reads = 0;
  #stopSelf: (() => void) | undefined;

  constructor(workspace: Workspace) {
    this.#workspace = workspace;
    workspace.onClose(() => {
      this.#stopSelf?.();
      this.#stopSelf = undefined;
      this.#reads += 1;
      this.#reading = undefined;
      this.#held = undefined;
      this.#switched.clear();
    });
  }

  #where(): Where | undefined {
    const workspaceId = this.#workspace.info?.id;
    const meId = this.#workspace.me?.id;
    if (!this.#workspace.backend.favorites || !workspaceId || !meId) return undefined;
    return { workspaceId, meId };
  }

  /** Whether stars are kept here: not through a public link, nor before the workspace is entered. */
  get available(): boolean {
    return this.#where() !== undefined;
  }

  /**
   * For whom and where the stars are now (`<workspace>/<reader>`), undefined
   * where none are kept: what shows them reads them again as it changes.
   */
  get scope(): string | undefined {
    const where = this.#where();
    return where && keyOf(where);
  }

  /** The stars, in their order; empty until they are read. */
  get list(): readonly Favorite[] {
    const where = this.#where();
    const held = this.#held;
    if (!where || held?.key !== keyOf(where)) return [];
    return held.list.filter((favorite) => this.#switched.get(favorite.targetId) !== false);
  }

  /** Whether the list was read for this reader and workspace. */
  get loaded(): boolean {
    const where = this.#where();
    return where !== undefined && this.#held?.key === keyOf(where);
  }

  /** Whether a star is on it; undefined until the list is read, and where none is kept. */
  has(targetId: string): boolean | undefined {
    if (!this.loaded) return undefined;
    const switched = this.#switched.get(targetId);
    if (switched !== undefined) return switched;
    return this.#held?.list.some((favorite) => favorite.targetId === targetId) ?? false;
  }

  /** Reads the list, unless it was read, or is being read, for this reader and workspace. */
  read(): Promise<void> {
    const where = this.#where();
    if (!where) return Promise.resolve();
    const key = keyOf(where);
    if (this.#reading?.key === key) return this.#reading.done;
    if (this.#held?.key === key) return Promise.resolve();
    return this.#readFor(where);
  }

  /**
   * Puts a star on it, or takes it off; a refusal puts it back as it was, and
   * says why. A star taken off already (in another tab, say) is off: the
   * server's 404 says so.
   */
  async toggle(targetId: string): Promise<void> {
    const where = this.#where();
    const favorites = this.#workspace.backend.favorites;
    const starred = this.has(targetId);
    if (!where || !favorites || starred === undefined) return;
    this.#switched.set(targetId, !starred);
    try {
      if (starred) await favorites.unstar(where.workspaceId, targetId);
      else await favorites.star(where.workspaceId, targetId);
    } catch (error) {
      if (starred && isBackendError(error) && error.status === 404) {
        await this.#readFor(where);
        return;
      }
      this.#switched.delete(targetId);
      this.#workspace.report('save', error);
      return;
    }
    await this.#readFor(where);
  }

  /** Puts the stars in the order of `targetIds`; a refusal reads the order the server kept. */
  async reorder(targetIds: readonly string[]): Promise<void> {
    const where = this.#where();
    const favorites = this.#workspace.backend.favorites;
    const held = this.#held;
    if (!where || !favorites || held?.key !== keyOf(where)) return;
    const first = targetIds.flatMap(
      (id) => held.list.find((favorite) => favorite.targetId === id) ?? [],
    );
    const rest = held.list.filter((favorite) => !targetIds.includes(favorite.targetId));
    this.#held = { key: held.key, list: [...first, ...rest] };
    try {
      await favorites.reorder(where.workspaceId, targetIds);
    } catch (error) {
      this.#workspace.report('save', error);
      await this.#readFor(where);
    }
  }

  #readFor(where: Where): Promise<void> {
    const favorites = this.#workspace.backend.favorites;
    if (!favorites) return Promise.resolve();
    this.#follow();
    const key = keyOf(where);
    const turn = ++this.#reads;
    const done = favorites.list(where.workspaceId).then(
      (list) => {
        if (turn !== this.#reads) return;
        this.#held = { key, list };
        this.#switched.clear();
      },
      () => {
        // Not read: what was shown stays, and a switch made meanwhile with it.
      },
    );
    this.#reading = { key, done };
    void done.finally(() => {
      if (turn === this.#reads) this.#reading = undefined;
    });
    return done;
  }

  /**
   * Follows the reader's changes: what changes the list, or may have, reads it
   * again; also where the list held is of another workspace than the one now.
   */
  #follow(): void {
    if (this.#stopSelf) return;
    this.#stopSelf = this.#workspace.backend.watchSelf((change) => {
      const where = this.#where();
      if (!where) return;
      const held = this.#held?.key === keyOf(where) ? this.#held : undefined;
      const again =
        change.kind === 'resync' ||
        (change.kind === 'favorites' && change.workspaceId === where.workspaceId) ||
        (change.kind === 'containerDeleted' &&
          held?.list.some((favorite) => favorite.containerId === change.containerId) === true);
      if (again) void this.#readFor(where);
    });
  }
}

const ofWorkspace = new WeakMap<Workspace, WorkspaceFavorites>();

/** The reader's stars in `workspace`: one list for all its screens. */
export function workspaceFavorites(workspace: Workspace): WorkspaceFavorites {
  let favorites = ofWorkspace.get(workspace);
  if (!favorites) {
    favorites = new WorkspaceFavorites(workspace);
    ofWorkspace.set(workspace, favorites);
  }
  return favorites;
}
