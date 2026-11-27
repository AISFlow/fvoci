/**
 * In-memory implementation of course exams and strictly personal exam scores
 * matching `crates/model/src/course_exam.rs`, `crates/store/src/course_exams.rs`,
 * and `crates/server/src/app/course_events.rs`.
 *
 * Invariants:
 * - Exams sorted by `examDate ASC NULLS LAST, createdAt ASC, id ASC`.
 * - Personal exam scores are strictly isolated per caller, excluded for unjoined
 *   readers and erased/removed users, and emit zero WebSocket broadcasts.
 */
import { parseCalendarDate } from '../../logic/dates.ts';
import {
  compareCourseExams,
  COURSE_EXAM_DESCRIPTION_MAX_CHARS,
  COURSE_EXAM_MAX_SCORE_LIMIT,
  COURSE_EXAM_TITLE_MAX_CHARS,
  EXAM_SCORE_NOTE_MAX_CHARS,
  type CourseExam,
  type CourseExamPatch,
  type CourseExamsApi,
  type MyExamScore,
  type MyExamScoreUpdate,
  type NewCourseExam,
} from '../course-meeting-types.ts';

import { memberRole } from './container-access.ts';
import {
  activeMemberIds,
  checkMultiline,
  checkTitle,
  isEnrolledActive,
  mayManageCourseEvents,
} from './course-meetings.ts';
import { fail } from './refuse.ts';
import { copy } from './shapes.ts';
import type { WikiHost } from './wikis.ts';
import type { StoredCourseExam, World } from './world.ts';

function checkExamDate(raw: string | null | undefined): string | null {
  if (raw === undefined || raw === null) return null;
  const parsed = parseCalendarDate(raw);
  if (!parsed) {
    fail(400, 'must be a valid calendar date (YYYY-MM-DD)', {
      field: 'examDate',
      reason: 'invalid_format',
    });
  }
  return parsed;
}

function checkMaxScore(value: number): number {
  if (!Number.isFinite(value) || value <= 0 || value > COURSE_EXAM_MAX_SCORE_LIMIT) {
    fail(400, `maxScore must be greater than 0 and at most ${COURSE_EXAM_MAX_SCORE_LIMIT}`, {
      field: 'maxScore',
      reason: 'invalid',
    });
  }
  return value;
}

const MIN_OTHER_SCORERS_FOR_AVERAGE = 2;
/** The fewest scores a snapshot of the results is published of, as the server's. */
const FEWEST_PUBLISHED = 5;

/** The scores of the course's active members for the exam: their sum and count, and how many are another's than `callerId`. */
function scoresOf(
  world: World,
  courseId: string,
  examId: string,
  callerId: string,
): { sum: number; count: number; others: number } {
  let sum = 0;
  let count = 0;
  let others = 0;
  for (const memberId of activeMemberIds(world, courseId)) {
    const score = world.examScores.get(memberId)?.get(examId);
    if (score === undefined) continue;
    sum += score.score;
    count += 1;
    if (memberId !== callerId) others += 1;
  }
  return { sum, count, others };
}

function buildExamView(
  world: World,
  callerId: string,
  courseId: string,
  exam: StoredCourseExam,
): CourseExam {
  const { published, ...stored } = exam;
  const unpublished = { resultsPublishedAt: null, publishedScoreBucket: null };
  if (!activeMemberIds(world, courseId).has(callerId)) {
    return { ...copy(stored), myScore: null, averageScore: null, scoreCount: 0, ...unpublished };
  }
  const myRaw = world.examScores.get(callerId)?.get(exam.id);
  const { sum, count, others } = scoresOf(world, courseId, exam.id, callerId);
  const told = published
    ? {
        averageScore: published.average,
        resultsPublishedAt: published.at,
        publishedScoreBucket: published.bucket,
      }
    : {
        averageScore: count > 0 && others >= MIN_OTHER_SCORERS_FOR_AVERAGE ? sum / count : null,
        ...unpublished,
      };
  return { ...copy(stored), myScore: myRaw ? copy(myRaw) : null, scoreCount: count, ...told };
}

/** Publishes the snapshot of the results, for an admin of the course, of five scores or more; or takes it back. */
export function memorySetExamResultsPublished(
  world: World,
  userId: string,
  courseId: string,
  examId: string,
  publish: boolean,
): CourseExam {
  if (memberRole(world, userId, courseId, 'course') !== 'admin') {
    fail(403, 'access denied to the course');
  }
  const exam = findExam(world, courseId, examId);
  if (!publish) {
    delete exam.published;
    return buildExamView(world, userId, courseId, exam);
  }
  const { sum, count } = scoresOf(world, courseId, examId, userId);
  if (exam.published) fail(409, 'the results are published');
  if (count < FEWEST_PUBLISHED) fail(409, 'too few scores to publish');
  exam.published = {
    at: Date.now(),
    average: Math.round((sum / count) * 10) / 10,
    bucket: Math.floor(count / FEWEST_PUBLISHED) * FEWEST_PUBLISHED,
  };
  return buildExamView(world, userId, courseId, exam);
}

function findExam(world: World, courseId: string, examId: string): StoredCourseExam {
  const list = world.courseExams.get(courseId) ?? [];
  const found = list.find((item) => item.id === examId);
  if (!found) fail(404, 'no such exam');
  return found;
}

export function memoryListCourseExams(
  world: World,
  userId: string,
  courseId: string,
): CourseExam[] {
  memberRole(world, userId, courseId, 'course');
  return [...(world.courseExams.get(courseId) ?? [])]
    .map((exam) => buildExamView(world, userId, courseId, exam))
    .sort(compareCourseExams);
}

