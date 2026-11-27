/**
 * Reactive store for the workspace "Today's learning" feed
 * (`GET /w/{workspaceId}/learning/today`).
 *
 * Lists the caller's incomplete overdue units (sorted by effective date asc)
 * and incomplete units due today across all enrolled live courses, and lets the
 * learner mark any item complete directly from the list.
 */
import { SvelteSet } from 'svelte/reactivity';

import type { LearningTodayItem } from './course-types.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';

export class LearningTodayStore {
  readonly workspace: Workspace;

  status = $state<LoadStatus>('idle');
  today = $state('');
  todayUnits = $state.raw<LearningTodayItem[]>([]);
  overdueUnits = $state.raw<LearningTodayItem[]>([]);
  readonly #completing = new SvelteSet<string>();
  #reads = 0;
  #closed = false;

  constructor(workspace: Workspace) {
    this.workspace = workspace;
  }

  isCompleting(unitId: string): boolean {
    return this.#completing.has(unitId);
  }

  #isClosed(): boolean {
    return this.#closed;
  }

  close(): void {
    this.#closed = true;
    this.#reads += 1;
  }

  async load(workspaceId: string): Promise<void> {
    const courses = this.workspace.backend.course;
    if (!courses || this.#isClosed()) return;
    this.#reads += 1;
    const reading = this.#reads;
    this.status = 'loading';
    try {
      const data = await courses.learningToday(workspaceId);
      if (this.#isClosed() || reading !== this.#reads) return;
      this.today = data.today;
      this.todayUnits = data.todayUnits;
      this.overdueUnits = data.overdueUnits;
      this.status = 'ready';
    } catch (error) {
      if (this.#isClosed() || reading !== this.#reads) return;
      this.status = 'failed';
      this.workspace.report('load', error);
    }
  }

  async completeUnit(item: LearningTodayItem, workspaceId: string): Promise<boolean> {
    const courses = this.workspace.backend.course;
    if (!courses || this.#isClosed() || this.#completing.has(item.unitId)) return false;
    this.#completing.add(item.unitId);
    try {
      await courses.putUnitProgress(item.courseId, item.unitId, { completed: true }, workspaceId);
      if (this.#isClosed()) return false;
      this.todayUnits = this.todayUnits.filter((one) => one.unitId !== item.unitId);
      this.overdueUnits = this.overdueUnits.filter((one) => one.unitId !== item.unitId);
      return true;
    } catch (error) {
      if (!this.#isClosed()) this.workspace.report('save', error);
      return false;
    } finally {
      this.#completing.delete(item.unitId);
    }
  }
}
