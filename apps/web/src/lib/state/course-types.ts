/**
 * The courses of a workspace and their chapters and units, part of the data
 * boundary (`backend.ts`, `Backend.course`).
 *
 * A course is a container of the kind `course` (`Container.kind`): it is read,
 * changed, deleted, brought back, joined and filed in the sidebar as a board
 * or wiki is, by the container operations. What only a course has is here: its
 * outline of chapters (title-only top-level groupings) and units (each with a
 * body edited together in its room via `Backend.openDoc`).
 */
import type { DocNode } from '../editor/schema.ts';
import type { AmountUnit } from '../logic/course-schedule.ts';
import type { Container, ContainerVisibility, MemberRole } from '../logic/wire.ts';

import type { BodyWrite, Comment, CommentVersion } from './backend.ts';
import type { BodyVersions } from './body-versions.ts';
import type { CourseExamsApi, CourseMeetingsApi } from './course-meeting-types.ts';
import type { DeletedItem, Paging } from './trash-reads.ts';
import type { CourseReviews } from './unit-review-types.ts';

export * from './unit-review-types.ts';

/** What the reader can change of the course itself. */
export interface CourseLooks {
  title?: string;
  description?: string;
  icon?: string;
  visibility?: Container['visibility'];
  minimumRole?: Container['minimumRole'];
}

export {
  ATTENDANCE_STATUSES,
  compareCourseExams,
  compareCourseMeetings,
  COURSE_EXAM_DESCRIPTION_MAX_CHARS,
  COURSE_EXAM_MAX_SCORE_LIMIT,
  COURSE_EXAM_TITLE_MAX_CHARS,
  COURSE_MEETING_LAST_MS,
  COURSE_MEETING_LOCATION_MAX_CHARS,
  COURSE_MEETING_MAX_UNITS,
  COURSE_MEETING_NOTES_MAX_CHARS,
  COURSE_MEETING_TITLE_MAX_CHARS,
  EXAM_SCORE_NOTE_MAX_CHARS,
  isAttendanceStatus,
  type AttendanceStatus,
  type CourseExam,
  type CourseExamPatch,
  type CourseExamsApi,
  type CourseMeeting,
  type CourseMeetingPatch,
  type CourseMeetingsApi,
  type MeetingAttendance,
  type MeetingAttendanceUpdate,
  type MyExamScore,
  type MyExamScoreUpdate,
  type NewCourseExam,
  type NewCourseMeeting,
} from './course-meeting-types.ts';

export {
  AMOUNT_UNITS,
  MAX_SCHEDULE_SPAN_DAYS,
  STUDY_DAYS_ALL,
  STUDY_DAYS_FRIDAY,
  STUDY_DAYS_MONDAY,
  STUDY_DAYS_SATURDAY,
  STUDY_DAYS_SUNDAY,
  STUDY_DAYS_THURSDAY,
  STUDY_DAYS_TUESDAY,
  STUDY_DAYS_WEDNESDAY,
  STUDY_DAYS_WEEKDAYS,
  STUDY_WEEKDAYS,
  UNIT_AMOUNT_MAX,
  UNIT_AMOUNT_MIN,
  type AmountUnit,
  type StudyWeekdayKey,
} from '../logic/course-schedule.ts';

/** Whether an outline entry is a unit (with a body) or a chapter (a top-level heading). */
export type UnitKind = 'unit' | 'chapter';

/** A unit or chapter as the outline of its course lists it: without its body. */
export interface CourseUnit {
  id: string;
  courseId: string;
  kind: UnitKind;
  /** The chapter it is below; empty for a chapter or a top-level unit. */
  parentId: string;
  title: string;
  /**
   * Its place among siblings of the same parent: compared by code unit, then
   * by `id` (`logic/page-tree.ts`). The backend makes it; screens only sort by it.
   */
  order: string;
  /** Optional study amount (`1..=100_000`, units only; always `null`/absent for chapters). */
  amount?: number | null;
  /** Optional unit of study amount (`minutes`, `pages`, `problems`; units only). */
  amountUnit?: AmountUnit | null;
  /** Optional shared scheduled date (`YYYY-MM-DD`; units only). */
  scheduledDate?: string | null;
  createdAt: number;
  updatedAt: number;
}