export function memoryCreateCourseExam(
  world: World,
  userId: string,
  courseId: string,
  draft: NewCourseExam,
): CourseExam {
  mayManageCourseEvents(world, userId, courseId);
  const title = checkTitle(draft.title, COURSE_EXAM_TITLE_MAX_CHARS);
  const examDate = checkExamDate(draft.examDate);
  const maxScore = checkMaxScore(draft.maxScore);
  const description = checkMultiline(
    draft.description ?? '',
    'description',
    COURSE_EXAM_DESCRIPTION_MAX_CHARS,
  );
  const now = world.now();
  const exam: StoredCourseExam = {
    id: world.newId(),
    containerId: courseId,
    title,
    examDate,
    maxScore,
    description,
    createdAt: now,
    updatedAt: now,
  };
  const existing = world.courseExams.get(courseId) ?? [];
  existing.push(exam);
  world.courseExams.set(courseId, existing);
  return buildExamView(world, userId, courseId, exam);
}

export function memoryPatchCourseExam(
  world: World,
  userId: string,
  courseId: string,
  examId: string,
  patch: CourseExamPatch,
): CourseExam {
  mayManageCourseEvents(world, userId, courseId);
  const exam = findExam(world, courseId, examId);
  // Validate all fields before mutating so a rejected PATCH leaves stored state untouched.
  const nextTitle =
    patch.title === undefined ? undefined : checkTitle(patch.title, COURSE_EXAM_TITLE_MAX_CHARS);
  const hasExamDate = 'examDate' in patch && patch.examDate !== undefined;
  const nextExamDate = hasExamDate ? checkExamDate(patch.examDate) : undefined;
  const nextMaxScore = patch.maxScore === undefined ? undefined : checkMaxScore(patch.maxScore);
  const nextDescription =
    patch.description === undefined
      ? undefined
      : checkMultiline(patch.description, 'description', COURSE_EXAM_DESCRIPTION_MAX_CHARS);

  if (nextTitle !== undefined) exam.title = nextTitle;
  if (hasExamDate) exam.examDate = nextExamDate ?? null;
  if (nextMaxScore !== undefined) exam.maxScore = nextMaxScore;
  if (nextDescription !== undefined) exam.description = nextDescription;
  exam.updatedAt = world.now();
  return buildExamView(world, userId, courseId, exam);
}

export function memoryDeleteCourseExam(
  world: World,
  userId: string,
  courseId: string,
  examId: string,
): void {
  mayManageCourseEvents(world, userId, courseId);
  const list = world.courseExams.get(courseId) ?? [];
  const index = list.findIndex((item) => item.id === examId);
  if (index === -1) fail(404, 'no such exam');
  list.splice(index, 1);
  for (const scores of world.examScores.values()) scores.delete(examId);
}

export function memoryPutMyExamScore(
  world: World,
  userId: string,
  courseId: string,
  examId: string,
  update: MyExamScoreUpdate,
): MyExamScore {
  memberRole(world, userId, courseId, 'course');
  if (!isEnrolledActive(world, userId, courseId)) {
    fail(403, 'join the course before recording your exam score');
  }
  const exam = findExam(world, courseId, examId);
  if (!Number.isFinite(update.score) || update.score < 0) {
    fail(400, 'score must be a finite number greater than or equal to 0', {
      field: 'score',
      reason: 'invalid',
    });
  }
  if (update.score > exam.maxScore) {
    fail(400, `score must be at most ${exam.maxScore}`, { field: 'score', reason: 'invalid' });
  }
  const note = checkMultiline(update.note ?? '', 'note', EXAM_SCORE_NOTE_MAX_CHARS);
  let byExam = world.examScores.get(userId);
  if (!byExam) {
    byExam = new Map();
    world.examScores.set(userId, byExam);
  }
  const saved: MyExamScore = { score: update.score, note, updatedAt: world.now() };
  byExam.set(examId, saved);
  return copy(saved);
}

export function memoryDeleteMyExamScore(
  world: World,
  userId: string,
  courseId: string,
  examId: string,
): void {
  memberRole(world, userId, courseId, 'course');
  if (!isEnrolledActive(world, userId, courseId)) {
    fail(403, 'join the course before deleting your exam score');
  }
  findExam(world, courseId, examId);
  world.examScores.get(userId)?.delete(examId);
}

/** Deletes all personal exam scores for `userId` in `courseId` (used on member removal). */
export function purgeMemberExamScores(world: World, courseId: string, userId: string): void {
  const byExam = world.examScores.get(userId);
  if (!byExam) return;
  for (const exam of world.courseExams.get(courseId) ?? []) byExam.delete(exam.id);
}

export function memoryCourseExams(host: WikiHost, enter: () => Promise<void>): CourseExamsApi {
  return {
    async list(courseId) {
      await enter();
      return memoryListCourseExams(host.world, host.userId, courseId);
    },
    async create(courseId, draft) {
      await enter();
      return memoryCreateCourseExam(host.world, host.userId, courseId, draft);
    },
    async patch(courseId, examId, patch) {
      await enter();
      return memoryPatchCourseExam(host.world, host.userId, courseId, examId, patch);
    },
    async delete(courseId, examId) {
      await enter();
      memoryDeleteCourseExam(host.world, host.userId, courseId, examId);
    },
    async putMyScore(courseId, examId, update) {
      await enter();
      return memoryPutMyExamScore(host.world, host.userId, courseId, examId, update);
    },
    async deleteMyScore(courseId, examId) {
      await enter();
      memoryDeleteMyExamScore(host.world, host.userId, courseId, examId);
    },
    async setResultsPublished(courseId, examId, published) {
      await enter();
      return memorySetExamResultsPublished(host.world, host.userId, courseId, examId, published);
    },
  };
}
