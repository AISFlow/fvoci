/**
 * The courses and their chapters and units on the server (`Backend.course`):
 * the operations under `/w/{workspaceId}/courses` (`getCourses`, `createCourse`,
 * `getUnitTree`, `createUnit`, `getUnit`, `patchUnit`, `deleteUnit`, `moveUnit`,
 * `getDeletedUnits`, `restoreUnit`). What a course shares with a board or wiki
 * (reading, changing, deleting, restoring, members, joining, sidebar, export)
 * goes through the container operations.
 */
import type { DocNode } from '../editor/schema.ts';
import type { Container } from '../logic/wire.ts';
import {
  BackendError,
  type BodyWrite,
  type Comment,
  type CommentVersion,
} from '../state/backend.ts';
import type {
  CourseDraft,
  CourseExamsApi,
  CourseMeetingsApi,
  CourseProgressSummary,
  Courses,
  CourseScheduleUpdate,
  CourseScheduleView,
  CourseUnit,
  CourseUnitDetail,
  RedistributeScheduleRequest,
  UnitDraft,
  UnitMove,
  UnitPatch,
  UnitProgress,
  UnitProgressUpdate,
  UnitScheduleUpdate,
  WorkspaceLearningToday,
} from '../state/course-types.ts';
import type { DeletedItem, Paging } from '../state/trash-reads.ts';
import { BlockTurns } from '../state/write.ts';

import { ServerUnitVersions } from './body-versions.ts';
import {
  bodyOf,
  commentOf,
  commentVersionOf,
  containerOf,
  wireBody,
  type Schemas,
} from './convert.ts';
import { serverCourseExams, serverCourseMeetings } from './course-meetings.ts';
import {
  deleteCourseSchedule,
  getCourseSchedule,
  getWorkspaceLearningToday,
  putCourseSchedule,
  putUnitSchedule as sendUnitSchedule,
  redistributeCourseSchedule,
  redistributeMyCourseSchedule,
} from './course-schedules.ts';
import {
  getCourseProgressSummary,
  getMyCourseProgress,
  putUnitProgress as sendUnitProgress,
} from './unit-progress.ts';
import { ServerUnitReviews } from './unit-reviews.ts';
import type { WikiRequest } from './wikis.ts';

/** A unit or chapter as the outline lists it; `courseId` is the course it was read from. */
export function unitOf(node: Schemas['UnitNode'], courseId: string): CourseUnit {
  return {
    id: node.id,
    courseId,
    kind: node.kind,
    parentId: node.parentId,
    title: node.title,
    order: node.order,
    amount: node.kind === 'unit' ? (node.amount ?? null) : null,
    amountUnit: node.kind === 'unit' ? (node.amountUnit ?? null) : null,
    scheduledDate: node.kind === 'unit' ? (node.scheduledDate ?? null) : null,
    createdAt: node.createdAt,
    updatedAt: node.updatedAt,
  };
}

export function unitDetailOf(unit: Schemas['Unit']): CourseUnitDetail {
  return {
    ...unitOf(unit, unit.courseId),
    body: bodyOf(unit.body),
    createdBy: unit.createdBy,
    modifiedBy: unit.modifiedBy,
  };
}

function amountUnitOf(value: unknown): Schemas['AmountUnit'] | null {
  return value === 'minutes' || value === 'pages' || value === 'problems' ? value : null;
}

/**
 * A unit or chapter as a change message (`blockChanged`) brings it: a block of
 * type `unit` or `chapter` whose `order` and optional schedule fields are among
 * its `fields`.
 */
