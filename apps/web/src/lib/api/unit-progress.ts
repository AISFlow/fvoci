/**
 * Personal unit progress and course progress summary API helpers
 * (`getMyCourseProgress`, `putUnitProgress`, `getCourseProgressSummary`).
 *
 * All three operations run under `/api/v1/w/{workspaceId}/courses/{courseId}/...`
 * and are never called from public share links (`/shared/{courseId}`).
 */
import type {
  CourseProgressSummary,
  UnitProgress,
  UnitProgressUpdate,
} from '../state/course-types.ts';

import type { Schemas } from './convert.ts';
import type { WikiRequest } from './wikis.ts';

export function unitProgressOf(row: Schemas['UnitProgress']): UnitProgress {
  return {
    unitId: row.unitId,
    completedAt: row.completedAt,
    plannedDate: row.plannedDate,
    lastPosition: row.lastPosition,
    ...(typeof row.suggestedNextIntervalDays === 'number'
      ? { suggestedNextIntervalDays: row.suggestedNextIntervalDays }
      : {}),
    updatedAt: row.updatedAt,
  };
}

export async function getMyCourseProgress(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
): Promise<UnitProgress[]> {
  const rows = await request<Schemas['UnitProgress'][]>(
    '/courses/{courseId}/progress',
    { courseId },
    {},
    workspaceId,
  );
  return rows.map(unitProgressOf);
}

export async function putUnitProgress(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  unitId: string,
  update: UnitProgressUpdate,
): Promise<UnitProgress> {
  // Never send `completed: null`; omit unless explicitly boolean.
  const json: Schemas['UnitProgressUpdate'] = {
    ...(typeof update.completed === 'boolean' ? { completed: update.completed } : {}),
    ...(update.lastPosition === undefined ? {} : { lastPosition: update.lastPosition }),
  };
  const row = await request<Schemas['UnitProgress']>(
    '/courses/{courseId}/units/{unitId}/progress',
    { courseId, unitId },
    { method: 'PUT', json },
    workspaceId,
  );
  return unitProgressOf(row);
}

export async function getCourseProgressSummary(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
): Promise<CourseProgressSummary> {
  const summary = await request<Schemas['CourseProgressSummary']>(
    '/courses/{courseId}/progress/summary',
    { courseId },
    {},
    workspaceId,
  );
  return {
    totalUnits: summary.totalUnits,
    members: summary.members.map((member) => ({
      userId: member.userId,
      completedUnits: member.completedUnits,
    })),
  };
}
