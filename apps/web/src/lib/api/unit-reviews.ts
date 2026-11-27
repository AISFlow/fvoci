/**
 * Personal spaced-repetition unit reviews, workspace "Today's reviews", and
 * personal unit study notes API client.
 *
 * All nine operations run under `/api/v1/w/{workspaceId}/...` and are never
 * called from public share links (`/shared/{courseId}`): `SHARED_UNIT_REVIEWS`
 * returns empty arrays on reads and rejects writes locally with 403.
 */
import { BackendError } from '../state/backend.ts';
import type {
  CompletedUnitReview,
  CompleteUnitReview,
  CourseReviews,
  CreateUnitNote,
  ListCourseReviewsQuery,
  PatchUnitNote,
  ScheduleUnitReview,
  TodayReviewItem,
  UnitNote,
  UnitNoteKind,
  UnitReview,
} from '../state/unit-review-types.ts';

import type { Schemas } from './convert.ts';
import type { WikiRequest } from './wikis.ts';

export function unitReviewOf(row: Schemas['UnitReview']): UnitReview {
  return {
    id: row.id,
    unitId: row.unitId,
    containerId: row.containerId,
    scheduledDate: row.scheduledDate,
    completedAt: row.completedAt ?? null,
    intervalDays: row.intervalDays,
    createdAt: row.createdAt,
    updatedAt: row.updatedAt,
  };
}

export function completedUnitReviewOf(row: Schemas['CompletedUnitReview']): CompletedUnitReview {
  return {
    review: unitReviewOf(row.review),
    suggestedNextIntervalDays: row.suggestedNextIntervalDays,
    nextReview: row.nextReview ? unitReviewOf(row.nextReview) : null,
  };
}

export function todayReviewItemOf(row: Schemas['TodayReviewItem']): TodayReviewItem {
  return {
    id: row.id,
    unitId: row.unitId,
    containerId: row.containerId,
    courseTitle: row.courseTitle,
    unitTitle: row.unitTitle,
    scheduledDate: row.scheduledDate,
    intervalDays: row.intervalDays,
    overdueDays: row.overdueDays,
    createdAt: row.createdAt,
    updatedAt: row.updatedAt,
  };
}

export function unitNoteOf(row: Schemas['UnitNote']): UnitNote {
  return {
    id: row.id,
    unitId: row.unitId,
    containerId: row.containerId,
    kind: row.kind,
    content: row.content,
    createdAt: row.createdAt,
    updatedAt: row.updatedAt,
  };
}

export async function listMyCourseReviews(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  query: ListCourseReviewsQuery = {},
): Promise<UnitReview[]> {
  const rows = await request<Schemas['UnitReview'][]>(
    '/courses/{courseId}/reviews',
    { courseId },
    {
      query: {
        ...(query.status ? { status: query.status } : {}),
        ...(query.unitId ? { unitId: query.unitId } : {}),
      },
    },
    workspaceId,
  );
  return rows.map(unitReviewOf);
}

export async function scheduleUnitReview(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  unitId: string,
  body: ScheduleUnitReview,
): Promise<UnitReview> {
  // Never send explicit `null` for optional fields; omit undefined members.
  const json: Schemas['ScheduleUnitReview'] = {
    ...(typeof body.intervalDays === 'number' ? { intervalDays: body.intervalDays } : {}),
    ...(typeof body.scheduledDate === 'string' ? { scheduledDate: body.scheduledDate } : {}),
  };
  const row = await request<Schemas['UnitReview']>(
    '/courses/{courseId}/units/{unitId}/reviews',
    { courseId, unitId },
    { method: 'POST', json },
    workspaceId,
  );
  return unitReviewOf(row);
}

export async function completeUnitReview(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  reviewId: string,
  body: CompleteUnitReview = {},
): Promise<CompletedUnitReview> {
  const json: Schemas['CompleteUnitReview'] = {
    ...(typeof body.scheduleNextIntervalDays === 'number'
      ? { scheduleNextIntervalDays: body.scheduleNextIntervalDays }
      : {}),
  };
  const row = await request<Schemas['CompletedUnitReview']>(
    '/courses/{courseId}/reviews/{reviewId}/complete',
    { courseId, reviewId },
    { method: 'POST', json },
    workspaceId,
  );
  return completedUnitReviewOf(row);
}

export async function deleteUnitReview(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  reviewId: string,
): Promise<void> {
  await request(
    '/courses/{courseId}/reviews/{reviewId}',
    { courseId, reviewId },
    { method: 'DELETE' },
    workspaceId,
  );
}

export async function getWorkspaceTodayReviews(
  request: WikiRequest,
  workspaceId: string,
): Promise<TodayReviewItem[]> {
  const rows = await request<Schemas['TodayReviewItem'][]>(
    '/learning/reviews/today',
    {},
    {},
    workspaceId,
  );
  return rows.map(todayReviewItemOf);
}

