/**
 * Pure logic for course study schedules, weekday bitmasks, proportional unit
 * distribution, outline traversal, and time-zone "today" resolution.
 *
 * Mirrors `crates/model/src/course_schedule.rs` so the in-memory backend and
 * client-side validation agree with the server without relying on device time.
 */
import {
  addDays,
  dateOfInstant,
  dateOfUtc,
  daysBetween,
  parseCalendarDate,
  type CalendarDate,
} from './dates.ts';
import { isTimeZone } from './regional.ts';

/** Unit of measure for a course unit's study amount. */
export type AmountUnit = 'minutes' | 'pages' | 'problems';

export const AMOUNT_UNITS: readonly AmountUnit[] = ['minutes', 'pages', 'problems'];

export const UNIT_AMOUNT_MIN = 1;
export const UNIT_AMOUNT_MAX = 100_000;
export const MAX_SCHEDULE_SPAN_DAYS = 3_660;

export const STUDY_DAYS_MONDAY = 1 << 0;
export const STUDY_DAYS_TUESDAY = 1 << 1;
export const STUDY_DAYS_WEDNESDAY = 1 << 2;
export const STUDY_DAYS_THURSDAY = 1 << 3;
export const STUDY_DAYS_FRIDAY = 1 << 4;
export const STUDY_DAYS_SATURDAY = 1 << 5;
export const STUDY_DAYS_SUNDAY = 1 << 6;
export const STUDY_DAYS_WEEKDAYS = 0b001_1111;
export const STUDY_DAYS_ALL = 0b111_1111;

export type StudyWeekdayKey = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';

export const STUDY_WEEKDAYS: readonly { bit: number; key: StudyWeekdayKey }[] = [
  { bit: STUDY_DAYS_MONDAY, key: 'mon' },
  { bit: STUDY_DAYS_TUESDAY, key: 'tue' },
  { bit: STUDY_DAYS_WEDNESDAY, key: 'wed' },
  { bit: STUDY_DAYS_THURSDAY, key: 'thu' },
  { bit: STUDY_DAYS_FRIDAY, key: 'fri' },
  { bit: STUDY_DAYS_SATURDAY, key: 'sat' },
  { bit: STUDY_DAYS_SUNDAY, key: 'sun' },
];

/** Returns whether `value` is a supported `AmountUnit`. */
export function isAmountUnit(value: unknown): value is AmountUnit {
  return value === 'minutes' || value === 'pages' || value === 'problems';
}

/** Returns whether `bits` is a valid study-days bitmask (`1..=127`). */
export function isValidStudyDays(bits: number): boolean {
  return Number.isInteger(bits) && bits >= 1 && bits <= STUDY_DAYS_ALL;
}

/** Returns the Monday=1..Sunday=64 bit for a calendar date (`YYYY-MM-DD`). */
export function studyDayBitOfDate(date: CalendarDate): number {
  const utc = new Date(0);
  utc.setUTCFullYear(
    Number(date.slice(0, 4)),
    Number(date.slice(5, 7)) - 1,
    Number(date.slice(8, 10)),
  );
  // getUTCDay(): 0 = Sun, 1 = Mon .. 6 = Sat -> map to 0 = Mon .. 6 = Sun.
  const isoZero = (utc.getUTCDay() + 6) % 7;
  return 1 << isoZero;
}

/** Returns whether `date` falls on a weekday enabled in `studyDays`. */
export function studyDaysIncludes(studyDays: number, date: CalendarDate): boolean {
  return (studyDays & studyDayBitOfDate(date)) !== 0;
}

/**
 * Returns today's calendar date for `nowMillis` in `timeZone`.
 * Matches server behavior: `"auto"` or an unknown zone resolves in UTC.
 */
export function todayInTimeZone(nowMillis: number, timeZone: string): CalendarDate {
  if (timeZone === 'auto' || !isTimeZone(timeZone)) return dateOfUtc(nowMillis);
  return dateOfInstant(nowMillis, timeZone);
}

/**
 * Returns whether at least one day in `start..=target` matches `studyDays`.
 * Checks at most 7 consecutive days from `start`.
 */
export function hasStudyDayInRange(
  start: CalendarDate,
  target: CalendarDate,
  studyDays: number,
): boolean {
  if (target < start || !isValidStudyDays(studyDays)) return false;
  for (let offset = 0; offset < 7; offset += 1) {
    const candidate = addDays(start, offset);
    if (candidate > target) break;
    if (studyDaysIncludes(studyDays, candidate)) return true;
  }
  return false;
}

/** Returns every calendar date in `start..=target` matching `studyDays`, ascending. */
export function studyDatesInRange(
  start: CalendarDate,
  target: CalendarDate,
  studyDays: number,
): CalendarDate[] {
  if (target < start || !isValidStudyDays(studyDays)) return [];
  const span = daysBetween(start, target);
  if (span > MAX_SCHEDULE_SPAN_DAYS) return [];
  const dates: CalendarDate[] = [];
  for (let offset = 0; offset <= span; offset += 1) {
    const current = addDays(start, offset);
    if (studyDaysIncludes(studyDays, current)) dates.push(current);
  }
  return dates;
}

