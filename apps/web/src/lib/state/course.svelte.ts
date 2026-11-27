/**
 * One open course: the course container itself, its members, its outline of
 * chapters and units (`pageTree`), and the open `UnitStore`. Optimistic
 * hierarchy writes are handled by `HierarchyWriteQueue`.
 */
import { isEmptyDoc } from '../editor/schema.ts';
import type { PagePlace } from '../logic/page-moves.ts';
import { pageTree } from '../logic/page-tree.ts';
import type { Container, ContainerMember, MemberRole } from '../logic/wire.ts';

import {
  isBackendError,
  READ_ONLY_PERMISSIONS,
  type ContainerPermissions,
  type ContainerRole,
  type Courses,
  type CourseUnit,
  type CourseUnitDetail,
  type DeletedItem,
  type Paging,
  type UnitDraft,
} from './backend.ts';
import { containerOfWorkspace, roleOf } from './board-parts.ts';
import { ContainerPresence } from './board-presence.svelte.ts';
import { CourseMeetingsState } from './course-meetings.svelte.ts';
import { CourseProgress } from './course-progress.svelte.ts';
import { CourseScheduleState } from './course-schedule.svelte.ts';
import {
  applyCourseChange,
  changeCourseContainer,
  refreshCourse,
  type CourseSyncHost,
} from './course-sync.ts';
import type {
  CourseLooks,
  CourseProgressSummary,
  ScheduledUnit,
  UnitProgress,
} from './course-types.ts';
export type { CourseLooks };
import { HierarchyWriteQueue } from './hierarchy-writes.ts';
import { MemberWrites } from './member-writes.ts';
import { noteMembership } from './memberships.svelte.ts';
import { orderMembers } from './order.ts';
import type { TreeSource } from './tree-source.ts';
import { UnitStore } from './unit.svelte.ts';
import { memberId } from './user-names.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';
import { take } from './write.ts';

export type CourseStoreTab = 'units' | 'meetings' | 'exams';

export class CourseStore implements TreeSource<CourseUnit> {
  readonly kind = 'course' as const;
  readonly workspace: Workspace;
  readonly courseId: string;

  status = $state<LoadStatus>('idle');
  missing = $state(false);
  joining = $state(false);
  course = $state.raw<Container | undefined>(undefined);
  units = $state.raw<CourseUnit[]>([]);
  #members = $state.raw<ContainerMember[]>([]);
  /** Its members as read; whether the reader is one is told to the sidebar (`noteMembership`). */
  get members(): ContainerMember[] {
    return this.#members;
  }
  set members(members: ContainerMember[]) {
    this.#members = members;
    noteMembership(this.workspace, this.courseId, members);
  }
  permissions = $state.raw<ContainerPermissions>({ ...READ_ONLY_PERMISSIONS });
  openUnit = $state.raw<UnitStore | undefined>(undefined);

  readonly tree = $derived(pageTree(this.units));
  readonly orderedMembers = $derived.by(() =>
    orderMembers(
      this.members,
      (userId) => this.workspace.userName(userId) ?? '',
      this.workspace.locale,
    ),
  );
  readonly role = $derived<MemberRole>(
    roleOf(this.members.find((member) => member.userId === this.workspace.me?.id)),
  );
  readonly canEdit = $derived(this.permissions.canEdit);
  readonly canManage = $derived(
    this.permissions.canManageVisibility || this.permissions.canManageRoles,
  );
  readonly canDelete = $derived(this.permissions.canDelete);
  readonly canShare = $derived(this.permissions.canShare);
  readonly joinable = $derived(
    this.status === 'ready' && this.course?.visibility === 'open' && this.role === '',
  );
  readonly canTrackProgress = $derived(this.status === 'ready' && this.role !== '');

  #stopWatching: (() => void) | undefined;
  #closed = false;
  #readings = 0;
  #activeTab: CourseStoreTab = 'units';
  #speculativeUnitId: string | undefined;
  #watchingIds: Set<string> | undefined;
  #watchingLoad: Promise<Set<string> | undefined> | undefined;

  readonly presence: ContainerPresence;
  readonly progress: CourseProgress;
  readonly schedule: CourseScheduleState;
  readonly events: CourseMeetingsState;
  readonly #writes: HierarchyWriteQueue<CourseUnit, CourseUnitDetail, UnitDraft>;
  readonly #people = new MemberWrites(this);
  readonly #sync: CourseSyncHost;

