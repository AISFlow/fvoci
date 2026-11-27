/**
 * Personal spaced-repetition reviews (`unit_reviews`) and workspace "Today's
 * reviews" in the in-memory backend, paired with personal study notes in
 * `unit-notes.ts`.
 *
 * Matches the server rules (`crates/server/src/api/unit_reviews.rs`,
 * `crates/store/src/unit_reviews.rs`, `crates/model/src/unit_review.rs`):
 * - Strict per-user isolation: each learner reads and mutates only their own rows.
 * - Unjoined open-course readers get `[]` on GET reviews/notes and `403` on writes.
 * - Chapters, trashed units, non-course containers, foreign IDs, and non-UUIDs return `404`.
 * - Completion retry is idempotent (`completedAt`/`updatedAt` preserved, `nextReview: null`).
 * - Interval suggestions respect `course.properties.defaultReviewIntervals` or `[1, 3, 7, 14, 30]`.
 * - Zero WebSocket broadcasts on review/note writes.
 */
import {
  addDays,
  dateOfInstant,
  daysBetween,
  parseCalendarDate,
  type CalendarDate,
} from '../../logic/dates.ts';
import { isTimeZone } from '../../logic/regional.ts';
import {
  courseReviewIntervals,
  MAX_REVIEW_INTERVAL_DAYS,
  MIN_REVIEW_INTERVAL_DAYS,
  suggestedNextIntervalDays,
  type CompletedUnitReview,
  type CompleteUnitReview,
  type CourseReviews,
  type CreateUnitNote,
  type ListCourseReviewsQuery,
  type PatchUnitNote,
  type ScheduleUnitReview,
  type TodayReviewItem,
  type UnitNote,
  type UnitNoteKind,
  type UnitReview,
} from '../unit-review-types.ts';

import { settingsOf } from './account.ts';
import { roleOn } from './comments.ts';
import { memberRole } from './container-access.ts';
import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import { checkWorkspace } from './spaces.ts';
import {
  memoryCreateUnitNote,
  memoryDeleteUnitNote,
  memoryListMyUnitNotes,
  memoryPatchUnitNote,
} from './unit-notes.ts';
import type { World } from './world.ts';

export {
  memoryCreateUnitNote,
  memoryDeleteUnitNote,
  memoryListMyUnitNotes,
  memoryPatchUnitNote,
} from './unit-notes.ts';

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function isUuid(id: string): boolean {
  return UUID_RE.test(id);
}

function hasMembership(world: World, userId: string, courseId: string): boolean {
  return roleOn(world.members.get(courseId) ?? [], userId) !== '';
}

function requireEnrolled(world: World, userId: string, courseId: string, detail: string): void {
  if (!hasMembership(world, userId, courseId)) fail(403, detail);
}

function requireLiveUnit(world: World, courseId: string, unitId: string): void {
  const block = world.blocks.get(unitId);
  if (block?.containerId !== courseId || block.type !== 'unit') fail(404, 'no such unit');
}

function userToday(world: World, userId: string): CalendarDate {
  const tz = settingsOf(world, userId).timeZone;
  return dateOfInstant(world.now(), tz !== 'auto' && isTimeZone(tz) ? tz : 'UTC');
}

function checkIntervalDays(raw: unknown, field: string): number {
  if (
    typeof raw !== 'number' ||
    !Number.isInteger(raw) ||
    raw < MIN_REVIEW_INTERVAL_DAYS ||
    raw > MAX_REVIEW_INTERVAL_DAYS
  ) {
    fail(400, `must be between ${MIN_REVIEW_INTERVAL_DAYS} and ${MAX_REVIEW_INTERVAL_DAYS} days`, {
      field,
      reason: 'invalid',
    });
  }
  return raw;
}

function checkCalendarDate(raw: unknown, field: string): CalendarDate {
  if (typeof raw !== 'string') {
    fail(400, 'must be a calendar date (YYYY-MM-DD)', { field, reason: 'invalid_format' });
  }
  const parsed = parseCalendarDate(raw);
  if (!parsed) {
    fail(400, 'must be a calendar date (YYYY-MM-DD)', { field, reason: 'invalid_format' });
  }
  return parsed;
}

