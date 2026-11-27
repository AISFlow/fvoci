/**
 * Localized labels, formatting, and validation error helpers shared across
 * course schedule components (`features/course/`).
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { ScheduleWindowIssue } from '$lib/logic/course-schedule.ts';
import {
  MAX_SCHEDULE_SPAN_DAYS,
  type AmountUnit,
  type BackendError,
  type StudyWeekdayKey,
} from '$lib/state/index.ts';

export function weekdayLabel(key: StudyWeekdayKey): string {
  switch (key) {
    case 'mon':
      return m.course_schedule_weekday_mon();
    case 'tue':
      return m.course_schedule_weekday_tue();
    case 'wed':
      return m.course_schedule_weekday_wed();
    case 'thu':
      return m.course_schedule_weekday_thu();
    case 'fri':
      return m.course_schedule_weekday_fri();
    case 'sat':
      return m.course_schedule_weekday_sat();
    case 'sun':
      return m.course_schedule_weekday_sun();
  }
}

export function amountUnitLabel(unit: AmountUnit): string {
  switch (unit) {
    case 'minutes':
      return m.course_amount_unit_minutes();
    case 'pages':
      return m.course_amount_unit_pages();
    case 'problems':
      return m.course_amount_unit_problems();
  }
}

/** Full display label for a unit's study amount (e.g. "30 min", "20쪽"). */
export function formatAmount(
  amount: number | null | undefined,
  unit: AmountUnit | null | undefined,
): string {
  if (amount === null || amount === undefined) return '';
  switch (unit) {
    case 'minutes':
      return m.course_amount_minutes({ count: amount });
    case 'pages':
      return m.course_amount_pages({ count: amount });
    case 'problems':
      return m.course_amount_problems({ count: amount });
    default:
      return String(amount);
  }
}

/** Compact display label for tree badges (e.g. "30m", "20쪽"). */
export function formatAmountShort(
  amount: number | null | undefined,
  unit: AmountUnit | null | undefined,
): string {
  if (amount === null || amount === undefined) return '';
  switch (unit) {
    case 'minutes':
      return m.course_amount_short_minutes({ count: amount });
    case 'pages':
      return m.course_amount_short_pages({ count: amount });
    case 'problems':
      return m.course_amount_short_problems({ count: amount });
    default:
      return String(amount);
  }
}

/** Compact `MM-DD` label from a `YYYY-MM-DD` calendar date string. */
export function shortMonthDay(isoDate: string | null | undefined): string {
  if (!isoDate || isoDate.length < 10) return '';
  return isoDate.slice(5, 10);
}

/** Maps a client-side `ScheduleWindowIssue` to a localized message. */
export function issueMessage(issue: ScheduleWindowIssue): string {
  if (issue.reason === 'invalid_format') return m.course_schedule_error_date();
  if (issue.reason === 'before_start') return m.course_schedule_error_target_before_start();
  if (issue.reason === 'span_too_long') {
    return m.course_schedule_error_span_too_long({ max: MAX_SCHEDULE_SPAN_DAYS });
  }
  if (issue.reason === 'no_study_days') return m.course_schedule_error_no_study_days();
  return m.course_schedule_error_study_days();
}

export interface ScheduleFieldErrors {
  startDate?: string;
  fromDate?: string;
  targetDate?: string;
  studyDays?: string;
  general?: string;
}

/** Extracts inline field errors from a 400 `BackendError` using RFC 9457 pointers. */
export function fieldErrorsFromBackend(error: BackendError | undefined): ScheduleFieldErrors {
  if (!error) return {};
  const result: ScheduleFieldErrors = {};
  const start = error.refusalAt('/startDate');
  const from = error.refusalAt('/fromDate');
  const target = error.refusalAt('/targetDate');
  const days = error.refusalAt('/studyDays');
  if (start) result.startDate = m.course_schedule_error_date();
  if (from) result.fromDate = m.course_schedule_error_date();
  if (target) {
    result.targetDate =
      target.reason === 'required'
        ? m.course_schedule_error_target_required()
        : target.reason === 'invalid_format'
          ? m.course_schedule_error_date()
          : target.detail.includes('3660')
            ? m.course_schedule_error_span_too_long({ max: MAX_SCHEDULE_SPAN_DAYS })
            : m.course_schedule_error_target_before_start();
  }
  if (days) {
    result.studyDays =
      days.reason === 'required' || days.detail.includes('1 to 127')
        ? m.course_schedule_error_study_days()
        : m.course_schedule_error_no_study_days();
  }
  if (!start && !from && !target && !days) {
    result.general = m.app_problem_save();
  }
  return result;
}
