/**
 * Personal spaced-repetition unit reviews (`unit_reviews`), workspace
 * "Today's reviews", and personal study notes (`unit_notes`) types and pure
 * validation/interval helpers.
 *
 * Mirrors `crates/model/src/unit_review.rs` so both server API calls and the
 * in-memory backend share the exact same domain shapes and rules.
 */

/** Default spaced-repetition interval sequence in days (`[1, 3, 7, 14, 30]`). */
export const DEFAULT_REVIEW_INTERVALS: readonly number[] = [1, 3, 7, 14, 30];

/** Minimum review interval in days. */
export const MIN_REVIEW_INTERVAL_DAYS = 1;

/** Maximum review interval in days (roughly ten years). */
export const MAX_REVIEW_INTERVAL_DAYS = 3650;

/** Maximum length in Unicode scalar values of `UnitNote.content`. */
export const UNIT_NOTE_CONTENT_MAX_CHARS = 10_000;

/** Course container property key holding an optional custom review interval sequence. */
export const COURSE_REVIEW_INTERVALS_PROPERTY = 'defaultReviewIntervals';

/** Filter for listing a learner's course reviews by completion state. */
export type ReviewStatusFilter = 'pending' | 'completed' | 'all';

/** Category of a learner's personal note on a unit. */
export type UnitNoteKind = 'question' | 'mistake' | 'review';

/** Ordered list of personal note categories. */
export const UNIT_NOTE_KINDS: readonly UnitNoteKind[] = ['question', 'mistake', 'review'];

/** One personal spaced-repetition review scheduled for a unit. */
export interface UnitReview {
  id: string;
  unitId: string;
  containerId: string;
  /** Scheduled calendar date (`YYYY-MM-DD`). */
  scheduledDate: string;
  /** Completion time in ms since Unix epoch; `null` while pending. */
  completedAt: number | null;
  /** Review interval in days (`1..=3650`). */
  intervalDays: number;
  createdAt: number;
  updatedAt: number;
}

/** Query parameters for listing the caller's reviews in a course. */
export interface ListCourseReviewsQuery {
  status?: ReviewStatusFilter;
  unitId?: string;
}

/** Request payload for scheduling a personal review on a unit. */
export interface ScheduleUnitReview {
  intervalDays?: number;
  scheduledDate?: string;
}

/** Request payload for completing a personal review. */
export interface CompleteUnitReview {
  scheduleNextIntervalDays?: number;
}

/** Response returned when completing a personal review. */
export interface CompletedUnitReview {
  review: UnitReview;
  suggestedNextIntervalDays: number;
  nextReview: UnitReview | null;
}

/** One pending review due today or overdue for the caller in a workspace. */
export interface TodayReviewItem {
  id: string;
  unitId: string;
  containerId: string;
  courseTitle: string;
  unitTitle: string;
  scheduledDate: string;
  intervalDays: number;
  /** `0` when due today, `> 0` when overdue. */
  overdueDays: number;
  createdAt: number;
  updatedAt: number;
}

/** One personal question, mistake, or review note on a course unit. */
export interface UnitNote {
  id: string;
  unitId: string;
  containerId: string;
  kind: UnitNoteKind;
  content: string;
  createdAt: number;
  updatedAt: number;
}

/** Request payload for creating a personal note on a unit. */
export interface CreateUnitNote {
  kind: UnitNoteKind;
  content: string;
}

/** Request payload for updating a personal note. Omitted fields stay unchanged. */
export interface PatchUnitNote {
  kind?: UnitNoteKind;
  content?: string;
}

/**
 * Returns the sorted, deduplicated review interval sequence configured on a
 * course's `properties.defaultReviewIntervals`, falling back to
 * `DEFAULT_REVIEW_INTERVALS` when absent or invalid.
 */
export function courseReviewIntervals(properties?: Record<string, unknown>): number[] {
  const raw = properties?.[COURSE_REVIEW_INTERVALS_PROPERTY];
  if (Array.isArray(raw)) {
    const valid = new Set<number>();
    for (const item of raw) {
      if (
        typeof item === 'number' &&
        Number.isInteger(item) &&
        item >= MIN_REVIEW_INTERVAL_DAYS &&
        item <= MAX_REVIEW_INTERVAL_DAYS
      ) {
        valid.add(item);
      }
    }
    if (valid.size > 0) return [...valid].sort((a, b) => a - b);
  }
  return [...DEFAULT_REVIEW_INTERVALS];
}

/**
 * Computes the suggested next review interval in days given the learner's
 * current interval (or `null`/`undefined` on initial completion) and the
 * course's interval sequence.
 */
export function suggestedNextIntervalDays(
  current: number | null | undefined,
  intervals: readonly number[] = DEFAULT_REVIEW_INTERVALS,
): number {
  const sequence = intervals.length > 0 ? intervals : DEFAULT_REVIEW_INTERVALS;
  if (current === null || current === undefined) {
    return Math.min(...sequence);
  }
  const larger = sequence.filter((candidate) => candidate > current);
  if (larger.length > 0) return Math.min(...larger);
  return Math.max(...sequence);
}

/**
 * Returns `true` if `raw` contains any Unicode C0, DEL, or C1 control
 * character other than `\n`, `\r`, or `\t`.
 */
export function hasDisallowedNoteControlChars(raw: string): boolean {
  for (const ch of raw) {
    const code = ch.codePointAt(0) ?? 0;
    if (code === 0x09 || code === 0x0a || code === 0x0d) continue;
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

/** Client/memory validation error code for `UnitNote.content`. */
export type UnitNoteContentError = 'empty' | 'too_long' | 'invalid_characters';

/** Validates `UnitNote.content` with the exact same rules as the server. */
export function validateUnitNoteContent(raw: string): UnitNoteContentError | null {
  if (hasDisallowedNoteControlChars(raw)) return 'invalid_characters';
  const trimmed = raw.trim();
  if (trimmed === '') return 'empty';
  if (Array.from(trimmed).length > UNIT_NOTE_CONTENT_MAX_CHARS) return 'too_long';
  return null;
}

/** Backend interface for personal unit reviews, workspace today reviews, and unit notes. */
export interface CourseReviews {
  listMyCourseReviews(
    courseId: string,
    query?: ListCourseReviewsQuery,
    workspaceId?: string,
  ): Promise<UnitReview[]>;
  scheduleUnitReview(
    courseId: string,
    unitId: string,
    request: ScheduleUnitReview,
    workspaceId?: string,
  ): Promise<UnitReview>;
  completeUnitReview(
    courseId: string,
    reviewId: string,
    request?: CompleteUnitReview,
    workspaceId?: string,
  ): Promise<CompletedUnitReview>;
  deleteUnitReview(courseId: string, reviewId: string, workspaceId?: string): Promise<void>;
  getWorkspaceTodayReviews(workspaceId: string): Promise<TodayReviewItem[]>;
  listMyUnitNotes(
    courseId: string,
    unitId: string,
    kind?: UnitNoteKind,
    workspaceId?: string,
  ): Promise<UnitNote[]>;
  createUnitNote(
    courseId: string,
    unitId: string,
    request: CreateUnitNote,
    workspaceId?: string,
  ): Promise<UnitNote>;
  patchUnitNote(
    courseId: string,
    noteId: string,
    patch: PatchUnitNote,
    workspaceId?: string,
  ): Promise<UnitNote>;
  deleteUnitNote(courseId: string, noteId: string, workspaceId?: string): Promise<void>;
}
