/**
 * Reactive state for course study meetings, shared attendance, course exams,
 * and strictly personal exam scores on one open course (`CourseStore`).
 *
 * Personal exam score writes (`putMyScore` / `deleteMyScore`) never broadcast
 * over WebSocket, so after each personal score write we immediately re-fetch
 * `listCourseExams` so the caller's updated `averageScore` and `scoreCount`
 * appear right away.
 */
import { SvelteSet } from 'svelte/reactivity';

import {
  compareCourseExams,
  compareCourseMeetings,
  type AttendanceStatus,
  type CourseExam,
  type CourseExamPatch,
  type CourseMeeting,
  type CourseMeetingPatch,
  type Courses,
  type MyExamScore,
  type MyExamScoreUpdate,
  type NewCourseExam,
  type NewCourseMeeting,
} from './course-types.ts';
import { isBackendError, type BackendError } from './problems.ts';
import type { Workspace } from './workspace.svelte.ts';

export interface CourseMeetingsHost {
  readonly courseId: string;
  readonly workspace: Workspace;
  courses(): Courses;
  canEdit(): boolean;
  /** Who manages the course's roles, as the server asks of a publication of results. */
  canManageRoles(): boolean;
  canTrack(): boolean;
  isClosed(): boolean;
}

export interface CourseEventWriteResult<T = undefined> {
  ok: boolean;
  value?: T;
  error?: BackendError;
}

function collectMeetingUserIds(meetings: readonly CourseMeeting[]): string[] {
  const ids: string[] = [];
  for (const meeting of meetings) {
    if (meeting.presenterId) ids.push(meeting.presenterId);
    if (meeting.hostId) ids.push(meeting.hostId);
    for (const row of meeting.attendance) ids.push(row.userId);
  }
  return ids;
}

export class CourseMeetingsState {
  readonly #host: CourseMeetingsHost;

  meetings = $state.raw<CourseMeeting[]>([]);
  exams = $state.raw<CourseExam[]>([]);
  meetingsLoaded = $state(false);
  examsLoaded = $state(false);
  meetingsFailed = $state(false);
  examsFailed = $state(false);
  busy = $state(false);
  readonly #busyAttendance = new SvelteSet<string>();
  readonly #busyExamScores = new SvelteSet<string>();

  #meetingsVersion = 0;
  #meetingsReads = 0;
  #meetingsInFlight: Promise<void> | undefined;
  #examsVersion = 0;
  #examsReads = 0;
  #examsInFlight: Promise<void> | undefined;

  constructor(host: CourseMeetingsHost) {
    this.#host = host;
  }

  get workspaceId(): string | undefined {
    return this.#host.workspace.info?.id;
  }

  isAttendanceBusy(meetingId: string, userId: string): boolean {
    return this.#busyAttendance.has(`${meetingId}:${userId}`);
  }

  isExamScoreBusy(examId: string): boolean {
    return this.#busyExamScores.has(examId);
  }

  async refreshAll(): Promise<void> {
    await Promise.all([this.refreshMeetings(), this.refreshExams()]);
  }

