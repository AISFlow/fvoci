/**
 * Personal progress & course summary state for one open course (`CourseStore`).
 *
 * Personal progress writes never broadcast over WebSocket, so mutations update
 * local `myProgress` directly from the `PUT` response and immediately reconcile
 * the caller's entry in `progressSummary`. Live counts always derive from live
 * units (chapters and trashed units excluded) with a zero-division guard.
 */
import { SvelteMap, SvelteSet } from 'svelte/reactivity';

import type {
  CourseProgressSummary,
  Courses,
  CourseUnit,
  UnitProgress,
  UnitProgressUpdate,
} from './course-types.ts';
import type { Workspace } from './workspace.svelte.ts';

export interface CourseProgressHost {
  readonly courseId: string;
  readonly workspace: Workspace;
  courses(): Courses;
  units(): readonly CourseUnit[];
  canTrack(): boolean;
  isClosed(): boolean;
  onCompletionChanged?: (unitId: string, completed: boolean) => void;
}

export class CourseProgress {
  readonly #host: CourseProgressHost;

  /** The caller's personal progress rows keyed by `unitId`. */
  readonly myProgress = new SvelteMap<string, UnitProgress>();
  /** Aggregate course completion summary across active course members. */
  progressSummary = $state.raw<CourseProgressSummary | null>(null);
  /** Unit IDs currently awaiting a `putUnitProgress` response. */
  readonly #busy = new SvelteSet<string>();
  #writeVersion = 0;
  #mineReads = 0;
  #summaryReads = 0;

  readonly liveUnitIds = $derived.by(
    () =>
      new SvelteSet(
        this.#host
          .units()
          .filter((unit) => unit.kind === 'unit')
          .map((unit) => unit.id),
      ),
  );
  readonly totalUnits = $derived(this.liveUnitIds.size);
  readonly completedUnits = $derived.by(() => {
    let count = 0;
    for (const [unitId, row] of this.myProgress) {
      if (this.liveUnitIds.has(unitId) && row.completedAt !== null) count += 1;
    }
    return count;
  });
  readonly percent = $derived(
    this.totalUnits > 0 ? Math.round((this.completedUnits / this.totalUnits) * 100) : 0,
  );

  constructor(host: CourseProgressHost) {
    this.#host = host;
  }

  /** Monotonic version incremented on every personal progress mutation or clear. */
  get writeVersion(): number {
    return this.#writeVersion;
  }

  take(
    rows: readonly UnitProgress[],
    summary: CourseProgressSummary | null,
    ifVersion?: number,
  ): void {
    this.#summaryReads += 1;
    if (ifVersion === undefined || ifVersion === this.#writeVersion) {
      this.#mineReads += 1;
      this.myProgress.clear();
      for (const row of rows) this.myProgress.set(row.unitId, row);
      this.progressSummary = summary;
    } else if (summary !== null) {
      this.progressSummary = summary;
      this.#reconcileOwnSummary();
    }
  }

  clearMine(): void {
    this.#writeVersion += 1;
    this.#mineReads += 1;
    this.myProgress.clear();
  }

  applyRow(row: UnitProgress): void {
    this.#writeVersion += 1;
    this.#mineReads += 1;
    this.myProgress.set(row.unitId, row);
  }

  of(unitId: string): UnitProgress | undefined {
    return this.myProgress.get(unitId);
  }

  isCompleted(unitId: string): boolean {
    return (
      (this.myProgress.get(unitId)?.completedAt ?? null) !== null && this.liveUnitIds.has(unitId)
    );
  }

  isBusy(unitId: string): boolean {
    return this.#busy.has(unitId);
  }

  async toggleComplete(unitId: string): Promise<boolean> {
    if (!this.#host.canTrack() || !this.liveUnitIds.has(unitId)) return false;
    return this.#put(unitId, { completed: !this.isCompleted(unitId) });
  }

  async setLastPosition(unitId: string, position: string | null): Promise<boolean> {
    if (!this.#host.canTrack() || !this.liveUnitIds.has(unitId)) return false;
    const trimmed = position === null ? null : position.trim();
    return this.#put(unitId, { lastPosition: trimmed === '' ? null : trimmed });
  }

  /** Re-reads `myProgress` (e.g., when another editor restores a trashed unit). */
  async refreshMine(): Promise<void> {
    if (this.#host.isClosed() || !this.#host.canTrack()) return;
    const version = this.#writeVersion;
    this.#mineReads += 1;
    const reading = this.#mineReads;
    try {
      const rows = await this.#host
        .courses()
        .myProgress(this.#host.courseId, this.#host.workspace.info?.id);
      if (
        this.#host.isClosed() ||
        !this.#host.canTrack() ||
        reading !== this.#mineReads ||
        version !== this.#writeVersion
      ) {
        return;
      }
      this.myProgress.clear();
      for (const row of rows) this.myProgress.set(row.unitId, row);
      this.#reconcileOwnSummary();
    } catch {
      // Best-effort refresh; existing rows remain shown.
    }
  }

  async refreshSummary(): Promise<void> {
    if (this.#host.isClosed()) return;
    this.#summaryReads += 1;
    const reading = this.#summaryReads;
    try {
      const summary = await this.#host
        .courses()
        .progressSummary(this.#host.courseId, this.#host.workspace.info?.id);
      if (this.#host.isClosed() || reading !== this.#summaryReads) return;
      this.progressSummary = summary;
    } catch {
      // Summary refresh is best-effort; existing summary stays shown.
    }
  }

  async #put(unitId: string, update: UnitProgressUpdate): Promise<boolean> {
    if (this.#busy.has(unitId)) return false;
    this.#busy.add(unitId);
    try {
      const saved = await this.#host
        .courses()
        .putUnitProgress(this.#host.courseId, unitId, update, this.#host.workspace.info?.id);
      if (this.#host.isClosed() || !this.#host.canTrack()) return false;
      this.#writeVersion += 1;
      this.#mineReads += 1;
      this.myProgress.set(saved.unitId, saved);
      this.#reconcileOwnSummary();
      if (typeof update.completed === 'boolean') {
        this.#host.onCompletionChanged?.(saved.unitId, saved.completedAt !== null);
      }
      return true;
    } catch (error) {
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return false;
    } finally {
      this.#busy.delete(unitId);
    }
  }

  #reconcileOwnSummary(): void {
    const meId = this.#host.workspace.me?.id;
    const summary = this.progressSummary;
    if (!meId || !summary) return;
    this.#summaryReads += 1;
    const completed = this.completedUnits;
    const totalUnits = this.totalUnits;
    const hasMe = summary.members.some((member) => member.userId === meId);
    const members = hasMe
      ? summary.members.map((member) =>
          member.userId === meId ? { ...member, completedUnits: completed } : member,
        )
      : [...summary.members, { userId: meId, completedUnits: completed }].sort((a, b) =>
          a.userId < b.userId ? -1 : a.userId > b.userId ? 1 : 0,
        );
    this.progressSummary = { totalUnits, members };
  }
}
