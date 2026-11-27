/**
 * The courses of the backend in memory (`Backend.course`), following the server
 * rules (`crates/store/src/courses.rs`, `page_places.rs`):
 * - A course is a container of kind `course` (no template, no card properties).
 * - Chapters (`type: 'chapter'`) are title-only top-level groupings (`parentId === ''`,
 *   `body === null`, no collab room).
 * - Units (`type: 'unit'`) sit at the top or directly under a chapter (`depth <= 2`).
 * - Independent limits: 200 chapters (`too_many_chapters`) and 2,000 units (`too_many_units`).
 * - Deleting a chapter deletes its units together as one trash entry; restoring a unit
 *   whose chapter is gone restores it last at the top.
 */
import type { DocNode } from '../../editor/schema.ts';
import { createBoard, type Container } from '../../logic/wire.ts';
import { BackendError, type DeletedItem, type Paging } from '../backend.ts';
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
  UnitKind,
  UnitMove,
  UnitPatch,
  UnitProgress,
  UnitProgressUpdate,
  UnitScheduleUpdate,
  WorkspaceLearningToday,
} from '../course-types.ts';

import { checkedWhole } from './bodies.ts';
import { MemoryBodyVersions, writtenWhole } from './body-versions.ts';
import { fileAsNew } from './categories.ts';
import { memberRole } from './container-access.ts';
import { memoryCourseExams } from './course-exams.ts';
import { memoryCourseMeetings } from './course-meetings.ts';
import {
  memoryDeleteCourseSchedule,
  memoryGetCourseSchedule,
  memoryGetWorkspaceLearningToday,
  memoryPutCourseSchedule,
  memoryPutUnitSchedule,
  memoryRedistributeCourseSchedule,
  memoryRedistributeMyCourseSchedule,
} from './course-schedules.ts';
import {
  checkedUnitScheduleFields,
  checkedUnitTitle as checkedTitle,
  isCourseBlock,
  memoryRestoreUnit,
  patchedUnitScheduleFields,
  unitDetailOf as detailOf,
} from './course-unit-fields.ts';
import { MemoryPageComments, type OnPage } from './page-comments.ts';
import {
  DEFAULT_COURSE_LIMITS,
  PageTree,
  refuseLimit,
  type CourseLimits,
  type Place,
} from './page-tree.ts';
import { fail } from './refuse.ts';
import { copy, memberOf, orderOf, scramble, unitOf } from './shapes.ts';
import { checkWorkspace } from './spaces.ts';
import { deletedUnitsOf, deletion } from './trash.ts';
import { memoryMyProgress, memoryProgressSummary, memoryPutUnitProgress } from './unit-progress.ts';
import { MemoryUnitReviews } from './unit-reviews.ts';
import {
  memoryUnwatchUnit,
  memoryWatchingUnits,
  memoryWatchUnit,
  setUnitWatch,
} from './unit-watch.ts';
import type { WikiHost } from './wikis.ts';
import type { StoredBlock, StoredCard, World } from './world.ts';

const isCourse = (board: Container | undefined): board is Container => board?.kind === 'course';

export class MemoryCourses extends MemoryPageComments implements Courses {
  readonly #host: WikiHost;
  readonly #failures = new Map<keyof Courses, BackendError>();
  /** The limits a memory course is held to; a test may lower them to reach them. */
  limits: CourseLimits = DEFAULT_COURSE_LIMITS;
  #bodyVersions: MemoryBodyVersions | undefined;
  readonly meetings: CourseMeetingsApi;
  readonly exams: CourseExamsApi;
  #reviews: MemoryUnitReviews | undefined;

