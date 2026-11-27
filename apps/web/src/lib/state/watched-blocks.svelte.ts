/**
 * What the reader watches in the workspace they are in (`Backend.watches`),
 * read once for every card and page that shows the switch (`BlockWatch`)
 * rather than once for each of them: one list of the reader's, which grows
 * with all they ever wrote or commented on. A switch made here is known at
 * once to every screen; a connection that comes back (`resync`) reads it
 * again, as what changed meanwhile was not heard.
 */
import type { Workspace } from './workspace.svelte.ts';

/** Whose list, and where: a list read for one is never shown for another. */
export interface WatchedWhere {
  readonly workspaceId: string;
  readonly meId: string;
}

const keyOf = (where: WatchedWhere): string => `${where.workspaceId}/${where.meId}`;

/** `ids` with the block in them, or out of them. */
const switchedIn = (ids: Iterable<string>, blockId: string, watching: boolean): string[] => [
  ...[...ids].filter((id) => id !== blockId),
  ...(watching ? [blockId] : []),
];

/** What is watched, as it is held: never changed, a new one replaces it. */
const setOf = (ids: Iterable<string>): ReadonlySet<string> => new Set(ids);

export class WatchedBlocks {
  readonly #workspace: Workspace;
  /** What is watched, and for whom and where it was read. */
  #held = $state.raw<{ readonly key: string; readonly ids: ReadonlySet<string> } | undefined>(
    undefined,
  );
  /** The reading under way: for whom and where, and when it is done. */
  #reading: { readonly key: string; readonly done: Promise<void> } | undefined;
  #reads = 0;
  /** The switches made since the reading under way began: they win over what it says. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #switched = new Map<string, boolean>();
  #stopSelf: (() => void) | undefined;

  constructor(workspace: Workspace) {
    this.#workspace = workspace;
    workspace.onClose(() => {
      this.#stopSelf?.();
      this.#stopSelf = undefined;
      this.#reads += 1;
      this.#reading = undefined;
      this.#held = undefined;
    });
  }

  /** The reader and the workspace they are in; undefined until both are known. */
  where(): WatchedWhere | undefined {
    const workspaceId = this.#workspace.info?.id;
    const meId = this.#workspace.me?.id;
    return workspaceId && meId ? { workspaceId, meId } : undefined;
  }

  /** Whether the block is watched; undefined until that is read, and where it cannot be. */
  has(blockId: string): boolean | undefined {
    const where = this.where();
    const held = this.#held;
    return where && held?.key === keyOf(where) ? held.ids.has(blockId) : undefined;
  }

  /** Reads what is watched, unless it was read, or is being read, for this reader and workspace. */
  read(): Promise<void> {
    const where = this.where();
    if (!where) return Promise.resolve();
    const key = keyOf(where);
    if (this.#reading?.key === key) return this.#reading.done;
    if (this.#held?.key === key) return Promise.resolve();
    return this.#readFor(where);
  }

  /**
   * Takes a switch of the reader's at once, for whom and where it was made:
   * a reading begun before it lands does not undo it.
   */
  note(where: WatchedWhere, blockId: string, watching: boolean): void {
    const key = keyOf(where);
    if (this.#reading?.key === key) this.#switched.set(blockId, watching);
    const held = this.#held;
    if (held?.key !== key || held.ids.has(blockId) === watching) return;
    this.#held = { key, ids: setOf(switchedIn(held.ids, blockId, watching)) };
  }

  #readFor(where: WatchedWhere): Promise<void> {
    const watches = this.#workspace.backend.watches;
    if (!watches) return Promise.resolve();
    this.#follow();
    const key = keyOf(where);
    const turn = ++this.#reads;
    this.#switched.clear();
    const done = watches.watching(where.workspaceId, where.meId).then(
      (read) => {
        if (turn !== this.#reads) return;
        let ids = read;
        for (const [blockId, watching] of this.#switched) ids = switchedIn(ids, blockId, watching);
        this.#held = { key, ids: setOf(ids) };
      },
      () => {
        // Not read: the switches stay as they were, or are not shown where nothing was read yet.
      },
    );
    this.#reading = { key, done };
    void done.finally(() => {
      if (turn === this.#reads) this.#reading = undefined;
    });
    return done;
  }

  /** Follows the reader's connection: back after a break, it reads again what it had read. */
  #follow(): void {
    if (this.#stopSelf) return;
    this.#stopSelf = this.#workspace.backend.watchSelf((change) => {
      if (change.kind !== 'resync') return;
      const where = this.where();
      if (where && this.#held?.key === keyOf(where)) void this.#readFor(where);
    });
  }
}

const ofWorkspace = new WeakMap<Workspace, WatchedBlocks>();

/** What the reader watches in `workspace`: one list for all its screens. */
export function watchedBlocks(workspace: Workspace): WatchedBlocks {
  let watched = ofWorkspace.get(workspace);
  if (!watched) {
    watched = new WatchedBlocks(workspace);
    ofWorkspace.set(workspace, watched);
  }
  return watched;
}