export function unitOfBlock(block: Schemas['Block']): CourseUnit {
  const fields = block.fields as {
    order?: unknown;
    amount?: unknown;
    amountUnit?: unknown;
    scheduledDate?: unknown;
  };
  return unitOf(
    {
      id: block.id,
      kind: block.type === 'chapter' ? 'chapter' : 'unit',
      parentId: block.parentId,
      title: block.title,
      order: typeof fields.order === 'string' ? fields.order : '',
      amount: typeof fields.amount === 'number' ? fields.amount : null,
      amountUnit: amountUnitOf(fields.amountUnit),
      scheduledDate: typeof fields.scheduledDate === 'string' ? fields.scheduledDate : null,
      createdAt: block.createdAt,
      updatedAt: block.updatedAt,
    },
    block.containerId,
  );
}

function deletedUnitOf(unit: Schemas['DeletedUnit']): DeletedItem {
  return {
    id: unit.id,
    title: unit.title,
    deletedAt: unit.deletedAt,
    deletedBy: unit.deletedBy,
    below: unit.below,
  };
}

function newCourseOf(draft: CourseDraft): Schemas['NewCourse'] {
  return {
    title: draft.title,
    visibility: draft.visibility,
    ...(draft.description ? { description: draft.description } : {}),
    ...(draft.icon ? { icon: draft.icon } : {}),
    ...(draft.minimumRole ? { minimumRole: draft.minimumRole } : {}),
  };
}

function unitPatchOf(patch: UnitPatch): Schemas['UnitPatch'] {
  const body = patch.body === undefined ? {} : { body: patch.body && wireBody(patch.body) };
  return {
    ...(patch.title === undefined ? {} : { title: patch.title }),
    ...body,
    ...(patch.amount === undefined ? {} : { amount: patch.amount }),
    ...(patch.amountUnit === undefined ? {} : { amountUnit: patch.amountUnit }),
    ...(patch.scheduledDate === undefined ? {} : { scheduledDate: patch.scheduledDate }),
    ...(patch.readAt === undefined ? {} : { readAt: patch.readAt }),
  };
}

export class ServerCourses implements Courses {
  readonly #request: WikiRequest;
  readonly #learn: (courses: readonly Schemas['Container'][]) => void;
  readonly #turns = new BlockTurns();

  readonly bodyVersions: ServerUnitVersions<CourseUnitDetail>;
  readonly meetings: CourseMeetingsApi;
  readonly exams: CourseExamsApi;
  readonly reviews: ServerUnitReviews;

  constructor(request: WikiRequest, learn: (courses: readonly Schemas['Container'][]) => void) {
    this.#request = request;
    this.#learn = learn;
    this.bodyVersions = new ServerUnitVersions(request, unitDetailOf);
    this.meetings = serverCourseMeetings(request);
    this.exams = serverCourseExams(request);
    this.reviews = new ServerUnitReviews(request);
  }

  async courses(workspaceId: string): Promise<Container[]> {
    const courses = await this.#request<Schemas['Container'][]>('/courses', {}, {}, workspaceId);
    this.#learn(courses);
    return courses.map(containerOf);
  }