/** A unit or chapter read on its own: with its body (`null` for a chapter) and authors. */
export interface CourseUnitDetail extends CourseUnit {
  body: DocNode | null;
  createdBy: string;
  modifiedBy: string;
}

/** What a course is made from. Whoever makes it is its admin. */
export interface CourseDraft {
  title: string;
  description?: string;
  icon?: string;
  /** `open`: anyone in the workspace can read and join; `private`: invited only. */
  visibility: ContainerVisibility;
  minimumRole?: Exclude<MemberRole, '' | 'admin'>;
}

/** A unit or chapter to make; the backend gives it its identifier, times and `order`. */
export interface UnitDraft {
  /** Whether to make a `unit` (default) or a `chapter`. */
  kind?: UnitKind;
  title?: string;
  /** The chapter it goes below; empty or left out for the top. Must be empty for a chapter. */
  parentId?: string;
  /** The sibling it goes right after: empty for the first place, left out for the last. */
  after?: string;
  /** Optional study amount (`1..=100_000`, units only; forbidden on chapters). */
  amount?: number | null;
  /** Optional study amount unit (`minutes`, `pages`, `problems`; units only). */
  amountUnit?: AmountUnit | null;
  /** Optional shared scheduled date (`YYYY-MM-DD`, units only). */
  scheduledDate?: string | null;
}

/** Where a unit or chapter moves within its course. Chapters stay at the top (`parentId` empty). */
export interface UnitMove {
  parentId?: string;
  after?: string;
}

/** What a unit or chapter is changed to; omitted fields stay. Chapters take no `body` or schedule fields. */
export interface UnitPatch {
  title?: string;
  body?: DocNode | null;
  /** Tri-state study amount: omitted = keep, `null` = clear, integer `1..=100_000` = set. */
  amount?: number | null;
  /** Tri-state study amount unit: omitted = keep, `null` = clear, value = set. */
  amountUnit?: AmountUnit | null;
  /** Tri-state shared scheduled date (`YYYY-MM-DD`): omitted = keep, `null` = clear, date = set. */
  scheduledDate?: string | null;
  /** The `updatedAt` the unit was read at: 409 if the unit changed since. */
  readAt?: number;
}

/** Maximum length in Unicode scalar values of `UnitProgress.lastPosition`. */
export const UNIT_PROGRESS_POSITION_MAX_CHARS = 256;

