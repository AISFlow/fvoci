/**
 * Live change handling, background resync, and container mutation helpers for
 * `CourseStore` (`course.svelte.ts`), extracted to keep `CourseStore` <= 500 lines.
 */
import type { Container } from '../logic/wire.ts';

import {
  isBackendError,
  type Change,
  type Courses,
  type CourseUnit,
  type CourseUnitDetail,
  type UnitDraft,
} from './backend.ts';
import { isCommentBlock } from './changes.ts';
import type { CourseLooks } from './course-types.ts';
import type { CourseStore, CourseStoreTab } from './course.svelte.ts';
import type { HierarchyWriteQueue } from './hierarchy-writes.ts';
import type { MemberWrites } from './member-writes.ts';
import { memberId } from './user-names.ts';
import { reread } from './write.ts';

export interface CourseSyncHost {
  readonly store: CourseStore;
  readonly courses: () => Courses;
  readonly writes: HierarchyWriteQueue<CourseUnit, CourseUnitDetail, UnitDraft>;
  readonly people: MemberWrites;
  activeTab(): CourseStoreTab;
  isClosed(): boolean;
  nextReading(): number;
  currentReading(): number;
  clearWatchingCache(): void;
  takeCourse(course: Container): void;
  refreshPermissions(): Promise<void>;
  refreshActiveTab(): void;
}

export function applyCourseChange(host: CourseSyncHost, change: Change): void {
  const { store, writes, people } = host;
  switch (change.kind) {
    case 'container':
      if (change.container.id !== store.courseId) break;
      if (store.missing) void store.load({ activeTab: host.activeTab() });
      else host.takeCourse(change.container);
      break;
    case 'containerDeleted':
      if (change.containerId !== store.courseId) break;
      if (store.course?.visibility === 'open' && !host.isClosed()) {
        void store.load({ activeTab: host.activeTab() });
      } else {
        store.workspace.evictContainer(store.courseId);
        store.openUnit?.close();
        store.openUnit = undefined;
        [store.missing, store.status] = [true, 'failed'];
      }
      break;
    case 'units':
      for (const unit of change.units) store.receiveUnit(unit);
      void store.schedule.refresh();
      break;
    case 'blocks':
      for (const block of change.blocks) {
        if (isCommentBlock(block)) store.openUnit?.comments.receive(block);
      }
      break;
    case 'reactions':
      store.openUnit?.comments.receiveReactions(change.blockId, change.reactions);
      break;
    case 'blocksDeleted': {
      const hadUnit = store.units.some((unit) => change.blockIds.includes(unit.id));
      writes.forget(change.blockIds, 'others');
      store.openUnit?.comments.forget(change.blockIds);
      if (hadUnit) void store.schedule.refresh();
      break;
    }
    case 'members': {
      if (change.containerId !== store.courseId) break;
      const next = change.members;
      const removed = store.members.some((p) => !next.some((n) => memberId(n) === memberId(p)));
      people.told += 1;
      store.members = next;
      const meId = store.workspace.me?.id;
      if (meId && !next.some((m) => memberId(m) === meId)) {
        store.progress.clearMine();
        store.schedule.clearMine();
        store.events.clearMine();
        host.clearWatchingCache();
        if (store.course?.visibility !== 'open') store.workspace.evictContainer(store.courseId);
      } else if (removed && host.activeTab() === 'exams') void store.events.refreshExams();
      void store.workspace.learnUsers(next.map(memberId), { members: true });
      void host.refreshPermissions();
      break;
    }
    case 'workspaceRole':
      if (change.userId === store.workspace.me?.id) void host.refreshPermissions();
      break;
    case 'resync':
      void refreshCourse(host);
      break;
    default:
      break;
  }
}

export async function refreshCourse(host: CourseSyncHost): Promise<void> {
  const { store, writes, people } = host;
  if (store.status !== 'ready' || host.isClosed()) return;
  const reading = host.nextReading();
  const before = store.units.map((unit) => unit.id);
  const mark = writes.forgettings;
  const membersTold = people.told;
  const progressMark = store.progress.writeVersion;
  const scheduleMark = store.schedule.writeVersion;
  host.clearWatchingCache();
  try {
    const courses = host.courses();
    const workspaceId = store.workspace.info?.id;
    const [course, units, members, permissions, progress, summary, schedule] = await Promise.all([
      store.workspace.backend.container(store.courseId),
      courses.units(store.courseId),
      store.workspace.backend.members(store.courseId),
      store.workspace.backend.myPermissions(store.courseId),
      courses.myProgress(store.courseId, workspaceId),
      courses.progressSummary(store.courseId, workspaceId),
      courses.schedule(store.courseId, workspaceId),
    ]);
    if (host.isClosed() || reading !== host.currentReading()) return;
    host.takeCourse(course);
    store.permissions = permissions;
    if (membersTold === people.told) store.members = members;
    store.progress.take(progress, summary, progressMark);
    store.schedule.take(schedule, scheduleMark);
    host.refreshActiveTab();
    // The meetings and the exams too, which bring their tabs: the server tells nobody of them.
    void store.events.refreshAll();
    const shown = store.openUnit;
    const title = (shown && store.unit(shown.unitId)?.title) ?? '';
    const read = writes.admitted(units, mark);
    store.units = reread(store.units, read, before, (unit) => writes.receive(unit));
    writes.remember(store.units);
    if (shown && !store.unit(shown.unitId)) {
      if (!writes.isDeleting(shown.unitId)) shown.gone('others', title);
    } else if (shown) void shown.refresh();
  } catch (error) {
    if (host.isClosed() || reading !== host.currentReading()) return;
    if (isBackendError(error) && error.status === 401) {
      store.workspace.report('load', error);
      return;
    }
    if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
      store.workspace.evictContainer(store.courseId);
      void store.load({ activeTab: host.activeTab() });
    }
  }
}

export async function changeCourseContainer(
  store: CourseStore,
  takeCourse: (course: Container) => void,
  looks: CourseLooks,
  step: 'changeCourse' | 'changeVisibility' | 'changeMinimumRole' = 'changeCourse',
): Promise<void> {
  const before = store.course;
  if (!before) return;
  const back: CourseLooks = {
    ...(looks.title === undefined ? {} : { title: before.title }),
    ...(looks.description === undefined ? {} : { description: before.description }),
    ...(looks.icon === undefined ? {} : { icon: before.icon ?? '' }),
    ...(looks.visibility === undefined ? {} : { visibility: before.visibility }),
    ...(looks.minimumRole === undefined ? {} : { minimumRole: before.minimumRole }),
  };
  const write = async (to: CourseLooks, from: CourseLooks): Promise<void> => {
    if (store.course) store.course = { ...store.course, ...to };
    try {
      const changed = await store.workspace.backend.patchContainer(store.courseId, to);
      takeCourse(changed);
      store.workspace.receiveContainer(changed);
    } catch (error) {
      if (store.course) store.course = { ...store.course, ...from };
      throw error;
    }
  };
  try {
    await store.workspace.undo.perform(
      () => write(looks, back),
      () => write(back, looks),
      step,
    );
  } catch (error) {
    store.workspace.report('save', error);
  }
}
