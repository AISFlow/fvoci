/**
 * Course study meetings, shared attendance, exams, and strictly personal exam
 * score HTTP helpers (`contracts/openapi.json`).
 *
 * All 11 operations run under `/api/v1/w/{workspaceId}/courses/{courseId}/...`
 * and are never called from public share links (`/shared/{courseId}`).
 */
import type {
  CourseExam,
  CourseExamPatch,
  CourseExamsApi,
  CourseMeeting,
  CourseMeetingPatch,
  CourseMeetingsApi,
  MeetingAttendance,
  MeetingAttendanceUpdate,
  MyExamScore,
  MyExamScoreUpdate,
  NewCourseExam,
  NewCourseMeeting,
} from '../state/course-meeting-types.ts';

import type { Schemas } from './convert.ts';
import type { WikiRequest } from './wikis.ts';

function attendanceOf(row: Schemas['MeetingAttendance']): MeetingAttendance {
  return {
    userId: row.userId,
    status: row.status,
    updatedAt: row.updatedAt,
  };
}

export function courseMeetingOf(meeting: Schemas['CourseMeeting']): CourseMeeting {
  return {
    id: meeting.id,
    containerId: meeting.containerId,
    title: meeting.title,
    startsAt: meeting.startsAt,
    location: meeting.location,
    unitIds: [...meeting.unitIds],
    presenterId: meeting.presenterId ?? null,
    hostId: meeting.hostId ?? null,
    notes: meeting.notes,
    notifiedAt: meeting.notifiedAt ?? null,
    createdAt: meeting.createdAt,
    updatedAt: meeting.updatedAt,
    attendance: meeting.attendance.map(attendanceOf),
  };
}

function myExamScoreOf(score: Schemas['MyExamScore']): MyExamScore {
  return {
    score: score.score,
    note: score.note,
    updatedAt: score.updatedAt,
  };
}

export function courseExamOf(exam: Schemas['CourseExam']): CourseExam {
  return {
    id: exam.id,
    containerId: exam.containerId,
    title: exam.title,
    examDate: exam.examDate ?? null,
    maxScore: exam.maxScore,
    description: exam.description,
    createdAt: exam.createdAt,
    updatedAt: exam.updatedAt,
    myScore: exam.myScore ? myExamScoreOf(exam.myScore) : null,
    averageScore: exam.averageScore ?? null,
    scoreCount: exam.scoreCount,
    resultsPublishedAt: exam.resultsPublishedAt ?? null,
    publishedScoreBucket: exam.publishedScoreBucket ?? null,
  };
}

export async function listCourseMeetings(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
): Promise<CourseMeeting[]> {
  const rows = await request<Schemas['CourseMeeting'][]>(
    '/courses/{courseId}/meetings',
    { courseId },
    {},
    workspaceId,
  );
  return rows.map(courseMeetingOf);
}

export async function createCourseMeeting(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  draft: NewCourseMeeting,
): Promise<CourseMeeting> {
  const json: Schemas['NewCourseMeeting'] = {
    title: draft.title,
    startsAt: draft.startsAt,
    ...(draft.location === undefined ? {} : { location: draft.location }),
    ...(draft.unitIds === undefined ? {} : { unitIds: draft.unitIds }),
    ...(draft.presenterId === undefined ? {} : { presenterId: draft.presenterId }),
    ...(draft.hostId === undefined ? {} : { hostId: draft.hostId }),
    ...(draft.notes === undefined ? {} : { notes: draft.notes }),
  };
  const created = await request<Schemas['CourseMeeting']>(
    '/courses/{courseId}/meetings',
    { courseId },
    { method: 'POST', json },
    workspaceId,
  );
  return courseMeetingOf(created);
}

export async function patchCourseMeeting(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  meetingId: string,
  patch: CourseMeetingPatch,
): Promise<CourseMeeting> {
  const json: Schemas['CourseMeetingPatch'] = {
    ...(patch.title === undefined ? {} : { title: patch.title }),
    ...(patch.startsAt === undefined ? {} : { startsAt: patch.startsAt }),
    ...(patch.location === undefined ? {} : { location: patch.location }),
    ...(patch.unitIds === undefined ? {} : { unitIds: patch.unitIds }),
    ...(patch.presenterId === undefined ? {} : { presenterId: patch.presenterId }),
    ...(patch.hostId === undefined ? {} : { hostId: patch.hostId }),
    ...(patch.notes === undefined ? {} : { notes: patch.notes }),
  };
  const updated = await request<Schemas['CourseMeeting']>(
    '/courses/{courseId}/meetings/{meetingId}',
    { courseId, meetingId },
    { method: 'PATCH', json },
    workspaceId,
  );
  return courseMeetingOf(updated);
}

export async function deleteCourseMeeting(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  meetingId: string,
): Promise<void> {
  await request(
    '/courses/{courseId}/meetings/{meetingId}',
    { courseId, meetingId },
    { method: 'DELETE' },
    workspaceId,
  );
}

