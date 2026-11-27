/**
 * Reactive Svelte 5 state stores for personal unit reviews, personal study
 * notes, and workspace "Today's reviews".
 *
 * - `CourseReviewsStore`: scoped per `CourseStore` via `courseReviews(course)`.
 *   Holds the caller's reviews across the course (sorted by `scheduledDate ASC`)
 *   and the caller's personal notes for the currently open unit (newest first).
 * - `TodayReviewsStore`: scoped per `Workspace` via `todayReviews(workspace)`.
 *   Holds pending reviews due today or overdue across enrolled courses in the
 *   current workspace, and stays synchronized with mutations from any unit.
 */
import { SvelteSet } from 'svelte/reactivity';

import type { CourseStore } from './course.svelte.ts';
import {
  courseReviewIntervals,
  type CompletedUnitReview,
  type CompleteUnitReview,
  type CourseReviews,
  type CreateUnitNote,
  type PatchUnitNote,
  type ScheduleUnitReview,
  type TodayReviewItem,
  type UnitNote,
  type UnitReview,
} from './unit-review-types.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';

function sortReviewsAsc(rows: readonly UnitReview[]): UnitReview[] {
  return [...rows].sort((a, b) => {
    if (a.scheduledDate !== b.scheduledDate) return a.scheduledDate < b.scheduledDate ? -1 : 1;
    if (a.createdAt !== b.createdAt) return a.createdAt - b.createdAt;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

function sortNotesDesc(rows: readonly UnitNote[]): UnitNote[] {
  return [...rows].sort((a, b) => {
    if (a.createdAt !== b.createdAt) return b.createdAt - a.createdAt;
    return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
  });
}

export class TodayReviewsStore {
  readonly #workspace: Workspace;

  items = $state.raw<TodayReviewItem[]>([]);
  status = $state<LoadStatus>('idle');
  /** Incremented whenever any review is scheduled, completed, or deleted. */
  version = $state(0);
  /** Workspace ID whose `items` are currently loaded. */
  loadedWorkspaceId = $state<string | undefined>(undefined);
  /** Review IDs currently awaiting a complete/delete mutation. */
  readonly busyIds = new SvelteSet<string>();

  #loadSeq = 0;

  constructor(workspace: Workspace) {
    this.#workspace = workspace;
  }

  get #api(): CourseReviews | undefined {
    return this.#workspace.backend.course?.reviews;
  }

  intervalsFor(containerId: string): number[] {
    const course = this.#workspace.containers.find((candidate) => candidate.id === containerId);
    return courseReviewIntervals(course?.properties);
  }

  notifyChanged(): void {
    this.version += 1;
  }

  async load(workspaceId = this.#workspace.info?.id): Promise<void> {
    const api = this.#api;
    if (!api || !workspaceId) {
      this.items = [];
      this.loadedWorkspaceId = workspaceId;
      this.status = 'ready';
      return;
    }
    const seq = ++this.#loadSeq;
    if (this.loadedWorkspaceId !== workspaceId) {
      this.items = [];
      this.loadedWorkspaceId = workspaceId;
    }
    this.status = 'loading';
    try {
      const rows = await api.getWorkspaceTodayReviews(workspaceId);
      if (seq !== this.#loadSeq) return;
      this.items = rows;
      this.status = 'ready';
    } catch (error) {
      if (seq !== this.#loadSeq) return;
      this.status = 'failed';
      this.#workspace.report('load', error);
    }
  }

  async complete(
    item: TodayReviewItem,
    request: CompleteUnitReview = {},
  ): Promise<CompletedUnitReview | undefined> {
    const api = this.#api;
    if (!api || this.busyIds.has(item.id)) return undefined;
    this.busyIds.add(item.id);
    try {
      const result = await api.completeUnitReview(
        item.containerId,
        item.id,
        request,
        this.loadedWorkspaceId ?? this.#workspace.info?.id,
      );
      this.#loadSeq += 1;
      this.items = this.items.filter((row) => row.id !== item.id);
      return result;
    } catch (error) {
      this.#workspace.report('save', error);
      return undefined;
    } finally {
      this.busyIds.delete(item.id);
    }
  }
}

export class CourseReviewsStore {
  readonly course: CourseStore;

  reviews = $state.raw<UnitReview[]>([]);
  reviewsStatus = $state<LoadStatus>('idle');
  reviewsBusy = $state(false);
  readonly reviewBusyIds = new SvelteSet<string>();

  notes = $state.raw<UnitNote[]>([]);
  notesUnitId = $state<string | undefined>(undefined);
  notesStatus = $state<LoadStatus>('idle');
  notesBusy = $state(false);
  readonly noteBusyIds = new SvelteSet<string>();

  #reviewsSeq = 0;
  #notesSeq = 0;
  #reviewsWriteVersion = 0;
  #notesWriteVersion = 0;

  constructor(course: CourseStore) {
    this.course = course;
  }

  get #api(): CourseReviews | undefined {
    return this.course.workspace.backend.course?.reviews;
  }

  get #workspaceId(): string | undefined {
    return this.course.course?.workspaceId ?? this.course.workspace.info?.id;
  }

  #canTrack(): boolean {
    return this.course.canTrackProgress;
  }

  reviewsForUnit(unitId: string): UnitReview[] {
    return this.reviews.filter((review) => review.unitId === unitId);
  }

  clear(): void {
    this.#reviewsSeq += 1;
    this.#reviewsWriteVersion += 1;
    this.reviews = [];
    this.reviewsStatus = 'idle';
    this.clearNotes();
  }

  clearNotes(): void {
    this.#notesSeq += 1;
    this.#notesWriteVersion += 1;
    this.notes = [];
    this.notesUnitId = undefined;
    this.notesStatus = 'idle';
  }

  async loadReviews(): Promise<void> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress) {
      this.#reviewsSeq += 1;
      this.reviews = [];
      this.reviewsStatus = 'ready';
      return;
    }
    const seq = ++this.#reviewsSeq;
    const writeVersion = this.#reviewsWriteVersion;
    this.reviewsStatus = 'loading';
    try {
      const rows = await api.listMyCourseReviews(
        this.course.courseId,
        { status: 'all' },
        this.#workspaceId,
      );
      if (seq !== this.#reviewsSeq || !this.#canTrack()) return;
      if (writeVersion !== this.#reviewsWriteVersion) {
        this.reviewsStatus = 'ready';
        return;
      }
      this.reviews = sortReviewsAsc(rows);
      this.reviewsStatus = 'ready';
    } catch (error) {
      if (seq !== this.#reviewsSeq) return;
      this.reviewsStatus = 'failed';
      this.course.workspace.report('load', error);
    }
  }

  async scheduleReview(
    unitId: string,
    request: ScheduleUnitReview,
  ): Promise<UnitReview | undefined> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress || this.reviewsBusy) return undefined;
    this.reviewsBusy = true;
    try {
      const created = await api.scheduleUnitReview(
        this.course.courseId,
        unitId,
        request,
        this.#workspaceId,
      );
      if (!this.#canTrack()) return undefined;
      this.#reviewsWriteVersion += 1;
      this.reviews = sortReviewsAsc([...this.reviews, created]);
      this.reviewsStatus = 'ready';
      todayReviews(this.course.workspace).notifyChanged();
      return created;
    } catch (error) {
      this.course.workspace.report('save', error);
      return undefined;
    } finally {
      this.reviewsBusy = false;
    }
  }

  async completeReview(
    reviewId: string,
    request: CompleteUnitReview = {},
  ): Promise<CompletedUnitReview | undefined> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress || this.reviewBusyIds.has(reviewId)) {
      return undefined;
    }
    this.reviewBusyIds.add(reviewId);
    try {
      const completed = await api.completeUnitReview(
        this.course.courseId,
        reviewId,
        request,
        this.#workspaceId,
      );
      if (!this.#canTrack()) return undefined;
      this.#reviewsWriteVersion += 1;
      const updated = this.reviews.map((row) =>
        row.id === completed.review.id ? completed.review : row,
      );
      if (completed.nextReview && !updated.some((row) => row.id === completed.nextReview?.id)) {
        updated.push(completed.nextReview);
      }
      this.reviews = sortReviewsAsc(updated);
      this.reviewsStatus = 'ready';
      todayReviews(this.course.workspace).notifyChanged();
      return completed;
    } catch (error) {
      this.course.workspace.report('save', error);
      return undefined;
    } finally {
      this.reviewBusyIds.delete(reviewId);
    }
  }

  async deleteReview(reviewId: string): Promise<boolean> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress || this.reviewBusyIds.has(reviewId)) return false;
    this.reviewBusyIds.add(reviewId);
    try {
      await api.deleteUnitReview(this.course.courseId, reviewId, this.#workspaceId);
      if (!this.#canTrack()) return false;
      this.#reviewsWriteVersion += 1;
      this.reviews = this.reviews.filter((row) => row.id !== reviewId);
      todayReviews(this.course.workspace).notifyChanged();
      return true;
    } catch (error) {
      this.course.workspace.report('save', error);
      return false;
    } finally {
      this.reviewBusyIds.delete(reviewId);
    }
  }

  async loadNotes(unitId: string): Promise<void> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress) {
      this.#notesSeq += 1;
      this.notesUnitId = unitId;
      this.notes = [];
      this.notesStatus = 'ready';
      return;
    }
    const seq = ++this.#notesSeq;
    const writeVersion = this.#notesWriteVersion;
    if (this.notesUnitId !== unitId) {
      this.notesUnitId = unitId;
      this.notes = [];
    }
    this.notesStatus = 'loading';
    try {
      const rows = await api.listMyUnitNotes(
        this.course.courseId,
        unitId,
        undefined,
        this.#workspaceId,
      );
      if (seq !== this.#notesSeq || this.notesUnitId !== unitId || !this.#canTrack()) {
        return;
      }
      if (writeVersion !== this.#notesWriteVersion) {
        this.notesStatus = 'ready';
        return;
      }
      this.notes = sortNotesDesc(rows);
      this.notesStatus = 'ready';
    } catch (error) {
      if (seq !== this.#notesSeq) return;
      this.notesStatus = 'failed';
      this.course.workspace.report('load', error);
    }
  }

  async createNote(unitId: string, request: CreateUnitNote): Promise<UnitNote | undefined> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress || this.notesBusy) return undefined;
    this.notesBusy = true;
    try {
      const created = await api.createUnitNote(
        this.course.courseId,
        unitId,
        request,
        this.#workspaceId,
      );
      if (this.notesUnitId === unitId && this.#canTrack()) {
        this.#notesWriteVersion += 1;
        this.notes = sortNotesDesc([created, ...this.notes]);
        this.notesStatus = 'ready';
      }
      return created;
    } catch (error) {
      this.course.workspace.report('save', error);
      return undefined;
    } finally {
      this.notesBusy = false;
    }
  }

  async patchNote(noteId: string, patch: PatchUnitNote): Promise<UnitNote | undefined> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress || this.noteBusyIds.has(noteId)) return undefined;
    this.noteBusyIds.add(noteId);
    try {
      const updated = await api.patchUnitNote(
        this.course.courseId,
        noteId,
        patch,
        this.#workspaceId,
      );
      if (this.notesUnitId === updated.unitId && this.#canTrack()) {
        this.#notesWriteVersion += 1;
        this.notes = this.notes.map((row) => (row.id === updated.id ? updated : row));
        this.notesStatus = 'ready';
      }
      return updated;
    } catch (error) {
      this.course.workspace.report('save', error);
      return undefined;
    } finally {
      this.noteBusyIds.delete(noteId);
    }
  }

  async deleteNote(noteId: string): Promise<boolean> {
    const api = this.#api;
    if (!api || !this.course.canTrackProgress || this.noteBusyIds.has(noteId)) return false;
    this.noteBusyIds.add(noteId);
    try {
      await api.deleteUnitNote(this.course.courseId, noteId, this.#workspaceId);
      if (!this.#canTrack()) return false;
      this.#notesWriteVersion += 1;
      this.notes = this.notes.filter((row) => row.id !== noteId);
      return true;
    } catch (error) {
      this.course.workspace.report('save', error);
      return false;
    } finally {
      this.noteBusyIds.delete(noteId);
    }
  }
}

const byCourse = new WeakMap<CourseStore, CourseReviewsStore>();
const byWorkspace = new WeakMap<Workspace, TodayReviewsStore>();

/** Returns the personal reviews & notes store for a `CourseStore`. */
export function courseReviews(course: CourseStore): CourseReviewsStore {
  let store = byCourse.get(course);
  if (!store) {
    store = new CourseReviewsStore(course);
    byCourse.set(course, store);
  }
  return store;
}

/** Returns the workspace-scoped "Today's reviews" store for a `Workspace`. */
export function todayReviews(workspace: Workspace): TodayReviewsStore {
  let store = byWorkspace.get(workspace);
  if (!store) {
    store = new TodayReviewsStore(workspace);
    byWorkspace.set(workspace, store);
  }
  return store;
}