function compareReviewsAsc(a: UnitReview, b: UnitReview): number {
  if (a.scheduledDate !== b.scheduledDate) return a.scheduledDate < b.scheduledDate ? -1 : 1;
  if (a.createdAt !== b.createdAt) return a.createdAt - b.createdAt;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/**
 * Computes `suggestedNextIntervalDays` for a completed unit in the memory
 * backend, matching `Store::latest_unit_review_intervals` + `suggested_next_interval_days`.
 */
export function memorySuggestedIntervalForUnit(
  world: World,
  userId: string,
  courseId: string,
  unitId: string,
): number {
  const course = world.containers.get(courseId);
  const intervals = courseReviewIntervals(course?.properties);
  const reviews = (world.unitReviews.get(userId) ?? []).filter(
    (review) => review.containerId === courseId && review.unitId === unitId,
  );
  if (reviews.length === 0) return suggestedNextIntervalDays(null, intervals);
  reviews.sort((a, b) => {
    const aOrder = a.completedAt ?? a.createdAt;
    const bOrder = b.completedAt ?? b.createdAt;
    if (aOrder !== bOrder) return bOrder - aOrder;
    if (a.createdAt !== b.createdAt) return b.createdAt - a.createdAt;
    return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
  });
  return suggestedNextIntervalDays(reviews[0]?.intervalDays ?? null, intervals);
}

export function memoryListMyCourseReviews(
  world: World,
  userId: string,
  courseId: string,
  query: ListCourseReviewsQuery = {},
): UnitReview[] {
  memberRole(world, userId, courseId, 'course');
  const status: string = query.status ?? 'all';
  if (status !== 'all' && status !== 'pending' && status !== 'completed') {
    fail(400, 'status must be one of pending, completed, or all', {
      field: 'status',
      reason: 'invalid',
    });
  }
  if (!hasMembership(world, userId, courseId)) return [];
  const rows = (world.unitReviews.get(userId) ?? []).filter((review) => {
    if (review.containerId !== courseId) return false;
    const block = world.blocks.get(review.unitId);
    if (block?.containerId !== courseId || block.type !== 'unit') return false;
    if (query.unitId && review.unitId !== query.unitId) return false;
    if (status === 'pending' && review.completedAt !== null) return false;
    if (status === 'completed' && review.completedAt === null) return false;
    return true;
  });
  return rows.sort(compareReviewsAsc).map(copy);
}

export function memoryScheduleUnitReview(
  world: World,
  userId: string,
  courseId: string,
  unitId: string,
  request: ScheduleUnitReview,
): UnitReview {
  memberRole(world, userId, courseId, 'course');
  requireEnrolled(world, userId, courseId, 'join the course before scheduling reviews');
  const hasInterval = 'intervalDays' in request && request.intervalDays !== undefined;
  const hasDate = 'scheduledDate' in request && request.scheduledDate !== undefined;
  if (!hasInterval && !hasDate) {
    fail(400, 'intervalDays or scheduledDate is required', {
      field: 'intervalDays',
      reason: 'required',
    });
  }
  const today = userToday(world, userId);
  const intervalInput = hasInterval
    ? checkIntervalDays(request.intervalDays, 'intervalDays')
    : null;
  const dateInput = hasDate ? checkCalendarDate(request.scheduledDate, 'scheduledDate') : null;
  requireLiveUnit(world, courseId, unitId);

  let scheduledDate: string;
  let intervalDays: number;
  if (intervalInput !== null && dateInput !== null) {
    scheduledDate = dateInput;
    intervalDays = intervalInput;
  } else if (intervalInput !== null) {
    scheduledDate = addDays(today, intervalInput);
    intervalDays = intervalInput;
  } else if (dateInput !== null) {
    const delta = daysBetween(today, dateInput);
    intervalDays = Math.min(MAX_REVIEW_INTERVAL_DAYS, Math.max(MIN_REVIEW_INTERVAL_DAYS, delta));
    scheduledDate = dateInput;
  } else {
    fail(400, 'intervalDays or scheduledDate is required', { field: 'intervalDays' });
  }

  const now = world.now();
  const created: UnitReview = {
    id: world.newId(),
    unitId,
    containerId: courseId,
    scheduledDate,
    completedAt: null,
    intervalDays,
    createdAt: now,
    updatedAt: now,
  };
  const theirs = world.unitReviews.get(userId) ?? [];
  theirs.push(created);
  world.unitReviews.set(userId, theirs);
  return copy(created);
}

export function memoryCompleteUnitReview(
  world: World,
  userId: string,
  courseId: string,
  reviewId: string,
  request: CompleteUnitReview = {},
): CompletedUnitReview {
  memberRole(world, userId, courseId, 'course');
  requireEnrolled(world, userId, courseId, 'join the course before completing reviews');
  const nextInterval =
    'scheduleNextIntervalDays' in request && request.scheduleNextIntervalDays !== undefined
      ? checkIntervalDays(request.scheduleNextIntervalDays, 'scheduleNextIntervalDays')
      : null;
  if (!isUuid(reviewId)) fail(404, 'no such unit review');
  const theirs = world.unitReviews.get(userId) ?? [];
  const review = theirs.find((item) => item.id === reviewId && item.containerId === courseId);
  if (!review) fail(404, 'no such unit review');
  requireLiveUnit(world, courseId, review.unitId);

  const course = world.containers.get(courseId);
  const intervals = courseReviewIntervals(course?.properties);
  const suggested = suggestedNextIntervalDays(review.intervalDays, intervals);

  let nextReview: UnitReview | null = null;
  if (review.completedAt === null) {
    const now = world.now();
    review.completedAt = now;
    review.updatedAt = now;
    if (nextInterval !== null) {
      const today = userToday(world, userId);
      const followUp: UnitReview = {
        id: world.newId(),
        unitId: review.unitId,
        containerId: courseId,
        scheduledDate: addDays(today, nextInterval),
        completedAt: null,
        intervalDays: nextInterval,
        createdAt: now,
        updatedAt: now,
      };
      theirs.push(followUp);
      nextReview = copy(followUp);
    }
  }
  return { review: copy(review), suggestedNextIntervalDays: suggested, nextReview };
}

export function memoryDeleteUnitReview(
  world: World,
  userId: string,
  courseId: string,
  reviewId: string,
): void {
  memberRole(world, userId, courseId, 'course');
  requireEnrolled(world, userId, courseId, 'join the course before deleting reviews');
  if (!isUuid(reviewId)) fail(404, 'no such unit review');
  const theirs = world.unitReviews.get(userId) ?? [];
  const index = theirs.findIndex((item) => item.id === reviewId && item.containerId === courseId);
  if (index === -1) fail(404, 'no such unit review');
  theirs.splice(index, 1);
}

export function memoryWorkspaceTodayReviews(
  world: World,
  userId: string,
  workspaceId: string,
): TodayReviewItem[] {
  checkWorkspace(world, userId, workspaceId);
  const today = userToday(world, userId);
  const rows: TodayReviewItem[] = [];
  for (const review of world.unitReviews.get(userId) ?? []) {
    if (review.completedAt !== null || review.scheduledDate > today) continue;
    const course = world.containers.get(review.containerId);
    if (
      course?.workspaceId !== workspaceId ||
      course.kind !== 'course' ||
      course.isTemplate ||
      !hasMembership(world, userId, course.id)
    ) {
      continue;
    }
    const block = world.blocks.get(review.unitId);
    if (block?.containerId !== course.id || block.type !== 'unit') continue;
    const parsed = parseCalendarDate(review.scheduledDate);
    const overdueDays = parsed ? Math.max(0, daysBetween(parsed, today)) : 0;
    rows.push({
      id: review.id,
      unitId: review.unitId,
      containerId: course.id,
      courseTitle: course.title,
      unitTitle: block.title,
      scheduledDate: review.scheduledDate,
      intervalDays: review.intervalDays,
      overdueDays,
      createdAt: review.createdAt,
      updatedAt: review.updatedAt,
    });
  }
  return rows.sort((a, b) => {
    if (a.scheduledDate !== b.scheduledDate) return a.scheduledDate < b.scheduledDate ? -1 : 1;
    if (a.createdAt !== b.createdAt) return a.createdAt - b.createdAt;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

export class MemoryUnitReviews implements CourseReviews {
  readonly #host: { readonly world: World; readonly userId: string };
  readonly #enter: () => Promise<void>;

  constructor(
    host: { readonly world: World; readonly userId: string },
    enter: () => Promise<void>,
  ) {
    this.#host = host;
    this.#enter = enter;
  }

  async listMyCourseReviews(
    courseId: string,
    query?: ListCourseReviewsQuery,
  ): Promise<UnitReview[]> {
    await this.#enter();
    return memoryListMyCourseReviews(this.#host.world, this.#host.userId, courseId, query);
  }

  async scheduleUnitReview(
    courseId: string,
    unitId: string,
    request: ScheduleUnitReview,
  ): Promise<UnitReview> {
    await this.#enter();
    return memoryScheduleUnitReview(this.#host.world, this.#host.userId, courseId, unitId, request);
  }

  async completeUnitReview(
    courseId: string,
    reviewId: string,
    request?: CompleteUnitReview,
  ): Promise<CompletedUnitReview> {
    await this.#enter();
    return memoryCompleteUnitReview(
      this.#host.world,
      this.#host.userId,
      courseId,
      reviewId,
      request,
    );
  }

  async deleteUnitReview(courseId: string, reviewId: string): Promise<void> {
    await this.#enter();
    memoryDeleteUnitReview(this.#host.world, this.#host.userId, courseId, reviewId);
  }

  async getWorkspaceTodayReviews(workspaceId: string): Promise<TodayReviewItem[]> {
    await this.#enter();
    return memoryWorkspaceTodayReviews(this.#host.world, this.#host.userId, workspaceId);
  }

  async listMyUnitNotes(
    courseId: string,
    unitId: string,
    kind?: UnitNoteKind,
  ): Promise<UnitNote[]> {
    await this.#enter();
    return memoryListMyUnitNotes(this.#host.world, this.#host.userId, courseId, unitId, kind);
  }

  async createUnitNote(
    courseId: string,
    unitId: string,
    request: CreateUnitNote,
  ): Promise<UnitNote> {
    await this.#enter();
    return memoryCreateUnitNote(this.#host.world, this.#host.userId, courseId, unitId, request);
  }

  async patchUnitNote(courseId: string, noteId: string, patch: PatchUnitNote): Promise<UnitNote> {
    await this.#enter();
    return memoryPatchUnitNote(this.#host.world, this.#host.userId, courseId, noteId, patch);
  }

  async deleteUnitNote(courseId: string, noteId: string): Promise<void> {
    await this.#enter();
    memoryDeleteUnitNote(this.#host.world, this.#host.userId, courseId, noteId);
  }
}