/** Returns `true` if `raw` contains any Unicode C0, DEL, or C1 control character. */
export function hasControlChars(raw: string): boolean {
  for (const ch of raw) {
    const code = ch.codePointAt(0) ?? 0;
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

/** One learner's personal progress on a single unit. */
export interface UnitProgress {
  unitId: string;
  /** Completion time in milliseconds since Unix epoch; `null` if incomplete. */
  completedAt: number | null;
  /** Optional personal target date (`YYYY-MM-DD`), reserved for L3 scheduling. */
  plannedDate: string | null;
  /** Optional last read or watched position string, at most 256 Unicode scalar values. */
  lastPosition: string | null;
  /** Suggested next spaced-repetition review interval in days when completed. */
  suggestedNextIntervalDays?: number;
  /** Last modification time in milliseconds since Unix epoch. */
  updatedAt: number;
}

/**
 * Partial update for the caller's personal progress on a unit.
 * Omitted fields keep their stored value; never pass `completed: null`.
 */
export interface UnitProgressUpdate {
  completed?: boolean;
  lastPosition?: string | null;
}

/** Aggregate completion count of one active course member. */
export interface MemberCourseProgress {
  userId: string;
  completedUnits: number;
}

/** Course-wide completion summary across all active course members. */
export interface CourseProgressSummary {
  totalUnits: number;
  members: MemberCourseProgress[];
}

/** Saved shared study schedule of a course. */
export interface CourseSchedule {
  startDate: string;
  targetDate: string;
  /** Weekday bitmask (`1..=127`; bit 0 = Monday .. bit 6 = Sunday). */
  studyDays: number;
  updatedAt: number;
}

/** Payload for saving a course's shared schedule (`putCourseSchedule`). */
export interface CourseScheduleUpdate {
  startDate: string;
  targetDate: string;
  studyDays: number;
  autoAssign?: boolean;
}

/** One live unit in a course's schedule view, combining shared and caller's personal dates. */
export interface ScheduledUnit {
  unitId: string;
  parentId: string;
  title: string;
  amount: number | null;
  amountUnit: AmountUnit | null;
  scheduledDate: string | null;
  myPlannedDate: string | null;
  effectiveDate: string | null;
  completed: boolean;
  overdue: boolean;
}

/** Full schedule view of a course for the calling user. */
export interface CourseScheduleView {
  schedule: CourseSchedule | null;
  /** Caller's today (`YYYY-MM-DD`) in `UserSettings.timeZone` (UTC when `auto`). */
  today: string;
  todayCount: number;
  overdueCount: number;
  units: ScheduledUnit[];
}

/** Optional window overrides for manual shared or personal redistribution. */
export interface RedistributeScheduleRequest {
  fromDate?: string | null;
  targetDate?: string | null;
  studyDays?: number | null;
}

/** Personal planned-date override update (`plannedDate` is required on the wire; `null` clears). */
export interface UnitScheduleUpdate {
  plannedDate: string | null;
}

/** One incomplete unit due today or overdue in a workspace's today learning feed. */
export interface LearningTodayItem {
  courseId: string;
  courseTitle: string;
  unitId: string;
  parentId: string;
  title: string;
  amount: number | null;
  amountUnit: AmountUnit | null;
  scheduledDate: string | null;
  myPlannedDate: string | null;
  effectiveDate: string;
}

/** Caller's incomplete units due today and overdue across enrolled live courses in a workspace. */
export interface WorkspaceLearningToday {
  today: string;
  todayUnits: LearningTodayItem[];
  overdueUnits: LearningTodayItem[];
}

export interface Courses {
  /** The courses of the workspace the reader can see (member or open). */
  courses(workspaceId: string): Promise<Container[]>;
  /** Makes a course, filed under the reader's default category. */
  createCourse(workspaceId: string, draft: CourseDraft): Promise<Container>;
  /** Every chapter and unit of the course, in no promised order. */
  units(courseId: string): Promise<CourseUnit[]>;
  unit(courseId: string, unitId: string): Promise<CourseUnitDetail>;
  /** For editors. Invalid `parentId`/`after` is 400; limits are `too_many_units` / `too_many_chapters`. */
  createUnit(courseId: string, draft: UnitDraft): Promise<CourseUnitDetail>;
  patchUnit(courseId: string, unitId: string, patch: UnitPatch): Promise<CourseUnitDetail>;
  /** Moves a unit, or a chapter with all units below it, to the course's trash. */
  deleteUnit(courseId: string, unitId: string): Promise<void>;
  /** Moves a unit or chapter within its course; moving a chapter keeps its units below it. */
  moveUnit(courseId: string, unitId: string, to: UnitMove): Promise<CourseUnitDetail>;
  /** The course's deleted chapters and units, newest first. */
  deletedUnits(courseId: string, paging?: Paging): Promise<DeletedItem[]>;
  /** Restores a unit (or a chapter with the units deleted with it) from the trash. */
  restoreUnit(courseId: string, unitId: string): Promise<CourseUnitDetail>;

  // Comments on a unit (chapters return 404).
  /** Oldest first. Open-course unjoined workspace members may read. */
  comments(courseId: string, unitId: string): Promise<Comment[]>;
  /** For the course's commenters and up. A comment must be non-empty and have no file blocks. */
  createComment(courseId: string, unitId: string, body: DocNode): Promise<Comment>;
  /** Replaces a comment's text; 409 when it changed since `readAt`. */
  patchComment(
    courseId: string,
    unitId: string,
    commentId: string,
    write: BodyWrite,
  ): Promise<Comment>;
  /** Moves a unit comment to the trash. */
  deleteComment(courseId: string, unitId: string, commentId: string): Promise<void>;
  /** Restores a deleted unit comment onto its unit. */
  restoreComment(courseId: string, unitId: string, commentId: string): Promise<Comment>;
  /** Previous texts of a unit comment. */
  commentVersions(courseId: string, unitId: string, commentId: string): Promise<CommentVersion[]>;
  /** The versions of a unit's body (chapters return 404). */
  readonly bodyVersions: BodyVersions;

  // Subscriptions (watching) for shared-workspace units.
  /** Block IDs the user is subscribed to in the workspace. */
  watching(workspaceId: string, userId: string): Promise<string[]>;
  /** Subscribes the user to notifications on a unit. */
  watchUnit(workspaceId: string, unitId: string, userId: string): Promise<void>;
  /** Unsubscribes the user from notifications on a unit. */
  unwatchUnit(workspaceId: string, unitId: string, userId: string): Promise<void>;

  // Personal unit progress & course summary (never called on public links).
  /** The caller's own progress rows for live units of the course, ordered by `unitId`. */
  myProgress(courseId: string, workspaceId?: string): Promise<UnitProgress[]>;
  /** Creates or updates the caller's progress on a live unit (chapters/trashed 404; unjoined 403). */
  putUnitProgress(
    courseId: string,
    unitId: string,
    update: UnitProgressUpdate,
    workspaceId?: string,
  ): Promise<UnitProgress>;
  /** Aggregate completion counts for active course members, ordered by `userId`. */
  progressSummary(courseId: string, workspaceId?: string): Promise<CourseProgressSummary>;

  // Personal spaced-repetition reviews, workspace today reviews, and personal study notes.
  readonly reviews: CourseReviews;

  // Study schedule, manual redistribution, personal planned dates & today's learning (never on public links).
  /** Course schedule view in outline tree order for the caller (unjoined gets `myPlannedDate: null`). */
  schedule(courseId: string, workspaceId?: string): Promise<CourseScheduleView>;
  /** Saves the course's shared schedule and optionally auto-assigns unit `scheduledDate`s. */
  putSchedule(
    courseId: string,
    update: CourseScheduleUpdate,
    workspaceId?: string,
  ): Promise<CourseScheduleView>;
  /** Clears the saved course schedule row without changing unit or personal dates. */
  deleteSchedule(courseId: string, workspaceId?: string): Promise<void>;
  /** Redistributes shared `scheduledDate` across all live units (editors/admins only). */
  redistributeSchedule(
    courseId: string,
    request: RedistributeScheduleRequest,
    workspaceId?: string,
  ): Promise<CourseScheduleView>;
  /** Redistributes only caller's incomplete units into personal `plannedDate` (any enrolled member). */
  redistributeMySchedule(
    courseId: string,
    request: RedistributeScheduleRequest,
    workspaceId?: string,
  ): Promise<CourseScheduleView>;
  /** Sets or clears (`null`) caller's personal `plannedDate` on a live unit (any enrolled member). */
  putUnitSchedule(
    courseId: string,
    unitId: string,
    update: UnitScheduleUpdate,
    workspaceId?: string,
  ): Promise<UnitProgress>;
  /** Caller's incomplete today and overdue units across enrolled live courses of a workspace. */
  learningToday(workspaceId: string): Promise<WorkspaceLearningToday>;

  /** Course study meetings and shared attendance (never called on public links). */
  readonly meetings: CourseMeetingsApi;
  /** Course exams and strictly personal exam scores (never called on public links). */
  readonly exams: CourseExamsApi;
}

/** Where a backend keeps its courses. One by a public link reads only its course's outline and units. */
export interface CoursesOfBackend {
  readonly course?: Courses;
}
