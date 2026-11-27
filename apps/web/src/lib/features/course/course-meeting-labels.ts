/**
 * Formatting, validation, and design-token helpers for course study meetings,
 * shared attendance, and course exams.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { instantOfWallTime, wallTimeOf } from '$lib/logic/date-entry.ts';
import { dateOfInstant, parseCalendarDate } from '$lib/logic/dates.ts';
import { roundTo } from '$lib/logic/format.ts';
import {
  COURSE_MEETING_LAST_MS,
  COURSE_MEETING_MAX_UNITS,
  type AttendanceStatus,
  type BackendError,
  type CourseStore,
} from '$lib/state/index.ts';

export type CourseSectionTab = 'units' | 'meetings' | 'exams';

/** Localized label for an attendance status. */
export function attendanceLabel(status: AttendanceStatus): string {
  switch (status) {
    case 'present':
      return m.course_attendance_present();
    case 'late':
      return m.course_attendance_late();
    case 'absent':
      return m.course_attendance_absent();
    case 'excused':
      return m.course_attendance_excused();
  }
}

/** Semantic token classes for an attendance status pill (no raw palette/hex colors). */
export function attendanceTone(status: AttendanceStatus): string {
  switch (status) {
    case 'present':
      return 'border-transparent bg-option-green text-foreground';
    case 'late':
      return 'border-transparent bg-option-yellow text-foreground';
    case 'absent':
      return 'border-transparent bg-option-red text-foreground';
    case 'excused':
      return 'border-transparent bg-option-blue text-foreground';
  }
}

/** Resolves a user ID to display name or the localized deleted-user tombstone label. */
export function memberDisplayLabel(course: CourseStore, userId: string): string {
  return course.workspace.userName(userId) ?? m.people_deleted_user();
}

/** Returns `location` if it is a safe `http://` or `https://` URL without whitespace, else `undefined`. */
export function safeHttpUrl(raw: string): string | undefined {
  const trimmed = raw.trim();
  if (/\s/.test(trimmed) || !/^https?:\/\//i.test(trimmed)) return undefined;
  try {
    const parsed = new URL(trimmed);
    return parsed.protocol === 'http:' || parsed.protocol === 'https:' ? parsed.href : undefined;
  } catch {
    return undefined;
  }
}

/** Formats an exam score or average with up to 2 decimal places without trailing zeros. */
export function formatScore(value: number, locale: string): string {
  const rounded = roundTo(value, 2);
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(rounded);
}

/** Splits an epoch ms instant into `YYYY-MM-DD` and `HH:MM` in `timeZone`. */
export function splitInstantToInputs(
  instant: number,
  timeZone: string,
): { date: string; time: string } {
  const wall = wallTimeOf(instant, timeZone);
  const hh = String(wall.hour).padStart(2, '0');
  const mm = String(wall.minute).padStart(2, '0');
  return { date: wall.date, time: `${hh}:${mm}` };
}

/** Default date (`today` in `timeZone`) and time (`19:00`) for a new meeting. */
export function defaultMeetingInputs(
  now: number,
  timeZone: string,
): { date: string; time: string } {
  return { date: dateOfInstant(now, timeZone), time: '19:00' };
}

/** Combines `YYYY-MM-DD` and `HH:MM` in `timeZone` into epoch milliseconds, or `undefined`. */
export function parseMeetingInstant(
  dateText: string,
  timeText: string,
  timeZone: string,
): number | undefined {
  const date = parseCalendarDate(dateText);
  const match = /^(\d{2}):(\d{2})$/.exec(timeText);
  if (!date || !match) return undefined;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  if (hour < 0 || hour > 23 || minute < 0 || minute > 59) return undefined;
  const ms = instantOfWallTime({ date, hour, minute }, timeZone);
  if (!Number.isInteger(ms) || ms < 0 || ms > COURSE_MEETING_LAST_MS) return undefined;
  return ms;
}

export interface MeetingFormErrors {
  title?: string;
  startsAt?: string;
  location?: string;
  unitIds?: string;
  notes?: string;
  member?: string;
  general?: string;
}

export function meetingErrorsFromBackend(
  error: BackendError,
  assignedMember = false,
): MeetingFormErrors {
  if (error.status === 404) {
    return assignedMember
      ? { member: m.course_meetings_error_member() }
      : { general: m.app_problem_save() };
  }
  const out: MeetingFormErrors = {};
  if (error.refusalAt('/title')) out.title = m.course_meetings_error_title();
  if (error.refusalAt('/startsAt')) out.startsAt = m.course_meetings_error_datetime();
  if (error.refusalAt('/location')) out.location = m.course_meetings_error_location();
  if (error.refusalAt('/unitIds')) {
    out.unitIds = m.course_meetings_error_units({ max: COURSE_MEETING_MAX_UNITS });
  }
  if (error.refusalAt('/notes')) out.notes = m.course_meetings_error_notes();
  if (error.refusalAt('/presenterId') || error.refusalAt('/hostId')) {
    out.member = m.course_meetings_error_member();
  }
  if (Object.keys(out).length === 0) out.general = m.app_problem_save();
  return out;
}

export interface ExamFormErrors {
  title?: string;
  examDate?: string;
  maxScore?: string;
  description?: string;
  general?: string;
}

export function examErrorsFromBackend(error: BackendError): ExamFormErrors {
  const out: ExamFormErrors = {};
  if (error.refusalAt('/title')) out.title = m.course_exams_error_title();
  if (error.refusalAt('/examDate')) out.examDate = m.course_exams_error_date();
  if (error.refusalAt('/maxScore')) out.maxScore = m.course_exams_error_max_score();
  if (error.refusalAt('/description')) out.description = m.course_exams_error_description();
  if (Object.keys(out).length === 0) out.general = m.app_problem_save();
  return out;
}