export async function putMeetingAttendance(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  meetingId: string,
  userId: string,
  update: MeetingAttendanceUpdate,
): Promise<MeetingAttendance> {
  const json: Schemas['MeetingAttendanceUpdate'] = { status: update.status };
  const saved = await request<Schemas['MeetingAttendance']>(
    '/courses/{courseId}/meetings/{meetingId}/attendance/{userId}',
    { courseId, meetingId, userId },
    { method: 'PUT', json },
    workspaceId,
  );
  return attendanceOf(saved);
}

export async function listCourseExams(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
): Promise<CourseExam[]> {
  const rows = await request<Schemas['CourseExam'][]>(
    '/courses/{courseId}/exams',
    { courseId },
    {},
    workspaceId,
  );
  return rows.map(courseExamOf);
}

export async function createCourseExam(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  draft: NewCourseExam,
): Promise<CourseExam> {
  const json: Schemas['NewCourseExam'] = {
    title: draft.title,
    maxScore: draft.maxScore,
    ...(draft.examDate === undefined ? {} : { examDate: draft.examDate }),
    ...(draft.description === undefined ? {} : { description: draft.description }),
  };
  const created = await request<Schemas['CourseExam']>(
    '/courses/{courseId}/exams',
    { courseId },
    { method: 'POST', json },
    workspaceId,
  );
  return courseExamOf(created);
}

export async function patchCourseExam(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  examId: string,
  patch: CourseExamPatch,
): Promise<CourseExam> {
  const json: Schemas['CourseExamPatch'] = {
    ...(patch.title === undefined ? {} : { title: patch.title }),
    ...(patch.examDate === undefined ? {} : { examDate: patch.examDate }),
    ...(patch.maxScore === undefined ? {} : { maxScore: patch.maxScore }),
    ...(patch.description === undefined ? {} : { description: patch.description }),
  };
  const updated = await request<Schemas['CourseExam']>(
    '/courses/{courseId}/exams/{examId}',
    { courseId, examId },
    { method: 'PATCH', json },
    workspaceId,
  );
  return courseExamOf(updated);
}

export async function deleteCourseExam(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  examId: string,
): Promise<void> {
  await request(
    '/courses/{courseId}/exams/{examId}',
    { courseId, examId },
    { method: 'DELETE' },
    workspaceId,
  );
}

export async function putMyExamScore(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  examId: string,
  update: MyExamScoreUpdate,
): Promise<MyExamScore> {
  const json: Schemas['MyExamScoreUpdate'] = {
    score: update.score,
    ...(update.note === undefined ? {} : { note: update.note }),
  };
  const saved = await request<Schemas['MyExamScore']>(
    '/courses/{courseId}/exams/{examId}/my-score',
    { courseId, examId },
    { method: 'PUT', json },
    workspaceId,
  );
  return myExamScoreOf(saved);
}

export async function deleteMyExamScore(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  examId: string,
): Promise<void> {
  await request(
    '/courses/{courseId}/exams/{examId}/my-score',
    { courseId, examId },
    { method: 'DELETE' },
    workspaceId,
  );
}

export async function setExamResultsPublished(
  request: WikiRequest,
  workspaceId: string | undefined,
  courseId: string,
  examId: string,
  published: boolean,
): Promise<CourseExam> {
  const exam = await request<Schemas['CourseExam']>(
    '/courses/{courseId}/exams/{examId}/results-publication',
    { courseId, examId },
    { method: published ? 'POST' : 'DELETE' },
    workspaceId,
  );
  return courseExamOf(exam);
}

export function serverCourseMeetings(request: WikiRequest): CourseMeetingsApi {
  return {
    list: (courseId, workspaceId) => listCourseMeetings(request, workspaceId, courseId),
    create: (courseId, draft, workspaceId) =>
      createCourseMeeting(request, workspaceId, courseId, draft),
    patch: (courseId, meetingId, patch, workspaceId) =>
      patchCourseMeeting(request, workspaceId, courseId, meetingId, patch),
    delete: (courseId, meetingId, workspaceId) =>
      deleteCourseMeeting(request, workspaceId, courseId, meetingId),
    putAttendance: (courseId, meetingId, userId, update, workspaceId) =>
      putMeetingAttendance(request, workspaceId, courseId, meetingId, userId, update),
  };
}

export function serverCourseExams(request: WikiRequest): CourseExamsApi {
  return {
    list: (courseId, workspaceId) => listCourseExams(request, workspaceId, courseId),
    create: (courseId, draft, workspaceId) =>
      createCourseExam(request, workspaceId, courseId, draft),
    patch: (courseId, examId, patch, workspaceId) =>
      patchCourseExam(request, workspaceId, courseId, examId, patch),
    delete: (courseId, examId, workspaceId) =>
      deleteCourseExam(request, workspaceId, courseId, examId),
    putMyScore: (courseId, examId, update, workspaceId) =>
      putMyExamScore(request, workspaceId, courseId, examId, update),
    deleteMyScore: (courseId, examId, workspaceId) =>
      deleteMyExamScore(request, workspaceId, courseId, examId),
    setResultsPublished: (courseId, examId, published, workspaceId) =>
      setExamResultsPublished(request, workspaceId, courseId, examId, published),
  };
}