  constructor(workspace: Workspace, courseId: string) {
    this.workspace = workspace;
    this.courseId = courseId;
    const base = {
      courseId,
      workspace,
      courses: () => this.#courses,
      canTrack: () => this.canTrackProgress,
      isClosed: () => this.#closed,
    };
    this.presence = new ContainerPresence({ workspace, containerId: courseId });
    this.progress = new CourseProgress({
      ...base,
      units: () => this.units,
      onCompletionChanged: (unitId, completed) => {
        this.schedule.reconcileCompletion(unitId, completed);
      },
    });
    this.schedule = new CourseScheduleState({
      ...base,
      canEdit: () => this.canEdit,
      takeUnit: (unit) => {
        this.receiveUnit(unit);
      },
      applyProgress: (row) => {
        this.progress.applyRow(row);
      },
      refreshMine: () => void this.progress.refreshMine(),
      syncUnitDates: (units) => {
        this.#syncUnitDates(units);
      },
    });
    this.events = new CourseMeetingsState({
      ...base,
      canEdit: () => this.canEdit,
      canManageRoles: () => this.permissions.canManageRoles,
    });
    this.#writes = new HierarchyWriteQueue({
      workspace,
      nodes: () => this.units,
      setNodes: (units) => {
        this.units = units;
      },
      openNode: () => this.openUnit,
      takeNode: (unit) => {
        this.#takeUnit(unit);
      },
      reread: () => void refreshCourse(this.#sync),
      create: (draft) => this.#courses.createUnit(courseId, draft),
      hasBody: async (unitId) => {
        const { kind, body } = await this.#courses.unit(courseId, unitId);
        return kind === 'unit' && body !== null && !isEmptyDoc(body);
      },
      remove: (unitId) => this.#courses.deleteUnit(courseId, unitId),
      restore: (unitId) => this.#courses.restoreUnit(courseId, unitId),
      move: (unitId, to) => this.#courses.moveUnit(courseId, unitId, to),
      patchTitle: (unitId, title) => this.#courses.patchUnit(courseId, unitId, { title }),
    });
    this.#sync = {
      store: this,
      courses: () => this.#courses,
      writes: this.#writes,
      people: this.#people,
      activeTab: () => this.#activeTab,
      isClosed: () => this.#closed,
      nextReading: () => ++this.#readings,
      currentReading: () => this.#readings,
      clearWatchingCache: () => {
        this.#watchingIds = undefined;
      },
      takeCourse: (course) => {
        this.#takeCourse(course);
      },
      refreshPermissions: () => this.#refreshPermissions(),
      refreshActiveTab: () => {
        this.#refreshActiveTab();
      },
    };
  }

  async #refreshPermissions(): Promise<void> {
    const reading = this.#readings;
    try {
      const permissions = await this.workspace.backend.myPermissions(this.courseId);
      if (!this.#closed && reading === this.#readings) this.permissions = permissions;
    } catch {
      // Handled on next load/refresh if access ended.
    }
  }

  get #courses(): Courses {
    const courses = this.workspace.backend.course;
    if (!courses) throw new Error('this backend has no courses');
    return courses;
  }

  get pages(): readonly CourseUnit[] {
    return this.units;
  }

  page(id: string | undefined): CourseUnit | undefined {
    return this.unit(id);
  }

  unit(unitId: string | undefined): CourseUnit | undefined {
    return this.units.find((unit) => unit.id === unitId);
  }

  async isWatchingUnit(unitId: string): Promise<boolean | undefined> {
    if (this.#watchingIds) return this.#watchingIds.has(unitId);
    const workspaceId = this.workspace.info?.id;
    const meId = this.workspace.me?.id;
    if (!workspaceId || !meId) return undefined;
    this.#watchingLoad ??= this.#courses
      .watching(workspaceId, meId)
      .then((ids) => {
        // eslint-disable-next-line svelte/prefer-svelte-reactivity -- non-rendered subscription cache
        const set = new Set(ids);
        this.#watchingIds = set;
        return set;
      })
      .catch(() => undefined)
      .finally(() => {
        this.#watchingLoad = undefined;
      });
    const set = await this.#watchingLoad;
    return set?.has(unitId);
  }

  noteWatching(unitId: string, watching: boolean): void {
    if (!this.#watchingIds) return;
    if (watching) this.#watchingIds.add(unitId);
    else this.#watchingIds.delete(unitId);
  }

  clearSpeculativeUnit(unitId: string): void {
    if (this.#speculativeUnitId !== unitId) return;
    this.#speculativeUnitId = undefined;
    if (this.openUnit?.unitId === unitId) {
      this.openUnit.close();
      this.openUnit = undefined;
    }
  }

  setActiveTab(tab: CourseStoreTab): void {
    const changed = this.#activeTab !== tab;
    this.#activeTab = tab;
    if (changed && this.status === 'ready') this.#refreshActiveTab();
  }

  #refreshActiveTab(): void {
    if (this.#activeTab === 'meetings') void this.events.refreshMeetings();
    else if (this.#activeTab === 'exams') void this.events.refreshExams();
  }

  async load(options: { unitId?: string; activeTab?: CourseStoreTab } = {}): Promise<void> {
    this.status = 'loading';
    this.missing = false;
    this.#readings += 1;
    const reading = this.#readings;
    const membersTold = this.#people.told;
    const progressMark = this.progress.writeVersion;
    const scheduleMark = this.schedule.writeVersion;
    if (options.activeTab) this.#activeTab = options.activeTab;
    const initialUnitId = this.#activeTab === 'units' ? options.unitId : undefined;
    try {
      this.#stopWatching?.();
      this.#stopWatching = this.workspace.backend.watchContainer(this.courseId, (change) => {
        applyCourseChange(this.#sync, change);
      });
      const backend = this.workspace.backend;
      const workspaceId = this.workspace.info?.id;
      const reads = Promise.all([
        containerOfWorkspace(backend, this.courseId, workspaceId, 'course'),
        this.#courses.units(this.courseId),
        backend.members(this.courseId),
        backend.myPermissions(this.courseId),
        this.#courses.myProgress(this.courseId, workspaceId),
        this.#courses.progressSummary(this.courseId, workspaceId),
        this.#courses.schedule(this.courseId, workspaceId),
      ]);
      if (initialUnitId) {
        this.#speculativeUnitId = initialUnitId;
        this.showUnit(initialUnitId, { speculative: true });
      } else {
        this.#speculativeUnitId = undefined;
      }
      const [course, units, members, permissions, progress, summary, schedule] = await reads;
      if (this.#closed || reading !== this.#readings) return;
      void this.workspace.learnUsers(members.map(memberId), { members: true });
      this.#takeCourse(course);
      this.permissions = permissions;
      this.units = units;
      this.#writes.remember(this.units);
      if (membersTold === this.#people.told) this.members = members;
      this.progress.take(progress, summary, progressMark);
      this.schedule.take(schedule, scheduleMark);
      if (this.#speculativeUnitId) {
        const node = this.unit(this.#speculativeUnitId);
        if (node?.kind === 'chapter') this.clearSpeculativeUnit(this.#speculativeUnitId);
        this.#speculativeUnitId = undefined;
      }
      this.status = 'ready';
      // Both, not the tab shown alone: a tab of meetings or exams shows once it holds one.
      void this.events.refreshAll();
    } catch (error) {
      if (this.#closed || reading !== this.#readings) return;
      this.#speculativeUnitId = undefined;
      this.openUnit?.close();
      this.openUnit = undefined;
      if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
        this.workspace.evictContainer(this.courseId);
        this.missing = true;
      } else this.workspace.report('load', error);
      this.status = 'failed';
    }
  }

  async join(): Promise<boolean> {
    if (this.joining) return false;
    this.joining = true;
    try {
      await this.workspace.backend.joinContainer(this.courseId);
    } catch (error) {
      if (isBackendError(error) && error.status === 404) {
        this.openUnit?.close();
        this.openUnit = undefined;
        this.workspace.evictContainer(this.courseId);
        [this.missing, this.status] = [true, 'failed'];
      } else this.workspace.report('save', error);
      return false;
    } finally {
      this.joining = false;
    }
    if (this.#closed) return false;
    await this.load({ activeTab: this.#activeTab, unitId: this.openUnit?.unitId });
    return this.status === 'ready';
  }

  settleBody(): Promise<void> {
    return this.openUnit?.settleBody() ?? Promise.resolve();
  }

  close(): void {
    this.#closed = true;
    this.#stopWatching?.();
    this.#stopWatching = undefined;
    this.openUnit?.close();
    this.openUnit = undefined;
  }

  showUnit(
    unitId: string | undefined,
    options: { speculative?: boolean } = {},
  ): UnitStore | undefined {
    if (!options.speculative) this.#speculativeUnitId = undefined;
    const open = this.openUnit;
    if (open && open.unitId === unitId && !open.missing) return open;
    this.openUnit?.close();
    this.openUnit = unitId === undefined ? undefined : new UnitStore(this, unitId, options);
    void this.openUnit?.load();
    return this.openUnit;
  }

  #takeCourse(course: Container): void {
    if (!this.course || this.course.updatedAt <= course.updatedAt) this.course = course;
  }

  receiveUnit(unit: CourseUnit): void {
    if (this.#writes.receive(unit)) this.#takeUnit(unit);
  }

  #takeUnit(unit: CourseUnit): void {
    if (unit.courseId !== this.courseId || !this.#writes.admits(unit)) return;
    const isNew = unit.kind === 'unit' && !this.units.some((existing) => existing.id === unit.id);
    this.units = take(this.units, unit);
    if (isNew && this.canTrackProgress) void this.progress.refreshMine();
    const shown = this.openUnit;
    if (shown?.unitId !== unit.id) return;
    if (shown.missing) void shown.load();
    else shown.unitChanged(unit);
  }

  createUnit(draft: UnitDraft): Promise<CourseUnitDetail | undefined> {
    return this.#writes.create(draft, 'createUnit');
  }

  moving = $state(0);
  readonly settling = $derived.by(() => this.moving > 0 || this.workspace.undoBusy);

  async moveUnit(unitId: string, to: PagePlace): Promise<boolean> {
    this.moving += 1;
    try {
      return await this.#writes.move(unitId, to, 'moveUnit');
    } finally {
      this.moving -= 1;
    }
  }

  deleteUnit(unitId: string): Promise<boolean> {
    return this.#writes.remove(unitId, 'deleteUnit');
  }

  async deletedUnits(paging?: Paging): Promise<DeletedItem[]> {
    await this.#writes.deletionsAnswered();
    return this.#courses.deletedUnits(this.courseId, paging);
  }

  restoreUnit(unitId: string): Promise<void> {
    return this.#writes.restore(unitId);
  }

  changeUnitTitle(unitId: string, title: string): Promise<void> {
    return this.#writes.changeTitle(unitId, title, 'changeUnitTitle');
  }

  changeCourse(
    looks: CourseLooks,
    step: 'changeCourse' | 'changeVisibility' | 'changeMinimumRole' = 'changeCourse',
  ): Promise<void> {
    return changeCourseContainer(
      this,
      (course) => {
        this.#takeCourse(course);
      },
      looks,
      step,
    );
  }

  get containerId(): string {
    return this.courseId;
  }
  get container(): Container | undefined {
    return this.course;
  }
  set container(course: Container | undefined) {
    this.course = course;
  }

  setOpen(open: boolean): Promise<void> {
    return this.changeCourse({ visibility: open ? 'open' : 'private' }, 'changeVisibility');
  }
  setMinimumRole(role: ContainerRole): Promise<void> {
    return this.changeCourse({ minimumRole: role }, 'changeMinimumRole');
  }
  addMember(userId: string, role: ContainerRole): Promise<void> {
    return this.#people.add(userId, role);
  }
  setMemberRole(userId: string, role: ContainerRole): Promise<void> {
    return this.#people.setRole(userId, role);
  }
  removeMember(userId: string): Promise<void> {
    return this.#people.remove(userId);
  }

  #syncUnitDates(scheduled: readonly ScheduledUnit[]): void {
    const byId: Record<string, ScheduledUnit | undefined> = Object.fromEntries(
      scheduled.map((item) => [item.unitId, item]),
    );
    this.units = this.units.map((unit) => {
      const next = byId[unit.id];
      return next ? { ...unit, scheduledDate: next.scheduledDate } : unit;
    });
  }

  get myProgress(): ReadonlyMap<string, UnitProgress> {
    return this.progress.myProgress;
  }
  get progressSummary(): CourseProgressSummary | null {
    return this.progress.progressSummary;
  }
  toggleUnitComplete(unitId: string): Promise<boolean> {
    return this.progress.toggleComplete(unitId);
  }
  setUnitLastPosition(unitId: string, position: string | null): Promise<boolean> {
    return this.progress.setLastPosition(unitId, position);
  }
  refreshProgressSummary(): Promise<void> {
    return this.progress.refreshSummary();
  }
}
