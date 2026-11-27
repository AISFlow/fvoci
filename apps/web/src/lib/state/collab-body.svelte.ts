/**
 * Shared Yjs body room lifecycle (`Backend.openDoc`) composed by `CardStore`,
 * `PageStore`, `UnitStore`, and `EntryStore`: opens and closes the room,
 * tracks `room`, `lost`, `bodyEdited`, and `bodySaving`, handles `not_found`
 * eviction via `afterPutOut` with `REJOIN_SPACING` backoff, and registers
 * active rooms for stale-chunk reload settlement.
 */
import type { Backend, BodyDoc, BodyDocState } from './backend.ts';
import { REJOIN_SPACING, afterPutOut } from './card-parts.ts';
import type { Workspace } from './workspace.svelte.ts';

/** How long "saving" stays visible after pending changes reach the server. */
export const SAVING_SHOWN = 600;

export interface CollaborativeBodyHooks {
  readonly workspace: Workspace;
  /** Re-reads the entity via HTTP after a `not_found` room eviction to tell restored vs gone vs 401. */
  probeStillThere: () => Promise<unknown>;
  /** Called when `probeStillThere` confirms 403/404 (`gone`). */
  onGone: () => void;
  /** Optional hook when `state` changes (e.g. `EntryStore` tracking `#changedHere`). */
  onState?: (state: BodyDocState) => void;
  /** Optional hook when the room no longer holds a Yjs document (`!state.doc`). */
  onDocReleased?: () => void;
  /** When true, skips `not_found` probing during the store's initial `loading` phase (`CardStore`). */
  skipProbeWhileLoading?: () => boolean;
  /** Track `bodyEdited` and `bodySaving` indicator (`CardStore`). */
  trackSaving?: boolean;
}

export class CollaborativeBodyState {
  room = $state.raw<BodyDocState | undefined>(undefined);
  lost = $state.raw<readonly string[]>([]);
  bodyEdited = $state(false);
  bodySaving = $state(false);

  readonly #hooks: CollaborativeBodyHooks;
  #bodyDoc: BodyDoc | undefined;
  #savingShown: ReturnType<typeof setTimeout> | undefined;
  #rejoinedAt = -Infinity;

  constructor(hooks: CollaborativeBodyHooks) {
    this.#hooks = hooks;
  }

  get isOpen(): boolean {
    return this.#bodyDoc !== undefined;
  }

  get hasDoc(): boolean {
    return this.room?.doc !== undefined;
  }

  get pending(): boolean {
    return this.room?.pending ?? false;
  }

  open(
    backend: Backend,
    workspaceId: string | undefined,
    containerId: string,
    blockId: string,
  ): void {
    if (this.#bodyDoc !== undefined || !backend.openDoc || workspaceId === undefined) return;
    const opened = backend.openDoc(workspaceId, containerId, blockId, {
      state: (state) => {
        if (this.#bodyDoc !== opened) return;
        const shut = state.status === 'out' && state.reason === 'not_found';
        const prevDoc = this.room?.doc;
        const heldDoc = prevDoc !== undefined;
        const skipProbe = Boolean(this.#hooks.skipProbeWhileLoading?.());
        if (shut && this.room?.reason !== 'not_found' && !skipProbe) {
          void this.#stillThere(opened, heldDoc);
        }
        this.room = state;
        if (this.#hooks.trackSaving) this.#showSaving(state.pending);
        this.#hooks.onState?.(state);
        if (!state.doc || (prevDoc && state.doc !== prevDoc)) this.#hooks.onDocReleased?.();
      },
      discarded: (why) => {
        if (this.#bodyDoc === opened) this.sayLost(why);
      },
    });
    this.#bodyDoc = opened;
    this.room = opened.state;
  }

  /** If `load()` finished while the room had already transitioned to `not_found`, probe now. */
  probeIfNotFound(): void {
    const opened = this.#bodyDoc;
    if (opened && this.room?.reason === 'not_found') {
      void this.#stillThere(opened, false);
    }
  }

  async #stillThere(opened: BodyDoc, heldDoc: boolean): Promise<void> {
    const read = await afterPutOut(this.#hooks.probeStillThere);
    if (this.#bodyDoc !== opened) return;
    if (read.found === 'there' && heldDoc && Date.now() - this.#rejoinedAt >= REJOIN_SPACING) {
      this.#rejoinedAt = Date.now();
      opened.retry();
    }
    if (read.found === 'ended') this.#hooks.workspace.report('load', read.error);
    if (read.found === 'gone') this.#hooks.onGone();
  }

  #showSaving(pending: boolean): void {
    clearTimeout(this.#savingShown);
    if (pending) {
      this.bodyEdited = true;
      this.bodySaving = true;
      return;
    }
    this.#savingShown = setTimeout(() => {
      this.bodySaving = false;
    }, SAVING_SHOWN);
  }

  /** Detaches the underlying `BodyDoc` without closing it yet (for `EntryStore` empty-cleanup). */
  detach(): BodyDoc | undefined {
    clearTimeout(this.#savingShown);
    this.bodySaving = false;
    const opened = this.#bodyDoc;
    this.#bodyDoc = undefined;
    this.room = undefined;
    return opened;
  }

  close(): void {
    this.detach()?.close();
  }

  settle(): Promise<void> {
    return this.#bodyDoc?.settle() ?? Promise.resolve();
  }

  retry(): void {
    this.#bodyDoc?.retry();
  }

  sayLost(why: string): void {
    if (!this.lost.includes(why)) this.lost = [...this.lost, why];
  }

  dismissLost(): void {
    this.lost = [];
  }
}
