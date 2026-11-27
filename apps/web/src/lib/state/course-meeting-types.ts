/**
 * Course study meetings, shared attendance records, course exams, and strictly
 * personal exam scores with aggregate course averages (`Backend.course.meetings`
 * and `Backend.course.exams`).
 *
 * Personal exam scores (`MyExamScore`) belong strictly to the calling learner:
 * other course members never receive another member's score or note, only the
 * course-wide `averageScore` and `scoreCount` across active enrolled members.
 */

export const ATTENDANCE_STATUSES = ['present', 'late', 'absent', 'excused'] as const;

export type AttendanceStatus = (typeof ATTENDANCE_STATUSES)[number];

/** Maximum length in Unicode scalar values of `CourseMeeting.title`. */
export const COURSE_MEETING_TITLE_MAX_CHARS = 200;
/** Maximum length in Unicode scalar values of `CourseMeeting.location`. */
export const COURSE_MEETING_LOCATION_MAX_CHARS = 500;
/** Maximum length in Unicode scalar values of `CourseMeeting.notes`. */
export const COURSE_MEETING_NOTES_MAX_CHARS = 5000;
/** Maximum number of unit identifiers linked to a single meeting. */
export const COURSE_MEETING_MAX_UNITS = 200;
/** Maximum permitted `startsAt` timestamp (end of year 9999 UTC). */
export const COURSE_MEETING_LAST_MS = 253_402_300_799_999;

/** Maximum length in Unicode scalar values of `CourseExam.title`. */
export const COURSE_EXAM_TITLE_MAX_CHARS = 200;
/** Maximum length in Unicode scalar values of `CourseExam.description`. */
export const COURSE_EXAM_DESCRIPTION_MAX_CHARS = 5000;
/** Maximum length in Unicode scalar values of `MyExamScore.note`. */
export const EXAM_SCORE_NOTE_MAX_CHARS = 1000;
/** Maximum permitted `CourseExam.maxScore`. */
export const COURSE_EXAM_MAX_SCORE_LIMIT = 1_000_000_000;

/** One member's attendance record on a study meeting. */
export interface MeetingAttendance {
  /** Member user ID (retained as a tombstone if the account is erased). */
  userId: string;
  status: AttendanceStatus;
  updatedAt: number;
}

/** A study meeting belonging to a course, together with its attendance records. */
export interface CourseMeeting {
  id: string;
  containerId: string;
  title: string;
  startsAt: number;
  location: string;
  unitIds: string[];
  presenterId: string | null;
  hostId: string | null;
  notes: string;
  notifiedAt: number | null;
  createdAt: number;
  updatedAt: number;
  /** Attendance records ordered by `userId ASC`. */
  attendance: MeetingAttendance[];
}

/** Request payload for creating a study meeting. */
export interface NewCourseMeeting {
  title: string;
  startsAt: number;
  location?: string;
  unitIds?: string[];
  presenterId?: string | null;
  hostId?: string | null;
  notes?: string;
}

/**
 * Request payload for patching a study meeting.
 * Omitted fields are kept; `null` for `presenterId` or `hostId` clears the assignment.
 */
export interface CourseMeetingPatch {
  title?: string;
  startsAt?: number;
  location?: string;
  unitIds?: string[];
  presenterId?: string | null;
  hostId?: string | null;
  notes?: string;
}

/** Request payload for recording a member's attendance on a meeting. */
export interface MeetingAttendanceUpdate {
  status: AttendanceStatus;
}

/** The calling learner's own personal score on an exam (never exposed to others). */
export interface MyExamScore {
  score: number;
  note: string;
  updatedAt: number;
}

/** A course exam with the caller's own score and aggregate stats across active members. */
export interface CourseExam {
  id: string;
  containerId: string;
  title: string;
  examDate: string | null;
  maxScore: number;
  description: string;
  createdAt: number;
  updatedAt: number;
  /** Caller's own score row, or `null` if none / unjoined (never another member's score). */
  myScore: MyExamScore | null;
  /**
   * The published snapshot's mean score, rounded to 0.1; `null` while the
   * results are not published (a backend without publication may tell it
   * all the same), or for an unjoined reader.
   */
  averageScore: number | null;
  /** Number of active enrolled members who recorded a score: exact for its managers, counted down to fives for the rest. */
  scoreCount: number;
  /** When a manager of the course published the results; `null` while they are not. */
  resultsPublishedAt: number | null;
  /** How many had scored when the results were published, counted down to fives; `null` while they are not. */
  publishedScoreBucket: number | null;
}

/** Request payload for creating a course exam. */
export interface NewCourseExam {
  title: string;
  examDate?: string | null;
  maxScore: number;
  description?: string;
}

/**
 * Request payload for patching a course exam.
 * Omitted fields are kept; `examDate: null` clears the exam date.
 */
export interface CourseExamPatch {
  title?: string;
  examDate?: string | null;
  maxScore?: number;
  description?: string;
}

/** Request payload for saving or updating the caller's own exam score. */
export interface MyExamScoreUpdate {
  score: number;
  note?: string;
}

/** Course study meeting & attendance operations (never called on public share links). */
export interface CourseMeetingsApi {
  list(courseId: string, workspaceId?: string): Promise<CourseMeeting[]>;
  create(courseId: string, draft: NewCourseMeeting, workspaceId?: string): Promise<CourseMeeting>;
  patch(
    courseId: string,
    meetingId: string,
    patch: CourseMeetingPatch,
    workspaceId?: string,
  ): Promise<CourseMeeting>;
  delete(courseId: string, meetingId: string, workspaceId?: string): Promise<void>;
  putAttendance(
    courseId: string,
    meetingId: string,
    userId: string,
    update: MeetingAttendanceUpdate,
    workspaceId?: string,
  ): Promise<MeetingAttendance>;
}

/** Course exam & personal score operations (never called on public share links). */
export interface CourseExamsApi {
  list(courseId: string, workspaceId?: string): Promise<CourseExam[]>;
  create(courseId: string, draft: NewCourseExam, workspaceId?: string): Promise<CourseExam>;
  patch(
    courseId: string,
    examId: string,
    patch: CourseExamPatch,
    workspaceId?: string,
  ): Promise<CourseExam>;
  delete(courseId: string, examId: string, workspaceId?: string): Promise<void>;
  putMyScore(
    courseId: string,
    examId: string,
    update: MyExamScoreUpdate,
    workspaceId?: string,
  ): Promise<MyExamScore>;
  deleteMyScore(courseId: string, examId: string, workspaceId?: string): Promise<void>;
  /** Publishes the snapshot of the results, or takes it back; the exam as it then is. */
  setResultsPublished(
    courseId: string,
    examId: string,
    published: boolean,
    workspaceId?: string,
  ): Promise<CourseExam>;
}

export function isAttendanceStatus(value: unknown): value is AttendanceStatus {
  return value === 'present' || value === 'late' || value === 'absent' || value === 'excused';
}

export function compareCourseMeetings(a: CourseMeeting, b: CourseMeeting): number {
  return a.startsAt - b.startsAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}

export function compareCourseExams(a: CourseExam, b: CourseExam): number {
  if (a.examDate !== b.examDate) {
    if (a.examDate === null) return 1;
    if (b.examDate === null) return -1;
    return a.examDate < b.examDate ? -1 : 1;
  }
  return a.createdAt - b.createdAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}
