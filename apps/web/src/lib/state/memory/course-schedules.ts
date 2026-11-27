/**
 * Course schedules, proportional auto-assignment, manual shared/personal
 * redistribution, personal planned-date overrides, and workspace today/overdue
 * feeds in the in-memory backend.
 *
 * Matches `crates/model/src/course_schedule.rs`, `crates/store/src/course_schedules.rs`,
 * and `crates/server/src/app/course_schedules.rs` 1:1:
 * - Overdue incomplete units are NEVER auto-shifted forward.
 * - Personal `plannedDate` is strictly isolated per `(unitId, userId)` with zero WS events.
 * - Shared schedule updates & shared redistribution emit `units` change events for modified units.
 * - Unjoined open-course readers can read `schedule` (`myPlannedDate: null, completed: false`)
 *   but receive 403 on personal schedule writes and have unjoined courses excluded from `learningToday`.
 */
import {
  distributeUnits,
  hasStudyDayInRange,
  isValidStudyDays,
  outlineUnits,
  todayInTimeZone,
} from '../../logic/course-schedule.ts';
import { daysBetween, parseCalendarDate, type CalendarDate } from '../../logic/dates.ts';
import {
  MAX_SCHEDULE_SPAN_DAYS,
  type CourseSchedule,
  type CourseScheduleUpdate,
  type CourseScheduleView,
  type LearningTodayItem,
  type RedistributeScheduleRequest,
  type ScheduledUnit,
  type UnitProgress,
  type UnitScheduleUpdate,
  type WorkspaceLearningToday,
} from '../course-types.ts';

import { settingsOf } from './account.ts';
import { roleOn } from './comments.ts';
import { memberRole } from './container-access.ts';
import { fail } from './refuse.ts';
import { copy, unitOf } from './shapes.ts';
import { checkWorkspace } from './spaces.ts';
import type { StoredBlock, World } from './world.ts';

const isCourseBlock = (block: StoredBlock | undefined): boolean =>
  block?.type === 'unit' || block?.type === 'chapter';

function hasMembership(world: World, userId: string, courseId: string): boolean {
  return roleOn(world.members.get(courseId) ?? [], userId) !== '';
}

function mayManageCourse(world: World, userId: string, courseId: string): void {
  const role = memberRole(world, userId, courseId, 'course');
  if (role !== 'admin' && role !== 'editor') fail(403, 'access denied to the course');
}

function mustBeEnrolled(world: World, userId: string, courseId: string): void {
  memberRole(world, userId, courseId, 'course');
  if (!hasMembership(world, userId, courseId)) {
    fail(403, 'join the course before managing your personal schedule');
  }
}

function callerToday(world: World, userId: string): CalendarDate {
  return todayInTimeZone(world.now(), settingsOf(world, userId).timeZone);
}

function checkDateField(field: string, raw: string): CalendarDate {
  const parsed = parseCalendarDate(raw);
  if (!parsed) {
    fail(400, 'a date must be a valid calendar day written as YYYY-MM-DD', {
      field,
      reason: 'invalid_format',
    });
  }
  return parsed;
}

function checkStudyDaysField(bits: number): number {
  if (!isValidStudyDays(bits)) {
    fail(400, 'studyDays must be a weekday bitmask from 1 to 127', {
      field: 'studyDays',
      reason: 'invalid',
    });
  }
  return bits;
}

function checkWindow(start: CalendarDate, target: CalendarDate, studyDays: number): void {
  if (target < start) {
    fail(400, 'targetDate must be on or after the start date', {
      field: 'targetDate',
      reason: 'invalid',
    });
  }
  if (daysBetween(start, target) > MAX_SCHEDULE_SPAN_DAYS) {
    fail(400, `schedule span must be at most ${MAX_SCHEDULE_SPAN_DAYS} days`, {
      field: 'targetDate',
      reason: 'invalid',
    });
  }
  if (!hasStudyDayInRange(start, target, studyDays)) {
    fail(400, 'no selected study day falls between the start date and targetDate', {
      field: 'studyDays',
      reason: 'invalid',
    });
  }
}

function resolveRedistributePlan(
  request: RedistributeScheduleRequest,
  today: CalendarDate,
  stored: CourseSchedule | undefined,
): { fromDate: CalendarDate; targetDate: CalendarDate; studyDays: number } {
  const fromDate =
    request.fromDate !== undefined && request.fromDate !== null
      ? checkDateField('fromDate', request.fromDate)
      : today;
  const targetDate =
    request.targetDate !== undefined && request.targetDate !== null
      ? checkDateField('targetDate', request.targetDate)
      : stored
        ? checkDateField('targetDate', stored.targetDate)
        : fail(400, 'targetDate is required when the course has no saved schedule', {
            field: 'targetDate',
            reason: 'required',
          });
  const studyDays =
    request.studyDays !== undefined && request.studyDays !== null
      ? checkStudyDaysField(request.studyDays)
      : stored
        ? checkStudyDaysField(stored.studyDays)
        : fail(400, 'studyDays is required when the course has no saved schedule', {
            field: 'studyDays',
            reason: 'required',
          });
  checkWindow(fromDate, targetDate, studyDays);
  return { fromDate, targetDate, studyDays };
}

