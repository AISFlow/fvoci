/**
 * Course study schedules, proportional distribution, manual shared & personal
 * redistribution, personal planned-date overrides, and workspace "today's
 * learning" tests across all time zones (`tools/test-zones.ts`).
 */
import { describe, expect, it } from 'vitest';

import {
  deleteCourseSchedule,
  getCourseSchedule,
  getWorkspaceLearningToday,
  putCourseSchedule,
  putUnitSchedule,
  redistributeCourseSchedule,
  redistributeMyCourseSchedule,
} from '$lib/api/course-schedules.ts';
import type { WikiRequest } from '$lib/api/wikis.ts';
import {
  distributeUnits,
  outlineUnits,
  studyDatesInRange,
  todayInTimeZone,
  validateScheduleWindow,
} from '$lib/logic/course-schedule.ts';
import { parseCalendarDate, type CalendarDate } from '$lib/logic/dates.ts';
import {
  BackendError,
  CourseStore,
  LearningTodayStore,
  STUDY_DAYS_ALL,
  STUDY_DAYS_FRIDAY,
  STUDY_DAYS_MONDAY,
  STUDY_DAYS_SATURDAY,
  STUDY_DAYS_SUNDAY,
  STUDY_DAYS_WEEKDAYS,
  Workspace,
} from '$lib/state/index.ts';

import { settle, setup } from './support.ts';

const day = (iso: string): CalendarDate => {
  const parsed = parseCalendarDate(iso);
  if (!parsed) throw new Error(`bad test date: ${iso}`);
  return parsed;
};

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('expected call to reject');
};

describe('pure course schedule logic', () => {
  it('distributes units proportionally by amount (null = 1) and respects studyDays bitmask', () => {
    // 2026-05-04 is Monday .. 2026-05-08 is Friday (5 weekdays).
    const days = studyDatesInRange(day('2026-05-04'), day('2026-05-10'), STUDY_DAYS_WEEKDAYS);
    expect(days).toEqual(['2026-05-04', '2026-05-05', '2026-05-06', '2026-05-07', '2026-05-08']);

    // Weights [30, 10, 10] across 5 days -> u1 occupies first 3 days (starts day 0), u2 day 3, u3 day 4.
    const assigned = distributeUnits(
      [
        { unitId: 'u1', amount: 30 },
        { unitId: 'u2', amount: 10 },
        { unitId: 'u3', amount: 10 },
      ],
      day('2026-05-04'),
      day('2026-05-08'),
      STUDY_DAYS_WEEKDAYS,
    );
    expect(assigned).toEqual([
      ['u1', '2026-05-04'],
      ['u2', '2026-05-07'],
      ['u3', '2026-05-08'],
    ]);

    // Null amounts default to weight 1.
    const equal = distributeUnits(
      [
        { unitId: 'a', amount: null },
        { unitId: 'b', amount: null },
      ],
      day('2026-05-04'),
      day('2026-05-08'),
      STUDY_DAYS_MONDAY | STUDY_DAYS_FRIDAY,
    );
    expect(equal).toEqual([
      ['a', '2026-05-04'],
      ['b', '2026-05-08'],
    ]);
  });

  it('resolves caller today in IANA time zones and uses UTC when auto', () => {
    // 2026-05-04T16:30:00Z -> 2026-05-04 in UTC, 2026-05-05 in Asia/Seoul (UTC+9), 2026-05-04 in America/Los_Angeles.
    const instant = Date.UTC(2026, 4, 4, 16, 30, 0);
    expect(todayInTimeZone(instant, 'auto')).toBe('2026-05-04');
    expect(todayInTimeZone(instant, 'UTC')).toBe('2026-05-04');
    expect(todayInTimeZone(instant, 'Asia/Seoul')).toBe('2026-05-05');
    expect(todayInTimeZone(instant, 'America/Los_Angeles')).toBe('2026-05-04');
  });

  it('walks outline units in tree order omitting chapters, and validates schedule windows', () => {
    const ordered = outlineUnits([
      { id: 'u-top', kind: 'unit', parentId: '', order: 'b' },
      { id: 'ch1', kind: 'chapter', parentId: '', order: 'a' },
      { id: 'u2', kind: 'unit', parentId: 'ch1', order: 'b' },
      { id: 'u1', kind: 'unit', parentId: 'ch1', order: 'a' },
    ]);
    expect(ordered.map((u) => u.id)).toEqual(['u1', 'u2', 'u-top']);

    // Weekend-only mask inside Mon..Fri range reports no_study_days.
    expect(
      validateScheduleWindow('2026-05-04', '2026-05-08', STUDY_DAYS_SATURDAY | STUDY_DAYS_SUNDAY),
    ).toEqual([{ field: 'studyDays', reason: 'no_study_days' }]);
    expect(validateScheduleWindow('2026-05-10', '2026-05-04', STUDY_DAYS_ALL)).toEqual([
      { field: 'targetDate', reason: 'before_start' },
    ]);
  });
});

