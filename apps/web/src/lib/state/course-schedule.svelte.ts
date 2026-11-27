/**
 * Course study schedule, unit study amount/date metadata, shared & personal
 * manual redistribution, and per-unit personal planned dates for one open course
 * (`CourseStore`).
 *
 * Personal schedule writes never emit WebSocket events, so mutations reconcile
 * both `view` and `CourseProgress.myProgress` directly from the HTTP response.
 */
import { SvelteMap, SvelteSet } from 'svelte/reactivity';

import type {
  CourseSchedule,
  CourseScheduleUpdate,
  CourseScheduleView,
  Courses,
  CourseUnit,
  RedistributeScheduleRequest,
  ScheduledUnit,
  UnitPatch,
  UnitProgress,
} from './course-types.ts';
import { isBackendError, type BackendError } from './problems.ts';
import type { Workspace } from './workspace.svelte.ts';

export interface CourseScheduleHost {
  readonly courseId: string;
  readonly workspace: Workspace;
  courses(): Courses;
  canEdit(): boolean;
  canTrack(): boolean;
  isClosed(): boolean;
  takeUnit(unit: CourseUnit): void;
  applyProgress(row: UnitProgress): void;
  refreshMine(): void;
  syncUnitDates(units: readonly ScheduledUnit[]): void;
}

export interface ScheduleWriteResult {
  ok: boolean;
  error?: BackendError;
}

function recount(view: CourseScheduleView, units: ScheduledUnit[]): CourseScheduleView {
  let todayCount = 0;
  let overdueCount = 0;
  for (const unit of units) {
    if (unit.overdue) overdueCount += 1;
    else if (!unit.completed && unit.effectiveDate === view.today) todayCount += 1;
  }
  return { ...view, units, todayCount, overdueCount };
}

export class CourseScheduleState {
  readonly #host: CourseScheduleHost;

  view = $state.raw<CourseScheduleView | null>(null);
  busy = $state(false);
  readonly #busyUnits = new SvelteSet<string>();
  #writeVersion = 0;
  #reads = 0;

  readonly schedule = $derived<CourseSchedule | null>(this.view?.schedule ?? null);
  readonly today = $derived(this.view?.today ?? '');
  readonly todayCount = $derived(this.view?.todayCount ?? 0);
  readonly overdueCount = $derived(this.view?.overdueCount ?? 0);
  readonly byUnit = $derived.by(
    () => new SvelteMap((this.view?.units ?? []).map((item) => [item.unitId, item])),
  );

  constructor(host: CourseScheduleHost) {
    this.#host = host;
  }

  get writeVersion(): number {
    return this.#writeVersion;
  }

