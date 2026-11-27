/**
 * Course study schedule, shared/personal manual redistribution, per-unit personal
 * planned-date override, and workspace "today's learning" API helpers.
 *
 * All 7 operations run under `/api/v1/w/{workspaceId}/...` and are never called
 * from public share links (`/shared/{courseId}`).
 */
import type {
  CourseScheduleView,
  LearningTodayItem,
  RedistributeScheduleRequest,
  ScheduledUnit,
  UnitProgress,
  UnitScheduleUpdate,
  WorkspaceLearningToday,
  CourseScheduleUpdate,
} from '../state/course-types.ts';

import type { Schemas } from './convert.ts';
import { unitProgressOf } from './unit-progress.ts';
import type { WikiRequest } from './wikis.ts';

function scheduledUnitOf(unit: Schemas['ScheduledUnit']): ScheduledUnit {
  return {
    unitId: unit.unitId,
    parentId: unit.parentId,
    title: unit.title,
    amount: unit.amount ?? null,
    amountUnit: unit.amountUnit ?? null,
    scheduledDate: unit.scheduledDate ?? null,
    myPlannedDate: unit.myPlannedDate ?? null,
    effectiveDate: unit.effectiveDate ?? null,
    completed: unit.completed,
    overdue: unit.overdue,
  };
}

export function courseScheduleViewOf(view: Schemas['CourseScheduleView']): CourseScheduleView {
  return {
    schedule: view.schedule
      ? {
          startDate: view.schedule.startDate,
          targetDate: view.schedule.targetDate,
          studyDays: view.schedule.studyDays,
          updatedAt: view.schedule.updatedAt,
        }
      : null,
    today: view.today,
    todayCount: view.todayCount,
    overdueCount: view.overdueCount,
    units: view.units.map(scheduledUnitOf),
  };
}

function learningTodayItemOf(item: Schemas['LearningTodayItem']): LearningTodayItem {
  return {
    courseId: item.courseId,
    courseTitle: item.courseTitle,
    unitId: item.unitId,
    parentId: item.parentId,
    title: item.title,
    amount: item.amount ?? null,
    amountUnit: item.amountUnit ?? null,
    scheduledDate: item.scheduledDate ?? null,
    myPlannedDate: item.myPlannedDate ?? null,
    effectiveDate: item.effectiveDate,
  };
}

function redistributeBodyOf(
  body: RedistributeScheduleRequest,
): Schemas['RedistributeScheduleRequest'] {
  return {
    ...(body.fromDate === undefined ? {} : { fromDate: body.fromDate }),
    ...(body.targetDate === undefined ? {} : { targetDate: body.targetDate }),
    ...(body.studyDays === undefined ? {} : { studyDays: body.studyDays }),
  };
}

export async function getCourseSchedule(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
): Promise<CourseScheduleView> {
  const view = await request<Schemas['CourseScheduleView']>(
    '/courses/{courseId}/schedule',
    { courseId },
    {},
    workspaceId,
  );
  return courseScheduleViewOf(view);
}

export async function putCourseSchedule(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  update: CourseScheduleUpdate,
): Promise<CourseScheduleView> {
  const json: Schemas['CourseScheduleUpdate'] = {
    startDate: update.startDate,
    targetDate: update.targetDate,
    studyDays: update.studyDays,
    ...(update.autoAssign === undefined ? {} : { autoAssign: update.autoAssign }),
  };
  const view = await request<Schemas['CourseScheduleView']>(
    '/courses/{courseId}/schedule',
    { courseId },
    { method: 'PUT', json },
    workspaceId,
  );
  return courseScheduleViewOf(view);
}

export async function deleteCourseSchedule(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
): Promise<void> {
  await request('/courses/{courseId}/schedule', { courseId }, { method: 'DELETE' }, workspaceId);
}

export async function redistributeCourseSchedule(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  body: RedistributeScheduleRequest,
): Promise<CourseScheduleView> {
  const view = await request<Schemas['CourseScheduleView']>(
    '/courses/{courseId}/schedule/redistribute',
    { courseId },
    { method: 'POST', json: redistributeBodyOf(body) },
    workspaceId,
  );
  return courseScheduleViewOf(view);
}

export async function redistributeMyCourseSchedule(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  body: RedistributeScheduleRequest,
): Promise<CourseScheduleView> {
  const view = await request<Schemas['CourseScheduleView']>(
    '/courses/{courseId}/my-schedule/redistribute',
    { courseId },
    { method: 'POST', json: redistributeBodyOf(body) },
    workspaceId,
  );
  return courseScheduleViewOf(view);
}

export async function putUnitSchedule(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  unitId: string,
  update: UnitScheduleUpdate,
): Promise<UnitProgress> {
  // `plannedDate` key is required on the wire (`null` clears the personal override).
  const json: Schemas['UnitScheduleUpdate'] = { plannedDate: update.plannedDate };
  const row = await request<Schemas['UnitProgress']>(
    '/courses/{courseId}/units/{unitId}/schedule',
    { courseId, unitId },
    { method: 'PUT', json },
    workspaceId,
  );
  return unitProgressOf(row);
}

export async function getWorkspaceLearningToday(
  request: WikiRequest,
  workspaceId: string,
): Promise<WorkspaceLearningToday> {
  const result = await request<Schemas['WorkspaceLearningToday']>(
    '/learning/today',
    {},
    {},
    workspaceId,
  );
  return {
    today: result.today,
    todayUnits: result.todayUnits.map(learningTodayItemOf),
    overdueUnits: result.overdueUnits.map(learningTodayItemOf),
  };
}