describe('course schedule API client', () => {
  it('calls all 7 routes and always serializes plannedDate (including null)', async () => {
    const calls: {
      path: string;
      params: Record<string, string>;
      options?: unknown;
      ws?: string;
    }[] = [];
    const emptyView = {
      schedule: null,
      today: '2026-05-05',
      todayCount: 0,
      overdueCount: 0,
      units: [],
    };
    const request: WikiRequest = <T>(
      path: string,
      params: Record<string, string>,
      options?: unknown,
      workspaceId?: string,
    ): Promise<T> => {
      calls.push({ path, params, options, ws: workspaceId });
      if (path === '/learning/today') {
        return Promise.resolve({ today: '2026-05-05', todayUnits: [], overdueUnits: [] } as T);
      }
      if (path === '/courses/{courseId}/units/{unitId}/schedule') {
        return Promise.resolve({
          unitId: 'u1',
          completedAt: null,
          plannedDate: null,
          lastPosition: null,
          updatedAt: 1,
        } as T);
      }
      return Promise.resolve(emptyView as T);
    };

    await getCourseSchedule(request, 'ws1', 'c1');
    await putCourseSchedule(request, 'ws1', 'c1', {
      startDate: '2026-05-01',
      targetDate: '2026-05-10',
      studyDays: 31,
      autoAssign: true,
    });
    await deleteCourseSchedule(request, 'ws1', 'c1');
    await redistributeCourseSchedule(request, 'ws1', 'c1', { fromDate: '2026-05-05' });
    await redistributeMyCourseSchedule(request, 'ws1', 'c1', { targetDate: '2026-05-12' });
    await putUnitSchedule(request, 'ws1', 'c1', 'u1', { plannedDate: null });
    await getWorkspaceLearningToday(request, 'ws1');

    expect(calls.map((c) => c.path)).toEqual([
      '/courses/{courseId}/schedule',
      '/courses/{courseId}/schedule',
      '/courses/{courseId}/schedule',
      '/courses/{courseId}/schedule/redistribute',
      '/courses/{courseId}/my-schedule/redistribute',
      '/courses/{courseId}/units/{unitId}/schedule',
      '/learning/today',
    ]);
    expect(calls[5]?.options).toEqual({ method: 'PUT', json: { plannedDate: null } });
  });
});