export async function listMyUnitNotes(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  unitId: string,
  kind?: UnitNoteKind,
): Promise<UnitNote[]> {
  const rows = await request<Schemas['UnitNote'][]>(
    '/courses/{courseId}/units/{unitId}/notes',
    { courseId, unitId },
    { query: kind ? { kind } : {} },
    workspaceId,
  );
  return rows.map(unitNoteOf);
}

export async function createUnitNote(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  unitId: string,
  body: CreateUnitNote,
): Promise<UnitNote> {
  const json: Schemas['CreateUnitNote'] = {
    kind: body.kind,
    content: body.content,
  };
  const row = await request<Schemas['UnitNote']>(
    '/courses/{courseId}/units/{unitId}/notes',
    { courseId, unitId },
    { method: 'POST', json },
    workspaceId,
  );
  return unitNoteOf(row);
}

export async function patchUnitNote(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  noteId: string,
  patch: PatchUnitNote,
): Promise<UnitNote> {
  const json: Schemas['PatchUnitNote'] = {
    ...(patch.kind === undefined ? {} : { kind: patch.kind }),
    ...(patch.content === undefined ? {} : { content: patch.content }),
  };
  const row = await request<Schemas['UnitNote']>(
    '/courses/{courseId}/notes/{noteId}',
    { courseId, noteId },
    { method: 'PATCH', json },
    workspaceId,
  );
  return unitNoteOf(row);
}

export async function deleteUnitNote(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  noteId: string,
): Promise<void> {
  await request(
    '/courses/{courseId}/notes/{noteId}',
    { courseId, noteId },
    { method: 'DELETE' },
    workspaceId,
  );
}

export class ServerUnitReviews implements CourseReviews {
  readonly #request: WikiRequest;

  constructor(request: WikiRequest) {
    this.#request = request;
  }

  listMyCourseReviews(
    courseId: string,
    query?: ListCourseReviewsQuery,
    workspaceId?: string,
  ): Promise<UnitReview[]> {
    return listMyCourseReviews(this.#request, workspaceId, courseId, query);
  }

  scheduleUnitReview(
    courseId: string,
    unitId: string,
    body: ScheduleUnitReview,
    workspaceId?: string,
  ): Promise<UnitReview> {
    return scheduleUnitReview(this.#request, workspaceId, courseId, unitId, body);
  }

  completeUnitReview(
    courseId: string,
    reviewId: string,
    body?: CompleteUnitReview,
    workspaceId?: string,
  ): Promise<CompletedUnitReview> {
    return completeUnitReview(this.#request, workspaceId, courseId, reviewId, body);
  }

  deleteUnitReview(courseId: string, reviewId: string, workspaceId?: string): Promise<void> {
    return deleteUnitReview(this.#request, workspaceId, courseId, reviewId);
  }

  getWorkspaceTodayReviews(workspaceId: string): Promise<TodayReviewItem[]> {
    return getWorkspaceTodayReviews(this.#request, workspaceId);
  }

  listMyUnitNotes(
    courseId: string,
    unitId: string,
    kind?: UnitNoteKind,
    workspaceId?: string,
  ): Promise<UnitNote[]> {
    return listMyUnitNotes(this.#request, workspaceId, courseId, unitId, kind);
  }

  createUnitNote(
    courseId: string,
    unitId: string,
    body: CreateUnitNote,
    workspaceId?: string,
  ): Promise<UnitNote> {
    return createUnitNote(this.#request, workspaceId, courseId, unitId, body);
  }

  patchUnitNote(
    courseId: string,
    noteId: string,
    patch: PatchUnitNote,
    workspaceId?: string,
  ): Promise<UnitNote> {
    return patchUnitNote(this.#request, workspaceId, courseId, noteId, patch);
  }

  deleteUnitNote(courseId: string, noteId: string, workspaceId?: string): Promise<void> {
    return deleteUnitNote(this.#request, workspaceId, courseId, noteId);
  }
}

const nothing = (): Promise<never[]> => Promise.resolve([]);
const refused = (): Promise<never> =>
  Promise.reject(
    new BackendError(403, { detail: 'a reader by the link reads only', reason: 'read_only' }),
  );

/** Zero-request stub for `SharedBackend`: public share links never touch personal reviews or notes. */
export const SHARED_UNIT_REVIEWS: CourseReviews = {
  listMyCourseReviews: nothing,
  scheduleUnitReview: refused,
  completeUnitReview: refused,
  deleteUnitReview: refused,
  getWorkspaceTodayReviews: nothing,
  listMyUnitNotes: nothing,
  createUnitNote: refused,
  patchUnitNote: refused,
  deleteUnitNote: refused,
};