  async createCourse(workspaceId: string, draft: CourseDraft): Promise<Container> {
    const course = await this.#request<Schemas['Container']>(
      '/courses',
      {},
      { method: 'POST', json: newCourseOf(draft) },
      workspaceId,
    );
    this.#learn([course]);
    return containerOf(course);
  }

  async units(courseId: string): Promise<CourseUnit[]> {
    const nodes = await this.#request<Schemas['UnitNode'][]>('/courses/{courseId}/units', {
      courseId,
    });
    return nodes.map((node) => unitOf(node, courseId));
  }

  async unit(courseId: string, unitId: string): Promise<CourseUnitDetail> {
    return unitDetailOf(
      await this.#request<Schemas['Unit']>('/courses/{courseId}/units/{unitId}', {
        courseId,
        unitId,
      }),
    );
  }

  async createUnit(courseId: string, draft: UnitDraft): Promise<CourseUnitDetail> {
    const request: Schemas['NewUnit'] = {
      ...(draft.kind ? { kind: draft.kind } : {}),
      ...(draft.title ? { title: draft.title } : {}),
      ...(draft.parentId ? { parentId: draft.parentId } : {}),
      ...(draft.after === undefined ? {} : { after: draft.after }),
      ...(draft.amount === undefined ? {} : { amount: draft.amount }),
      ...(draft.amountUnit === undefined ? {} : { amountUnit: draft.amountUnit }),
      ...(draft.scheduledDate === undefined ? {} : { scheduledDate: draft.scheduledDate }),
    };
    return unitDetailOf(
      await this.#request<Schemas['Unit']>(
        '/courses/{courseId}/units',
        { courseId },
        { method: 'POST', json: request },
      ),
    );
  }

  patchUnit(courseId: string, unitId: string, patch: UnitPatch): Promise<CourseUnitDetail> {
    return this.#turns.run(unitId, async () =>
      unitDetailOf(
        await this.#request<Schemas['Unit']>(
          '/courses/{courseId}/units/{unitId}',
          { courseId, unitId },
          { method: 'PATCH', json: unitPatchOf(patch) },
        ),
      ),
    );
  }

  async deleteUnit(courseId: string, unitId: string): Promise<void> {
    await this.#request(
      '/courses/{courseId}/units/{unitId}',
      { courseId, unitId },
      { method: 'DELETE' },
    );
  }

  async moveUnit(courseId: string, unitId: string, to: UnitMove): Promise<CourseUnitDetail> {
    const request: Schemas['UnitMove'] = {
      ...(to.parentId ? { parentId: to.parentId } : {}),
      ...(to.after === undefined ? {} : { after: to.after }),
    };
    return unitDetailOf(
      await this.#request<Schemas['Unit']>(
        '/courses/{courseId}/units/{unitId}/move',
        { courseId, unitId },
        { method: 'POST', json: request },
      ),
    );
  }

  async deletedUnits(courseId: string, paging?: Paging): Promise<DeletedItem[]> {
    const units = await this.#request<Schemas['DeletedUnit'][]>(
      '/courses/{courseId}/trash/units',
      { courseId },
      { query: { ...paging } },
    );
    return units.map(deletedUnitOf);
  }

  async restoreUnit(courseId: string, unitId: string): Promise<CourseUnitDetail> {
    return unitDetailOf(
      await this.#request<Schemas['Unit']>(
        '/courses/{courseId}/units/{unitId}/restore',
        { courseId, unitId },
        { method: 'POST' },
      ),
    );
  }

  async comments(courseId: string, unitId: string): Promise<Comment[]> {
    const blocks = await this.#request<Schemas['Block'][]>(
      '/courses/{courseId}/units/{unitId}/comments',
      { courseId, unitId },
    );
    return blocks.map(commentOf);
  }

  async createComment(courseId: string, unitId: string, body: DocNode): Promise<Comment> {
    const request: Schemas['NewUnitComment'] = { body: wireBody(body) };
    return commentOf(
      await this.#request<Schemas['Block']>(
        '/courses/{courseId}/units/{unitId}/comments',
        { courseId, unitId },
        { method: 'POST', json: request },
      ),
    );
  }

  patchComment(
    courseId: string,
    unitId: string,
    commentId: string,
    write: BodyWrite,
  ): Promise<Comment> {
    if (!write.body) {
      return Promise.reject(new BackendError(400, { detail: 'a comment has a text' }));
    }
    const request: Schemas['UnitCommentPatch'] = {
      body: wireBody(write.body),
      readAt: write.readAt,
    };
    return this.#turns.run(commentId, async () =>
      commentOf(
        await this.#request<Schemas['Block']>(
          '/courses/{courseId}/units/{unitId}/comments/{commentId}',
          { courseId, unitId, commentId },
          { method: 'PATCH', json: request },
        ),
      ),
    );
  }

  async deleteComment(courseId: string, unitId: string, commentId: string): Promise<void> {
    await this.#request(
      '/courses/{courseId}/units/{unitId}/comments/{commentId}',
      { courseId, unitId, commentId },
      { method: 'DELETE' },
    );
  }

  async restoreComment(courseId: string, unitId: string, commentId: string): Promise<Comment> {
    return commentOf(
      await this.#request<Schemas['Block']>(
        '/courses/{courseId}/units/{unitId}/comments/{commentId}/restore',
        { courseId, unitId, commentId },
        { method: 'POST' },
      ),
    );
  }

  async commentVersions(
    courseId: string,
    unitId: string,
    commentId: string,
  ): Promise<CommentVersion[]> {
    const versions = await this.#request<Schemas['CommentVersion'][]>(
      '/courses/{courseId}/units/{unitId}/comments/{commentId}/versions',
      { courseId, unitId, commentId },
    );
    return versions.map(commentVersionOf);
  }

  async watching(workspaceId: string, userId: string): Promise<string[]> {
    const subs = await this.#request<Schemas['Subscription'][]>(
      '/subscriptions/{userId}',
      { userId },
      {},
      workspaceId,
    );
    return subs.filter((sub) => sub.deletedAt === undefined).map((sub) => sub.blockId);
  }

  async watchUnit(workspaceId: string, unitId: string, userId: string): Promise<void> {
    const request: Schemas['NewSubscription'] = {
      blockId: unitId,
      blockType: 'unit',
      userId,
    };
    await this.#request<Schemas['Subscription']>(
      '/subscriptions',
      {},
      { method: 'POST', json: request },
      workspaceId,
    );
  }

  async unwatchUnit(workspaceId: string, unitId: string, userId: string): Promise<void> {
    await this.#request(
      '/subscriptions/{blockId}/{userId}',
      { blockId: unitId, userId },
      { method: 'DELETE' },
      workspaceId,
    );
  }

  myProgress(courseId: string, workspaceId?: string): Promise<UnitProgress[]> {
    return getMyCourseProgress(this.#request, workspaceId, courseId);
  }

  putUnitProgress(
    courseId: string,
    unitId: string,
    update: UnitProgressUpdate,
    workspaceId?: string,
  ): Promise<UnitProgress> {
    return sendUnitProgress(this.#request, workspaceId, courseId, unitId, update);
  }

  progressSummary(courseId: string, workspaceId?: string): Promise<CourseProgressSummary> {
    return getCourseProgressSummary(this.#request, workspaceId, courseId);
  }

  schedule(courseId: string, workspaceId?: string): Promise<CourseScheduleView> {
    return getCourseSchedule(this.#request, workspaceId, courseId);
  }

  putSchedule(
    courseId: string,
    update: CourseScheduleUpdate,
    workspaceId?: string,
  ): Promise<CourseScheduleView> {
    return putCourseSchedule(this.#request, workspaceId, courseId, update);
  }

  deleteSchedule(courseId: string, workspaceId?: string): Promise<void> {
    return deleteCourseSchedule(this.#request, workspaceId, courseId);
  }

  redistributeSchedule(
    courseId: string,
    request: RedistributeScheduleRequest,
    workspaceId?: string,
  ): Promise<CourseScheduleView> {
    return redistributeCourseSchedule(this.#request, workspaceId, courseId, request);
  }

  redistributeMySchedule(
    courseId: string,
    request: RedistributeScheduleRequest,
    workspaceId?: string,
  ): Promise<CourseScheduleView> {
    return redistributeMyCourseSchedule(this.#request, workspaceId, courseId, request);
  }

  putUnitSchedule(
    courseId: string,
    unitId: string,
    update: UnitScheduleUpdate,
    workspaceId?: string,
  ): Promise<UnitProgress> {
    return sendUnitSchedule(this.#request, workspaceId, courseId, unitId, update);
  }

  learningToday(workspaceId: string): Promise<WorkspaceLearningToday> {
    return getWorkspaceLearningToday(this.#request, workspaceId);
  }
}
