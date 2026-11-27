/**
 * Whether the reader watches a card or a page (`Backend.watches`), and the
 * switch of it: watching, they are told when someone comments on it or
 * changes it. Nothing is watched in the reader's own space, where nobody else
 * writes, nor through a public link (no `watches`).
 *
 * What is watched is the workspace's, read once for all (`WatchedBlocks`);
 * this is one block's view of it, with nothing of its own to close. The
 * switch shows at once, and goes back with a report when the server refuses;
 * ending a watch that is gone already (404) leaves it ended.
 */
import { isBackendError } from './backend.ts';
import type { WatchedKind } from './watch-types.ts';
import { watchedBlocks, type WatchedBlocks } from './watched-blocks.svelte.ts';
import { isOwnSpace } from './workspace-types.ts';
import type { Workspace } from './workspace.svelte.ts';

export class BlockWatch {
  busy = $state(false);
  readonly #workspace: Workspace;
  readonly #blockId: string;
  readonly #kind: WatchedKind;
  /**
   * Taken here, not where it is first read: state made in a reaction is not
   * followed by that reaction, and what draws the switch reads it.
   */
  readonly #watched: WatchedBlocks;

  constructor(workspace: Workspace, blockId: string, kind: WatchedKind) {
    this.#workspace = workspace;
    this.#blockId = blockId;
    this.#kind = kind;
    this.#watched = watchedBlocks(workspace);
  }

  get canWatch(): boolean {
    const { info, me, backend } = this.#workspace;
    return Boolean(backend.watches && info?.id && me?.id && !isOwnSpace(info));
  }

  /** Undefined until it is read, and where it cannot be. */
  get watching(): boolean | undefined {
    return this.canWatch ? this.#watched.has(this.#blockId) : undefined;
  }

  /** Reads what the reader watches in the workspace, unless that was read already. */
  read(): Promise<void> {
    return this.canWatch ? this.#watched.read() : Promise.resolve();
  }

  async set(next: boolean): Promise<void> {
    const watches = this.#workspace.backend.watches;
    const watched = this.#watched;
    const where = watched.where();
    const before = this.watching;
    if (!watches || !where || before === undefined || this.busy) return;
    const { workspaceId, meId } = where;
    watched.note(where, this.#blockId, next);
    this.busy = true;
    try {
      if (next) await watches.watch(workspaceId, this.#blockId, this.#kind, meId);
      else await watches.unwatch(workspaceId, this.#blockId, meId);
      watched.note(where, this.#blockId, next);
    } catch (error) {
      if (!next && isBackendError(error) && error.status === 404) {
        watched.note(where, this.#blockId, false);
      } else {
        watched.note(where, this.#blockId, before);
        this.#workspace.report('save', error);
      }
    } finally {
      this.busy = false;
    }
  }
}