/** Outline node shape needed to walk live units in course tree order. */
export interface OutlineNodeLike {
  readonly id: string;
  readonly kind: 'unit' | 'chapter';
  readonly parentId: string;
  readonly order: string;
}

function compareOrderId(a: OutlineNodeLike, b: OutlineNodeLike): number {
  if (a.order !== b.order) return a.order < b.order ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/**
 * Returns live `unit` nodes in true outline tree order (chapters expanded to
 * their child units, top-level units at their sibling position, chapters omitted).
 * Matches `crates/model/src/course_schedule.rs::outline_units`.
 */
export function outlineUnits<T extends OutlineNodeLike>(nodes: readonly T[]): T[] {
  const topLevel = nodes.filter((node) => node.parentId === '').sort(compareOrderId);
  const childrenByChapter = new Map<string, T[]>();
  for (const node of nodes) {
    if (node.parentId !== '' && node.kind === 'unit') {
      const list = childrenByChapter.get(node.parentId) ?? [];
      list.push(node);
      childrenByChapter.set(node.parentId, list);
    }
  }
  for (const children of childrenByChapter.values()) children.sort(compareOrderId);

  const ordered: T[] = [];
  const seen = new Set<string>();
  for (const node of topLevel) {
    if (node.kind === 'unit') {
      if (!seen.has(node.id)) {
        seen.add(node.id);
        ordered.push(node);
      }
    } else {
      for (const child of childrenByChapter.get(node.id) ?? []) {
        if (!seen.has(child.id)) {
          seen.add(child.id);
          ordered.push(child);
        }
      }
    }
  }

  const orphans = nodes
    .filter((node) => node.kind === 'unit' && !seen.has(node.id))
    .sort(compareOrderId);
  ordered.push(...orphans);
  return ordered;
}

export interface UnitWeight {
  readonly unitId: string;
  readonly amount: number | null;
}

/**
 * Distributes `units` (in outline order) across study days in `start..=target`
 * proportionally to each unit's `amount` (null/absent = weight 1) using exact
 * BigInt integer arithmetic matching `crates/model/src/course_schedule.rs::distribute_units`.
 */
export function distributeUnits(
  units: readonly UnitWeight[],
  start: CalendarDate,
  target: CalendarDate,
  studyDays: number,
): [string, CalendarDate][] {
  if (units.length === 0) return [];
  const days = studyDatesInRange(start, target, studyDays);
  if (days.length === 0) return [];
  const dayCount = BigInt(days.length);
  let totalWeight = 0n;
  for (const unit of units) totalWeight += BigInt(Math.max(1, unit.amount ?? 1));
  const denominator = 2n * totalWeight;
  let cumulative = 0n;
  const assigned: [string, CalendarDate][] = [];
  for (const unit of units) {
    const weight = BigInt(Math.max(1, unit.amount ?? 1));
    const weightedDays = weight * dayCount;
    const halfStep = weightedDays < totalWeight ? weightedDays : totalWeight;
    const numerator = 2n * cumulative * dayCount + halfStep;
    const rawIndex = numerator / denominator;
    const index = Number(rawIndex >= dayCount ? dayCount - 1n : rawIndex);
    const day = days[index] ?? days[days.length - 1];
    if (day) assigned.push([unit.unitId, day]);
    cumulative += weight;
  }
  return assigned;
}

export type ScheduleWindowIssue =
  | { field: 'startDate' | 'fromDate'; reason: 'invalid_format' }
  | { field: 'targetDate'; reason: 'invalid_format' | 'before_start' | 'span_too_long' }
  | { field: 'studyDays'; reason: 'invalid' | 'no_study_days' };

/**
 * Validates a schedule or redistribution window `(startText, targetText, studyDays)`.
 * Returns every detected issue with its field name for inline error display.
 */
export function validateScheduleWindow(
  startText: string,
  targetText: string,
  studyDays: number,
  startField: 'startDate' | 'fromDate' = 'startDate',
): ScheduleWindowIssue[] {
  const issues: ScheduleWindowIssue[] = [];
  const start = parseCalendarDate(startText);
  const target = parseCalendarDate(targetText);
  const validDays = isValidStudyDays(studyDays);
  if (!start) issues.push({ field: startField, reason: 'invalid_format' });
  if (!target) issues.push({ field: 'targetDate', reason: 'invalid_format' });
  if (!validDays) issues.push({ field: 'studyDays', reason: 'invalid' });
  if (start && target) {
    if (target < start) {
      issues.push({ field: 'targetDate', reason: 'before_start' });
    } else if (daysBetween(start, target) > MAX_SCHEDULE_SPAN_DAYS) {
      issues.push({ field: 'targetDate', reason: 'span_too_long' });
    } else if (validDays && !hasStudyDayInRange(start, target, studyDays)) {
      issues.push({ field: 'studyDays', reason: 'no_study_days' });
    }
  }
  return issues;
}

/** Validates an optional unit study amount (`1..=100_000` integer). */
export function isValidUnitAmount(amount: number): boolean {
  return Number.isInteger(amount) && amount >= UNIT_AMOUNT_MIN && amount <= UNIT_AMOUNT_MAX;
}