function liveCourseNodes(world: World, courseId: string): ReturnType<typeof unitOf>[] {
  return world.blocksOf(courseId).filter(isCourseBlock).map(unitOf);
}

function buildScheduleView(
  world: World,
  userId: string,
  courseId: string,
  today: CalendarDate,
): CourseScheduleView {
  const enrolled = hasMembership(world, userId, courseId);
  const progressByUnit = enrolled ? world.unitProgress.get(userId) : undefined;
  const ordered = outlineUnits(liveCourseNodes(world, courseId));
  let todayCount = 0;
  let overdueCount = 0;
  const units: ScheduledUnit[] = [];

  for (const node of ordered) {
    const row = progressByUnit?.get(node.id);
    const myPlannedDate = row?.plannedDate ?? null;
    const completed = (row?.completedAt ?? null) !== null;
    const scheduledDate = node.scheduledDate ?? null;
    const effectiveDate = myPlannedDate ?? scheduledDate;
    const overdue = !completed && effectiveDate !== null && effectiveDate < today;
    if (!completed && effectiveDate === today) todayCount += 1;
    if (overdue) overdueCount += 1;
    units.push({
      unitId: node.id,
      parentId: node.parentId,
      title: node.title,
      amount: node.amount ?? null,
      amountUnit: node.amountUnit ?? null,
      scheduledDate,
      myPlannedDate,
      effectiveDate,
      completed,
      overdue,
    });
  }

  const saved = world.courseSchedules.get(courseId);
  return {
    schedule: saved ? copy(saved) : null,
    today,
    todayCount,
    overdueCount,
    units,
  };
}

function applySharedDates(
  world: World,
  userId: string,
  courseId: string,
  start: CalendarDate,
  target: CalendarDate,
  studyDays: number,
): void {
  const ordered = outlineUnits(liveCourseNodes(world, courseId));
  const assignments = distributeUnits(
    ordered.map((node) => ({ unitId: node.id, amount: node.amount ?? null })),
    start,
    target,
    studyDays,
  );
  const changed: StoredBlock[] = [];
  for (const [unitId, date] of assignments) {
    const block = world.blocks.get(unitId);
    if (block?.type !== 'unit' || block.containerId !== courseId) continue;
    const prev = (block.fields as { scheduledDate?: unknown }).scheduledDate;
    if (prev === date) continue;
    block.fields = { ...block.fields, scheduledDate: date };
    block.updatedAt = world.now();
    block.modifiedBy = userId;
    changed.push(block);
  }
  if (changed.length > 0) {
    world.tellContainer(courseId, {
      kind: 'units',
      containerId: courseId,
      units: changed.map(unitOf),
    });
  }
}

export function memoryGetCourseSchedule(
  world: World,
  userId: string,
  courseId: string,
): CourseScheduleView {
  memberRole(world, userId, courseId, 'course');
  return buildScheduleView(world, userId, courseId, callerToday(world, userId));
}

export function memoryPutCourseSchedule(
  world: World,
  userId: string,
  courseId: string,
  update: CourseScheduleUpdate,
): CourseScheduleView {
  mayManageCourse(world, userId, courseId);
  const start = checkDateField('startDate', update.startDate);
  const target = checkDateField('targetDate', update.targetDate);
  const studyDays = checkStudyDaysField(update.studyDays);
  checkWindow(start, target, studyDays);
  const saved: CourseSchedule = {
    startDate: start,
    targetDate: target,
    studyDays,
    updatedAt: world.now(),
  };
  world.courseSchedules.set(courseId, saved);
  if (update.autoAssign) applySharedDates(world, userId, courseId, start, target, studyDays);
  return buildScheduleView(world, userId, courseId, callerToday(world, userId));
}

export function memoryDeleteCourseSchedule(world: World, userId: string, courseId: string): void {
  mayManageCourse(world, userId, courseId);
  world.courseSchedules.delete(courseId);
}