describe('in-memory course schedule contract & store integration', () => {
  it('enforces permissions, no auto-shift, personal isolation, chapter 400, and learningToday aggregation', async () => {
    const made = setup();
    // Seeded world timestamps reach Oct 2026, so start our monotonic clock in Nov 2026.
    let clock = Date.UTC(2026, 10, 4, 12, 0, 0); // 2026-11-04T12:00:00Z (Wednesday)
    made.world.useClock(() => clock);
    await made.workspace.load();

    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'Algorithms',
      visibility: 'open',
    });
    const ch = await made.backend.course.createUnit(course.id, { kind: 'chapter', title: 'Ch 1' });
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Graphs',
      parentId: ch.id,
      amount: 30,
      amountUnit: 'minutes',
    });
    const u2 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Trees',
      parentId: ch.id,
      amount: 30,
      amountUnit: 'minutes',
    });

    // Chapters reject amount/amountUnit/scheduledDate even when null.
    expect(
      (await rejection(made.backend.course.patchUnit(course.id, ch.id, { amount: null }))).status,
    ).toBe(400);
    expect(
      (await rejection(made.backend.course.patchUnit(course.id, ch.id, { scheduledDate: null })))
        .status,
    ).toBe(400);

    // Save schedule with autoAssign starting in the past (Mon Nov 2 .. Wed Nov 4) -> u1 is overdue, u2 is today.
    const view = await made.backend.course.putSchedule(course.id, {
      startDate: '2026-11-02',
      targetDate: '2026-11-04',
      studyDays: STUDY_DAYS_ALL,
      autoAssign: true,
    });
    expect(view.today).toBe('2026-11-04');
    expect(view.overdueCount).toBe(1);
    expect(view.todayCount).toBe(1);
    expect(view.units[0]?.overdue).toBe(true);
    expect(view.units[0]?.scheduledDate).toBe('2026-11-02');
    expect(view.units[1]?.scheduledDate).toBe('2026-11-04');

    // Time advances by 2 days (Friday Nov 6): overdue units NEVER auto-shift!
    clock = Date.UTC(2026, 10, 6, 12, 0, 0);
    const later = await made.backend.course.schedule(course.id);
    expect(later.today).toBe('2026-11-06');
    expect(later.overdueCount).toBe(2);
    expect(later.todayCount).toBe(0);
    expect(later.units.map((u) => u.effectiveDate)).toEqual(['2026-11-02', '2026-11-04']);

    // Unjoined open-course reader can view shared schedule (myPlannedDate: null), gets 403 on personal writes,
    // and sees nothing in learningToday until joining.
    const unjoinedView = await made.other.course.schedule(course.id);
    expect(unjoinedView.units[0]?.myPlannedDate).toBeNull();
    expect(
      (
        await rejection(
          made.other.course.putUnitSchedule(course.id, u1.id, { plannedDate: '2026-11-06' }),
        )
      ).status,
    ).toBe(403);
    expect((await made.other.course.learningToday(wsId)).overdueUnits).toEqual([]);

    // Add other user as Viewer: can redistribute own incomplete units & set personal plannedDate,
    // isolated from admin, and cannot modify shared schedule (403).
    await made.backend.addMember(course.id, made.other.userId, 'viewer');
    expect(
      (
        await rejection(
          made.other.course.putSchedule(course.id, {
            startDate: '2026-11-06',
            targetDate: '2026-11-10',
            studyDays: STUDY_DAYS_ALL,
          }),
        )
      ).status,
    ).toBe(403);
    expect((await rejection(made.other.course.redistributeSchedule(course.id, {}))).status).toBe(
      403,
    );

    // Viewer completes u1, then redistributes ONLY their remaining incomplete unit (u2) to Nov 6..8.
    const events: unknown[] = [];
    const stop = made.backend.watchContainer(course.id, (change) => events.push(change));
    await made.other.course.putUnitProgress(course.id, u1.id, { completed: true });
    const myRedist = await made.other.course.redistributeMySchedule(course.id, {
      fromDate: '2026-11-06',
      targetDate: '2026-11-08',
      studyDays: STUDY_DAYS_ALL,
    });
    await settle();
    expect(events).toEqual([]); // Zero WS events on personal schedule writes!
    stop();

    expect(myRedist.units[0]?.completed).toBe(true);
    expect(myRedist.units[0]?.myPlannedDate).toBeNull(); // Completed unit untouched!
    expect(myRedist.units[1]?.myPlannedDate).toBe('2026-11-06');
    expect(myRedist.units[1]?.scheduledDate).toBe('2026-11-04'); // Shared date untouched!

    // Admin never sees viewer's personal plannedDate.
    const adminCheck = await made.backend.course.schedule(course.id);
    expect(adminCheck.units[1]?.myPlannedDate).toBeNull();
    expect(adminCheck.units[1]?.effectiveDate).toBe('2026-11-04');

    // CourseStore + LearningTodayStore integration.
    const store = new CourseStore(made.workspace, course.id);
    await store.load();
    expect(store.schedule.overdueCount).toBe(2);

    // Completing u1 immediately reconciles schedule overdueCount from 2 -> 1.
    expect(await store.toggleUnitComplete(u1.id)).toBe(true);
    expect(store.schedule.overdueCount).toBe(1);

    // LearningTodayStore shows u2 as overdue for admin, and completes it right from the feed.
    const todayStore = new LearningTodayStore(made.workspace);
    await todayStore.load(wsId);
    expect(todayStore.today).toBe('2026-11-06');
    expect(todayStore.overdueUnits.map((i) => i.unitId)).toEqual([u2.id]);
    expect(todayStore.todayUnits).toEqual([]);

    const item = todayStore.overdueUnits[0];
    if (!item) throw new Error('expected overdue item');
    expect(await todayStore.completeUnit(item, wsId)).toBe(true);
    expect(todayStore.overdueUnits).toEqual([]);

    // Viewer's own learningToday shows u2 due today (Nov 6) and nothing overdue.
    const otherWorkspace = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWorkspace.load();
    const otherToday = new LearningTodayStore(otherWorkspace);
    await otherToday.load(wsId);
    expect(otherToday.overdueUnits).toEqual([]);
    expect(otherToday.todayUnits.map((i) => i.unitId)).toEqual([u2.id]);

    otherToday.close();
    todayStore.close();
    store.close();
  });

  it('excludes template courses from workspace learningToday', async () => {
    const made = setup();
    const wsId = made.world.workspace.id;
    made.world.useClock(() => Date.UTC(2026, 10, 10, 12, 0, 0));
    await made.workspace.load();

    const live = await made.backend.course.createCourse(wsId, {
      title: 'Live Course',
      visibility: 'private',
    });
    await made.backend.course.createUnit(live.id, {
      kind: 'unit',
      title: 'Live Unit',
      scheduledDate: '2026-11-10',
    });

    // Duplicate as a template course — its units must never leak into learningToday.
    const tpl = await made.backend.duplicateContainer(live.id, true, wsId);
    expect(tpl.isTemplate).toBe(true);

    const feed = await made.backend.course.learningToday(wsId);
    expect(feed.todayUnits.map((item) => item.courseId)).toEqual([live.id]);
    expect(feed.overdueUnits).toEqual([]);
  });
});