  constructor(host: WikiHost) {
    super();
    this.#host = host;
    this.meetings = memoryCourseMeetings(host, () => this.#enter('meetings'));
    this.exams = memoryCourseExams(host, () => this.#enter('exams'));
  }

  get #world(): World {
    return this.#host.world;
  }

  get reviews(): MemoryUnitReviews {
    return (this.#reviews ??= new MemoryUnitReviews(this.#host, () => this.#enter('reviews')));
  }

  get bodyVersions(): MemoryBodyVersions {
    return (this.#bodyVersions ??= new MemoryBodyVersions(
      this.#world,
      this.#host.userId,
      () => this.#enter('bodyVersions'),
      'unit',
    ));
  }

  protected enter(method: keyof Courses): Promise<void> {
    return this.#enter(method);
  }

  protected on(courseId: string, unitId: string): OnPage {
    return {
      world: this.#world,
      userId: this.#host.userId,
      wikiId: courseId,
      pageId: unitId,
      containerKind: 'course',
      blockType: 'unit',
    };
  }

  override async createComment(courseId: string, unitId: string, body: DocNode) {
    const comment = await super.createComment(courseId, unitId, body);
    const course = this.#world.containers.get(courseId);
    if (course && !course.isTemplate && course.workspaceId === this.#world.workspace.id) {
      setUnitWatch(this.#world, this.#host.userId, unitId, true);
    }
    return comment;
  }

  /** Makes the next call of `method` reject with `error`, once. */
  failNext(method: keyof Courses, error: BackendError): void {
    this.#failures.set(method, error);
  }

  async #enter(method: keyof Courses): Promise<void> {
    if (this.#host.latency > 0)
      await new Promise((resolve) => setTimeout(resolve, this.#host.latency));
    else await Promise.resolve();
    const failure = this.#failures.get(method);
    if (failure) {
      this.#failures.delete(method);
      throw failure;
    }
  }

  #role(courseId: string): string {
    return memberRole(this.#world, this.#host.userId, courseId, 'course');
  }

  #mayWrite(courseId: string): void {
    const role = this.#role(courseId);
    if (role !== 'admin' && role !== 'editor') fail(403, 'access denied to the course');
  }

  #node(courseId: string, unitId: string): StoredBlock {
    const block = this.#world.blocks.get(unitId);
    if (!block || !isCourseBlock(block) || block.containerId !== courseId) {
      fail(404, 'no such unit');
    }
    return block;
  }

  #nodesOf(courseId: string): StoredBlock[] {
    return this.#world.blocksOf(courseId).filter(isCourseBlock);
  }

  #tellUnits(courseId: string, blocks: readonly StoredBlock[]): void {
    this.#world.tellContainer(courseId, {
      kind: 'units',
      containerId: courseId,
      units: blocks.map(unitOf),
    });
  }

  #tree(courseId: string): PageTree {
    return new PageTree(
      this.#nodesOf(courseId).map((block) => ({
        id: block.id,
        parentId: block.parentId,
        order: orderOf(block),
      })),
    );
  }

  #writePlace(node: StoredBlock, place: Place): StoredBlock[] {
    const now = this.#world.now();
    const renumbered = place.renumbered.flatMap(([id, order]) => {
      const sibling = this.#world.blocks.get(id);
      if (!sibling) return [];
      sibling.fields = { ...sibling.fields, order };
      sibling.updatedAt = now;
      return [sibling];
    });
    node.fields = { ...node.fields, order: place.order };
    return renumbered;
  }

  #checkParent(courseId: string, kind: UnitKind, parentId: string, selfId?: string): void {
    if (parentId === '') return;
    if (kind === 'chapter') {
      fail(400, 'a chapter goes at the top of the course', { field: 'parentId' });
    }
    if (selfId !== undefined && parentId === selfId) {
      fail(400, 'the parent is the unit itself', { field: 'parentId' });
    }
    const parent = this.#world.blocks.get(parentId);
    if (parent?.type !== 'chapter' || parent.containerId !== courseId) {
      fail(400, 'the parent is no chapter of the course', { field: 'parentId' });
    }
  }

  #checkCounts(courseId: string, addChapters: number, addUnits: number): void {
    const nodes = this.#nodesOf(courseId);
    const chapters = nodes.filter((block) => block.type === 'chapter').length;
    const units = nodes.filter((block) => block.type === 'unit').length;
    if (addChapters > 0 && chapters + addChapters > this.limits.chapters) {
      refuseLimit('too_many_chapters', this.limits.chapters);
    }
    if (addUnits > 0 && units + addUnits > this.limits.units) {
      refuseLimit('too_many_units', this.limits.units);
    }
  }

  async courses(workspaceId: string): Promise<Container[]> {
    await this.#enter('courses');
    checkWorkspace(this.#world, this.#host.userId, workspaceId);
    const userId = this.#host.userId;
    const theirs = [...this.#world.containers.values()].filter(
      (board) =>
        isCourse(board) &&
        !board.isTemplate &&
        board.workspaceId === workspaceId &&
        (board.visibility === 'open' ||
          (this.#world.members.get(board.id) ?? []).some((member) => member.userId === userId)),
    );
    return scramble(theirs).map(copy);
  }

  async createCourse(workspaceId: string, draft: CourseDraft): Promise<Container> {
    await this.#enter('createCourse');
    checkWorkspace(this.#world, this.#host.userId, workspaceId);
    const world = this.#world;
    const userId = this.#host.userId;
    const now = world.now();
    const course: Container = {
      ...createBoard(),
      id: world.newId(),
      workspaceId,
      kind: 'course',
      createdBy: userId,
      modifiedBy: userId,
      visibility: draft.visibility,
      minimumRole: draft.minimumRole ?? 'commenter',
      title: draft.title,
      description: draft.description ?? '',
      icon: draft.icon ?? '',
      cardProperties: [],
      createdAt: now,
      updatedAt: now,
    };
    world.containers.set(course.id, course);
    world.members.set(course.id, [memberOf(course.id, userId, 'admin')]);
    world.tellContainer(course.id, { kind: 'container', container: copy(course) });
    world.tellUsers([userId], { kind: 'container', container: copy(course) });
    fileAsNew(world, userId, course);
    return copy(course);
  }

  async units(courseId: string): Promise<CourseUnit[]> {
    await this.#enter('units');
    this.#role(courseId);
    return scramble(this.#nodesOf(courseId)).map(unitOf);
  }

  async unit(courseId: string, unitId: string): Promise<CourseUnitDetail> {
    await this.#enter('unit');
    this.#role(courseId);
    return detailOf(this.#node(courseId, unitId));
  }

  async createUnit(courseId: string, draft: UnitDraft): Promise<CourseUnitDetail> {
    await this.#enter('createUnit');
    this.#mayWrite(courseId);
    const kind: UnitKind = draft.kind ?? 'unit';
    const title = checkedTitle(draft.title ?? '');
    const parentId = draft.parentId ?? '';
    this.#checkParent(courseId, kind, parentId);
    const scheduleFields = checkedUnitScheduleFields(kind, draft);
    this.#checkCounts(courseId, kind === 'chapter' ? 1 : 0, kind === 'unit' ? 1 : 0);
    const place = this.#tree(courseId).place(parentId, draft.after);
    const now = this.#world.now();
    const userId = this.#host.userId;
    const made: StoredBlock = {
      id: this.#world.newId(),
      containerId: courseId,
      parentId,
      createdBy: userId,
      modifiedBy: userId,
      type: kind,
      title,
      fields: scheduleFields,
      body: null,
      createdAt: now,
      updatedAt: now,
    };
    const renumbered = this.#writePlace(made, place);
    this.#world.blocks.set(made.id, made);
    this.#tellUnits(courseId, [...renumbered, made]);
    return detailOf(made);
  }

  async patchUnit(courseId: string, unitId: string, patch: UnitPatch): Promise<CourseUnitDetail> {
    await this.#enter('patchUnit');
    this.#mayWrite(courseId);
    const node = this.#node(courseId, unitId);
    if (node.type === 'chapter' && patch.body !== undefined) {
      fail(400, 'a chapter has no body', { field: 'body' });
    }
    const title = patch.title === undefined ? undefined : checkedTitle(patch.title);
    const nextFields = patchedUnitScheduleFields(node, patch);
    if (patch.body !== undefined && patch.readAt === undefined) {
      fail(400, 'a body is written with the time it was read', { field: 'readAt' });
    }
    const body: DocNode | null | undefined =
      patch.body === undefined
        ? undefined
        : checkedWhole(this.#world, courseId, unitId, patch.body);
    if (patch.readAt !== undefined && patch.readAt !== node.updatedAt) {
      throw new BackendError(409, { detail: 'the unit changed since it was read' });
    }
    const write = (): void => {
      if (title !== undefined) node.title = title;
      if (body !== undefined) node.body = body;
      if (nextFields !== undefined) node.fields = nextFields;
      node.updatedAt = this.#world.now();
      node.modifiedBy = this.#host.userId;
    };
    if (body === undefined) write();
    else writtenWhole(this.#world, node as StoredCard, body, this.#host.userId, write);
    if (body !== undefined) this.#world.bodyWritten?.(unitId);
    this.#tellUnits(courseId, [node]);
    return detailOf(node);
  }

  async moveUnit(courseId: string, unitId: string, to: UnitMove): Promise<CourseUnitDetail> {
    await this.#enter('moveUnit');
    this.#mayWrite(courseId);
    const node = this.#node(courseId, unitId);
    const kind: UnitKind = node.type === 'chapter' ? 'chapter' : 'unit';
    const parentId = to.parentId ?? '';
    this.#checkParent(courseId, kind, parentId, unitId);
    const tree = this.#tree(courseId);
    if (node.parentId === parentId && tree.isAt(unitId, parentId, to.after)) return detailOf(node);
    const place = tree.place(parentId, to.after, unitId);
    const renumbered = this.#writePlace(node, place);
    node.parentId = parentId;
    node.updatedAt = this.#world.now();
    node.modifiedBy = this.#host.userId;
    this.#tellUnits(courseId, [...renumbered, node]);
    return detailOf(node);
  }

  async deleteUnit(courseId: string, unitId: string): Promise<void> {
    await this.#enter('deleteUnit');
    this.#mayWrite(courseId);
    const node = this.#node(courseId, unitId);
    const gone = [node];
    for (const above of gone) gone.push(...this.#world.childrenOf(above.id));
    this.#world.deletedBlocks.set(unitId, {
      containerId: courseId,
      blocks: gone,
      ...deletion({ world: this.#world, userId: this.#host.userId }),
    });
    for (const block of gone) this.#world.blocks.delete(block.id);
    this.#world.tellContainer(courseId, {
      kind: 'blocksDeleted',
      containerId: courseId,
      blockIds: [unitId],
    });
  }

  async deletedUnits(courseId: string, paging?: Paging): Promise<DeletedItem[]> {
    await this.#enter('deletedUnits');
    this.#mayWrite(courseId);
    return deletedUnitsOf(this.#world, courseId, paging);
  }

  async restoreUnit(courseId: string, unitId: string): Promise<CourseUnitDetail> {
    await this.#enter('restoreUnit');
    this.#mayWrite(courseId);
    return memoryRestoreUnit({
      world: this.#world,
      userId: this.#host.userId,
      courseId,
      unitId,
      tree: this.#tree(courseId),
      checkCounts: (chapters, units) => {
        this.#checkCounts(courseId, chapters, units);
      },
      writePlace: (node, place) => this.#writePlace(node, place),
      tellUnits: (blocks) => {
        this.#tellUnits(courseId, blocks);
      },
    });
  }

  async watching(workspaceId: string, userId: string): Promise<string[]> {
    await this.#enter('watching');
    return memoryWatchingUnits(this.#world, this.#host.userId, workspaceId, userId);
  }

  async watchUnit(workspaceId: string, unitId: string, userId: string): Promise<void> {
    await this.#enter('watchUnit');
    memoryWatchUnit(this.#world, this.#host.userId, workspaceId, unitId, userId);
  }

  async unwatchUnit(workspaceId: string, unitId: string, userId: string): Promise<void> {
    await this.#enter('unwatchUnit');
    memoryUnwatchUnit(this.#world, this.#host.userId, workspaceId, unitId, userId);
  }

  async myProgress(courseId: string): Promise<UnitProgress[]> {
    await this.#enter('myProgress');
    return memoryMyProgress(this.#world, this.#host.userId, courseId);
  }

  async putUnitProgress(
    courseId: string,
    unitId: string,
    update: UnitProgressUpdate,
  ): Promise<UnitProgress> {
    await this.#enter('putUnitProgress');
    return memoryPutUnitProgress(this.#world, this.#host.userId, courseId, unitId, update);
  }

  async progressSummary(courseId: string): Promise<CourseProgressSummary> {
    await this.#enter('progressSummary');
    return memoryProgressSummary(this.#world, this.#host.userId, courseId);
  }

  async schedule(courseId: string): Promise<CourseScheduleView> {
    await this.#enter('schedule');
    return memoryGetCourseSchedule(this.#world, this.#host.userId, courseId);
  }

  async putSchedule(courseId: string, update: CourseScheduleUpdate): Promise<CourseScheduleView> {
    await this.#enter('putSchedule');
    return memoryPutCourseSchedule(this.#world, this.#host.userId, courseId, update);
  }

  async deleteSchedule(courseId: string): Promise<void> {
    await this.#enter('deleteSchedule');
    memoryDeleteCourseSchedule(this.#world, this.#host.userId, courseId);
  }

  async redistributeSchedule(
    courseId: string,
    request: RedistributeScheduleRequest,
  ): Promise<CourseScheduleView> {
    await this.#enter('redistributeSchedule');
    return memoryRedistributeCourseSchedule(this.#world, this.#host.userId, courseId, request);
  }

  async redistributeMySchedule(
    courseId: string,
    request: RedistributeScheduleRequest,
  ): Promise<CourseScheduleView> {
    await this.#enter('redistributeMySchedule');
    return memoryRedistributeMyCourseSchedule(this.#world, this.#host.userId, courseId, request);
  }

  async putUnitSchedule(
    courseId: string,
    unitId: string,
    update: UnitScheduleUpdate,
  ): Promise<UnitProgress> {
    await this.#enter('putUnitSchedule');
    return memoryPutUnitSchedule(this.#world, this.#host.userId, courseId, unitId, update);
  }

  async learningToday(workspaceId: string): Promise<WorkspaceLearningToday> {
    await this.#enter('learningToday');
    return memoryGetWorkspaceLearningToday(this.#world, this.#host.userId, workspaceId);
  }
}