  refreshMeetings(): Promise<void> {
    if (this.#host.isClosed()) return Promise.resolve();
    if (this.#meetingsInFlight) return this.#meetingsInFlight;
    const version = this.#meetingsVersion;
    this.#meetingsReads += 1;
    const reading = this.#meetingsReads;
    const run = this.#host
      .courses()
      .meetings.list(this.#host.courseId, this.workspaceId)
      .then((list) => {
        if (
          this.#host.isClosed() ||
          reading !== this.#meetingsReads ||
          version !== this.#meetingsVersion
        ) {
          return;
        }
        this.meetings = [...list].sort(compareCourseMeetings);
        this.meetingsLoaded = true;
        this.meetingsFailed = false;
        void this.#host.workspace.learnUsers(collectMeetingUserIds(this.meetings));
      })
      .catch(() => {
        if (!this.#host.isClosed() && !this.meetingsLoaded) this.meetingsFailed = true;
      })
      .finally(() => {
        if (this.#meetingsInFlight === run) this.#meetingsInFlight = undefined;
      });
    this.#meetingsInFlight = run;
    return run;
  }

  refreshExams(): Promise<void> {
    if (this.#host.isClosed()) return Promise.resolve();
    if (this.#examsInFlight) return this.#examsInFlight;
    const version = this.#examsVersion;
    this.#examsReads += 1;
    const reading = this.#examsReads;
    const run = this.#host
      .courses()
      .exams.list(this.#host.courseId, this.workspaceId)
      .then((list) => {
        if (
          this.#host.isClosed() ||
          reading !== this.#examsReads ||
          version !== this.#examsVersion
        ) {
          return;
        }
        this.exams = [...list].sort(compareCourseExams);
        this.examsLoaded = true;
        this.examsFailed = false;
      })
      .catch(() => {
        if (!this.#host.isClosed() && !this.examsLoaded) this.examsFailed = true;
      })
      .finally(() => {
        if (this.#examsInFlight === run) this.#examsInFlight = undefined;
      });
    this.#examsInFlight = run;
    return run;
  }

  clearMine(): void {
    this.#examsVersion += 1;
    this.#examsReads += 1;
    this.exams = this.exams.map((exam) => ({ ...exam, myScore: null }));
    void this.refreshExams();
  }

  async createMeeting(draft: NewCourseMeeting): Promise<CourseEventWriteResult<CourseMeeting>> {
    if (!this.#host.canEdit() || this.busy) return { ok: false };
    this.busy = true;
    try {
      const created = await this.#host
        .courses()
        .meetings.create(this.#host.courseId, draft, this.workspaceId);
      if (this.#host.isClosed()) return { ok: false };
      this.#meetingsVersion += 1;
      this.#meetingsReads += 1;
      this.meetings = [...this.meetings, created].sort(compareCourseMeetings);
      this.meetingsLoaded = true;
      void this.#host.workspace.learnUsers(collectMeetingUserIds([created]));
      return { ok: true, value: created };
    } catch (error) {
      if (isBackendError(error) && (error.status === 400 || error.status === 404)) {
        return { ok: false, error };
      }
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.busy = false;
    }
  }

  async patchMeeting(
    meetingId: string,
    patch: CourseMeetingPatch,
  ): Promise<CourseEventWriteResult<CourseMeeting>> {
    if (!this.#host.canEdit() || this.busy) return { ok: false };
    this.busy = true;
    try {
      const updated = await this.#host
        .courses()
        .meetings.patch(this.#host.courseId, meetingId, patch, this.workspaceId);
      if (this.#host.isClosed()) return { ok: false };
      this.#meetingsVersion += 1;
      this.#meetingsReads += 1;
      this.meetings = this.meetings
        .map((item) => (item.id === meetingId ? updated : item))
        .sort(compareCourseMeetings);
      void this.#host.workspace.learnUsers(collectMeetingUserIds([updated]));
      return { ok: true, value: updated };
    } catch (error) {
      if (isBackendError(error) && (error.status === 400 || error.status === 404)) {
        return { ok: false, error };
      }
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.busy = false;
    }
  }

  async deleteMeeting(meetingId: string): Promise<boolean> {
    if (!this.#host.canEdit() || this.busy) return false;
    this.busy = true;
    try {
      await this.#host.courses().meetings.delete(this.#host.courseId, meetingId, this.workspaceId);
      if (this.#host.isClosed()) return false;
      this.#meetingsVersion += 1;
      this.#meetingsReads += 1;
      this.meetings = this.meetings.filter((item) => item.id !== meetingId);
      return true;
    } catch (error) {
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return false;
    } finally {
      this.busy = false;
    }
  }

  async setAttendance(
    meetingId: string,
    userId: string,
    status: AttendanceStatus,
  ): Promise<boolean> {
    const meId = this.#host.workspace.me?.id;
    const allowed = this.#host.canTrack() && (this.#host.canEdit() || userId === meId);
    const key = `${meetingId}:${userId}`;
    if (!allowed || this.#busyAttendance.has(key)) return false;
    this.#busyAttendance.add(key);
    try {
      const saved = await this.#host
        .courses()
        .meetings.putAttendance(
          this.#host.courseId,
          meetingId,
          userId,
          { status },
          this.workspaceId,
        );
      if (this.#host.isClosed()) return false;
      this.#meetingsVersion += 1;
      this.#meetingsReads += 1;
      this.meetings = this.meetings.map((meeting) => {
        if (meeting.id !== meetingId) return meeting;
        const without = meeting.attendance.filter((row) => row.userId !== userId);
        const attendance = [...without, saved].sort((a, b) =>
          a.userId < b.userId ? -1 : a.userId > b.userId ? 1 : 0,
        );
        return { ...meeting, attendance };
      });
      return true;
    } catch (error) {
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return false;
    } finally {
      this.#busyAttendance.delete(key);
    }
  }

  async createExam(draft: NewCourseExam): Promise<CourseEventWriteResult<CourseExam>> {
    if (!this.#host.canEdit() || this.busy) return { ok: false };
    this.busy = true;
    try {
      const created = await this.#host
        .courses()
        .exams.create(this.#host.courseId, draft, this.workspaceId);
      if (this.#host.isClosed()) return { ok: false };
      this.#examsVersion += 1;
      this.#examsReads += 1;
      this.exams = [...this.exams, created].sort(compareCourseExams);
      this.examsLoaded = true;
      return { ok: true, value: created };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.busy = false;
    }
  }

  async patchExam(
    examId: string,
    patch: CourseExamPatch,
  ): Promise<CourseEventWriteResult<CourseExam>> {
    if (!this.#host.canEdit() || this.busy) return { ok: false };
    this.busy = true;
    try {
      const updated = await this.#host
        .courses()
        .exams.patch(this.#host.courseId, examId, patch, this.workspaceId);
      if (this.#host.isClosed()) return { ok: false };
      this.#examsVersion += 1;
      this.#examsReads += 1;
      this.exams = this.exams
        .map((item) => (item.id === examId ? updated : item))
        .sort(compareCourseExams);
      return { ok: true, value: updated };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.busy = false;
    }
  }

  /**
   * Publishes the snapshot of an exam's results, or takes it back: for who
   * manages the course's roles (`ManageRoles`, as the server asks and as the
   * button is shown), whether or not they edit it. Refused (409: too few scores, or too few new ones since
   * the last), it says so (`refused`), and the exam stays as it was; any other
   * failure is reported as a write's.
   */
  async setResultsPublished(
    examId: string,
    published: boolean,
  ): Promise<'done' | 'refused' | 'failed'> {
    if (!this.#host.canManageRoles() || this.busy) return 'failed';
    this.busy = true;
    try {
      const updated = await this.#host
        .courses()
        .exams.setResultsPublished(this.#host.courseId, examId, published, this.workspaceId);
      if (this.#host.isClosed()) return 'failed';
      this.#examsVersion += 1;
      this.#examsReads += 1;
      this.exams = this.exams.map((item) => (item.id === examId ? updated : item));
      return 'done';
    } catch (error) {
      if (isBackendError(error) && error.status === 409) return 'refused';
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return 'failed';
    } finally {
      this.busy = false;
    }
  }

  async deleteExam(examId: string): Promise<boolean> {
    if (!this.#host.canEdit() || this.busy) return false;
    this.busy = true;
    try {
      await this.#host.courses().exams.delete(this.#host.courseId, examId, this.workspaceId);
      if (this.#host.isClosed()) return false;
      this.#examsVersion += 1;
      this.#examsReads += 1;
      this.exams = this.exams.filter((item) => item.id !== examId);
      return true;
    } catch (error) {
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return false;
    } finally {
      this.busy = false;
    }
  }

  async saveMyScore(
    examId: string,
    update: MyExamScoreUpdate,
  ): Promise<CourseEventWriteResult<MyExamScore>> {
    if (!this.#host.canTrack() || this.#busyExamScores.has(examId)) return { ok: false };
    this.#busyExamScores.add(examId);
    try {
      const saved = await this.#host
        .courses()
        .exams.putMyScore(this.#host.courseId, examId, update, this.workspaceId);
      if (this.#host.isClosed()) return { ok: false };
      this.#examsVersion += 1;
      this.#examsReads += 1;
      this.exams = this.exams.map((exam) =>
        exam.id === examId ? { ...exam, myScore: saved } : exam,
      );
      await this.refreshExams();
      return { ok: true, value: saved };
    } catch (error) {
      if (isBackendError(error) && error.status === 400) return { ok: false, error };
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return { ok: false };
    } finally {
      this.#busyExamScores.delete(examId);
    }
  }

  async deleteMyScore(examId: string): Promise<boolean> {
    if (!this.#host.canTrack() || this.#busyExamScores.has(examId)) return false;
    this.#busyExamScores.add(examId);
    try {
      await this.#host.courses().exams.deleteMyScore(this.#host.courseId, examId, this.workspaceId);
      if (this.#host.isClosed()) return false;
      this.#examsVersion += 1;
      this.#examsReads += 1;
      this.exams = this.exams.map((exam) =>
        exam.id === examId ? { ...exam, myScore: null } : exam,
      );
      await this.refreshExams();
      return true;
    } catch (error) {
      if (!this.#host.isClosed()) this.#host.workspace.report('save', error);
      return false;
    } finally {
      this.#busyExamScores.delete(examId);
    }
  }
}