export function memoryRedistributeCourseSchedule(
  world: World,
  userId: string,
  courseId: string,
  request: RedistributeScheduleRequest,
): CourseScheduleView {
  mayManageCourse(world, userId, courseId);
  const today = callerToday(world, userId);
  const plan = resolveRedistributePlan(request, today, world.courseSchedules.get(courseId));
  applySharedDates(world, userId, courseId, plan.fromDate, plan.targetDate, plan.studyDays);
  return buildScheduleView(world, userId, courseId, today);
}

export function memoryRedistributeMyCourseSchedule(
  world: World,
  userId: string,
  courseId: string,
  request: RedistributeScheduleRequest,
): CourseScheduleView {
  mustBeEnrolled(world, userId, courseId);
  const today = callerToday(world, userId);
  const plan = resolveRedistributePlan(request, today, world.courseSchedules.get(courseId));
  let byUnit = world.unitProgress.get(userId);
  const ordered = outlineUnits(liveCourseNodes(world, courseId));
  const incomplete = ordered.filter((node) => (byUnit?.get(node.id)?.completedAt ?? null) === null);
  const assignments = distributeUnits(
    incomplete.map((node) => ({ unitId: node.id, amount: node.amount ?? null })),
    plan.fromDate,
    plan.targetDate,
    plan.studyDays,
  );
  if (assignments.length > 0) {
    if (!byUnit) {
      byUnit = new Map();
      world.unitProgress.set(userId, byUnit);
    }
    for (const [unitId, date] of assignments) {
      const existing = byUnit.get(unitId);
      byUnit.set(unitId, {
        unitId,
        completedAt: existing?.completedAt ?? null,
        plannedDate: date,
        lastPosition: existing?.lastPosition ?? null,
        updatedAt: world.now(),
      });
    }
  }
  return buildScheduleView(world, userId, courseId, today);
}

export function memoryPutUnitSchedule(
  world: World,
  userId: string,
  courseId: string,
  unitId: string,
  update: UnitScheduleUpdate,
): UnitProgress {
  mustBeEnrolled(world, userId, courseId);
  if (!('plannedDate' in update)) {
    fail(400, 'plannedDate is required', { field: 'plannedDate', reason: 'required' });
  }
  const plannedDate =
    update.plannedDate === null ? null : checkDateField('plannedDate', update.plannedDate);
  const block = world.blocks.get(unitId);
  if (block?.containerId !== courseId || block.type !== 'unit') {
    fail(404, 'no such unit');
  }
  let byUnit = world.unitProgress.get(userId);
  if (!byUnit) {
    byUnit = new Map();
    world.unitProgress.set(userId, byUnit);
  }
  const existing = byUnit.get(unitId);
  const next: UnitProgress = {
    unitId,
    completedAt: existing?.completedAt ?? null,
    plannedDate,
    lastPosition: existing?.lastPosition ?? null,
    updatedAt: world.now(),
  };
  byUnit.set(unitId, next);
  return copy(next);
}

export function memoryGetWorkspaceLearningToday(
  world: World,
  userId: string,
  workspaceId: string,
): WorkspaceLearningToday {
  checkWorkspace(world, userId, workspaceId);
  const today = callerToday(world, userId);
  const enrolledCourses = [...world.containers.values()]
    .filter(
      (container) =>
        container.kind === 'course' &&
        container.workspaceId === workspaceId &&
        !container.isTemplate &&
        hasMembership(world, userId, container.id),
    )
    .sort((a, b) => a.createdAt - b.createdAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));

  const progressByUnit = world.unitProgress.get(userId);
  const todayUnits: LearningTodayItem[] = [];
  const overdueUnits: LearningTodayItem[] = [];

  for (const course of enrolledCourses) {
    for (const node of outlineUnits(liveCourseNodes(world, course.id))) {
      const row = progressByUnit?.get(node.id);
      if ((row?.completedAt ?? null) !== null) continue;
      const myPlannedDate = row?.plannedDate ?? null;
      const scheduledDate = node.scheduledDate ?? null;
      const effectiveDate = myPlannedDate ?? scheduledDate;
      if (effectiveDate === null) continue;
      const item: LearningTodayItem = {
        courseId: course.id,
        courseTitle: course.title,
        unitId: node.id,
        parentId: node.parentId,
        title: node.title,
        amount: node.amount ?? null,
        amountUnit: node.amountUnit ?? null,
        scheduledDate,
        myPlannedDate,
        effectiveDate,
      };
      if (effectiveDate === today) todayUnits.push(item);
      else if (effectiveDate < today) overdueUnits.push(item);
    }
  }

  overdueUnits.sort((a, b) =>
    a.effectiveDate < b.effectiveDate ? -1 : a.effectiveDate > b.effectiveDate ? 1 : 0,
  );
  return { today, todayUnits, overdueUnits };
}