  take(view: CourseScheduleView | null, ifVersion?: number): void {
    if (ifVersion !== undefined && ifVersion !== this.#writeVersion) return;
    this.#reads += 1;
    this.view = view;
  }

  clearMine(): void {
    this.#writeVersion += 1;
    this.#reads += 1;
    if (!this.view) return;
    const today = this.view.today;
    const units = this.view.units.map((unit) => {
      const effectiveDate = unit.scheduledDate;
      const overdue = effectiveDate !== null && effectiveDate < today;
      return {
        ...unit,
        myPlannedDate: null,
        effectiveDate,
        completed: false,
        overdue,
      };
    });
    this.view = recount(this.view, units);
  }

  unit(unitId: string): ScheduledUnit | undefined {
    return this.byUnit.get(unitId);
  }

  isUnitBusy(unitId: string): boolean {
    return this.#busyUnits.has(unitId);
  }

  reconcileCompletion(unitId: string, completed: boolean): void {
    this.#writeVersion += 1;
    this.#reads += 1;
    if (!this.view) return;
    if (!this.view.units.some((unit) => unit.unitId === unitId)) {
      void this.refresh();
      return;
    }
    const today = this.view.today;
    const units = this.view.units.map((unit) => {
      if (unit.unitId !== unitId) return unit;
      const overdue = !completed && unit.effectiveDate !== null && unit.effectiveDate < today;
      return { ...unit, completed, overdue };
    });
    this.view = recount(this.view, units);
  }

  async refresh(): Promise<void> {
    if (this.#host.isClosed()) return;
    const version = this.#writeVersion;
    this.#reads += 1;
    const reading = this.#reads;
    try {
      const next = await this.#host
        .courses()
        .schedule(this.#host.courseId, this.#host.workspace.info?.id);
      if (this.#host.isClosed() || reading !== this.#reads || version !== this.#writeVersion) {
        return;
      }
      this.view = next;
    } catch {
      // Best-effort refresh; keep existing schedule view shown.
    }
  }

  async saveSchedule(update: CourseScheduleUpdate): Promise<ScheduleWriteResult> {
    if (!this.#host.canEdit() || this.busy) return { ok: false };
    this.busy = true;
    try {
      const next = await this.#host
        .courses()
        .putSchedule(this.#host.courseId, update, this.#host.workspace.info?.id);
      if (this.#host.isClosed()) return { ok: false };
      this.#writeVersion += 1;
      this.#reads += 1;
      this.view = next;
      if (update.autoAssign) this.#host.syncUnitDates(next.units);
      return { ok: true };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.busy = false;
    }
  }

  async deleteSchedule(): Promise<boolean> {
    if (!this.#host.canEdit() || this.busy) return false;
    this.busy = true;
    try {
      await this.#host.courses().deleteSchedule(this.#host.courseId, this.#host.workspace.info?.id);
      if (this.#host.isClosed()) return false;
      this.#writeVersion += 1;
      this.#reads += 1;
      if (this.view) this.view = { ...this.view, schedule: null };
      return true;
    } catch (error) {
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return false;
    } finally {
      this.busy = false;
    }
  }

  async redistributeShared(
    request: RedistributeScheduleRequest = {},
  ): Promise<ScheduleWriteResult> {
    if (!this.#host.canEdit() || this.busy) return { ok: false };
    this.busy = true;
    try {
      const next = await this.#host
        .courses()
        .redistributeSchedule(this.#host.courseId, request, this.#host.workspace.info?.id);
      if (this.#host.isClosed()) return { ok: false };
      this.#writeVersion += 1;
      this.#reads += 1;
      this.view = next;
      this.#host.syncUnitDates(next.units);
      return { ok: true };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.busy = false;
    }
  }

  async redistributeMine(request: RedistributeScheduleRequest = {}): Promise<ScheduleWriteResult> {
    if (!this.#host.canTrack() || this.busy) return { ok: false };
    this.busy = true;
    try {
      const next = await this.#host
        .courses()
        .redistributeMySchedule(this.#host.courseId, request, this.#host.workspace.info?.id);
      if (this.#host.isClosed()) return { ok: false };
      this.#writeVersion += 1;
      this.#reads += 1;
      this.view = next;
      this.#host.refreshMine();
      return { ok: true };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.busy = false;
    }
  }

  async setUnitPlannedDate(
    unitId: string,
    plannedDate: string | null,
  ): Promise<ScheduleWriteResult> {
    if (!this.#host.canTrack() || this.#busyUnits.has(unitId)) return { ok: false };
    this.#busyUnits.add(unitId);
    try {
      const saved = await this.#host
        .courses()
        .putUnitSchedule(
          this.#host.courseId,
          unitId,
          { plannedDate },
          this.#host.workspace.info?.id,
        );
      if (this.#host.isClosed() || !this.#host.canTrack()) return { ok: false };
      this.#writeVersion += 1;
      this.#reads += 1;
      this.#host.applyProgress(saved);
      if (this.view) {
        if (!this.view.units.some((unit) => unit.unitId === unitId)) {
          void this.refresh();
        } else {
          const today = this.view.today;
          const units = this.view.units.map((unit) => {
            if (unit.unitId !== unitId) return unit;
            const myPlannedDate = saved.plannedDate;
            const effectiveDate = myPlannedDate ?? unit.scheduledDate;
            const overdue = !unit.completed && effectiveDate !== null && effectiveDate < today;
            return { ...unit, myPlannedDate, effectiveDate, overdue };
          });
          this.view = recount(this.view, units);
        }
      }
      return { ok: true };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.#busyUnits.delete(unitId);
    }
  }

  async patchUnitMeta(
    unitId: string,
    patch: Pick<UnitPatch, 'amount' | 'amountUnit' | 'scheduledDate'>,
  ): Promise<ScheduleWriteResult> {
    if (!this.#host.canEdit() || this.#busyUnits.has(unitId)) return { ok: false };
    this.#busyUnits.add(unitId);
    try {
      const updated = await this.#host.courses().patchUnit(this.#host.courseId, unitId, patch);
      if (this.#host.isClosed()) return { ok: false };
      this.#writeVersion += 1;
      this.#reads += 1;
      this.#host.takeUnit(updated);
      if (this.view) {
        if (!this.view.units.some((unit) => unit.unitId === unitId)) {
          void this.refresh();
        } else {
          const today = this.view.today;
          const units = this.view.units.map((unit) => {
            if (unit.unitId !== unitId) return unit;
            const scheduledDate = updated.scheduledDate ?? null;
            const effectiveDate = unit.myPlannedDate ?? scheduledDate;
            const overdue = !unit.completed && effectiveDate !== null && effectiveDate < today;
            return {
              ...unit,
              amount: updated.amount ?? null,
              amountUnit: updated.amountUnit ?? null,
              scheduledDate,
              effectiveDate,
              overdue,
            };
          });
          this.view = recount(this.view, units);
        }
      }
      return { ok: true };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.#busyUnits.delete(unitId);
    }
  }
}
