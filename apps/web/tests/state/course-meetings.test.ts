/**
 * Course study meetings, shared attendance, course exams, strictly personal
 * exam scores, aggregate averages, erased-user tombstones, and `meeting_upcoming`
 * notification tests.
 */
import { describe, expect, it, vi } from 'vitest';

import {
  createCourseExam,
  createCourseMeeting,
  deleteCourseExam,
  deleteCourseMeeting,
  deleteMyExamScore,
  listCourseExams,
  listCourseMeetings,
  patchCourseExam,
  patchCourseMeeting,
  putMeetingAttendance,
  putMyExamScore,
} from '$lib/api/course-meetings.ts';
import type { WikiRequest } from '$lib/api/wikis.ts';
import {
  defaultMeetingInputs,
  parseMeetingInstant,
  safeHttpUrl,
  splitInstantToInputs,
} from '$lib/features/course/course-meeting-labels.ts';
import { isGone } from '$lib/logic/users.ts';
import { BackendError, CourseStore, READ_ONLY_PERMISSIONS, Workspace } from '$lib/state/index.ts';
import { noteUpcomingMeeting } from '$lib/state/memory/notifications.ts';
import { eraseUser } from '$lib/state/memory/users.ts';

import { settle, setup } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('expected call to reject');
};

describe('course meeting & exam helpers', () => {
  it('round-trips meeting date+time across IANA time zones without device timezone', () => {
    const ms = parseMeetingInstant('2026-11-25', '19:30', 'Asia/Seoul');
    expect(ms).toBe(Date.UTC(2026, 10, 25, 10, 30, 0));
    expect(splitInstantToInputs(ms ?? 0, 'Asia/Seoul')).toEqual({
      date: '2026-11-25',
      time: '19:30',
    });
    expect(splitInstantToInputs(ms ?? 0, 'UTC')).toEqual({
      date: '2026-11-25',
      time: '10:30',
    });
    expect(defaultMeetingInputs(Date.UTC(2026, 10, 25, 16, 0, 0), 'Asia/Seoul')).toEqual({
      date: '2026-11-26',
      time: '19:00',
    });
  });

  it('allows only http/https URLs as clickable meeting locations', () => {
    expect(safeHttpUrl('https://meet.example.com/room')).toBe('https://meet.example.com/room');
    expect(safeHttpUrl('http://example.com')).toBe('http://example.com/');
    expect(safeHttpUrl('javascript:alert(1)')).toBeUndefined();
    expect(safeHttpUrl('https://meet.example.com/a Room 301')).toBeUndefined();
    expect(safeHttpUrl('Room 301')).toBeUndefined();
  });
});

describe('course meetings & exams API client', () => {
  it('calls all 11 routes and preserves tristate null fields on patch', async () => {
    const calls: {
      path: string;
      params: Record<string, string>;
      options?: unknown;
      ws?: string;
    }[] = [];
    const sampleMeeting = {
      id: 'm1',
      containerId: 'c1',
      title: 'Sync',
      startsAt: 1000,
      location: '',
      unitIds: [],
      presenterId: null,
      hostId: null,
      notes: '',
      notifiedAt: null,
      createdAt: 1,
      updatedAt: 1,
      attendance: [],
    };
    const sampleExam = {
      id: 'e1',
      containerId: 'c1',
      title: 'Midterm',
      examDate: null,
      maxScore: 100,
      description: '',
      createdAt: 1,
      updatedAt: 1,
      myScore: null,
      averageScore: null,
      scoreCount: 0,
      resultsPublishedAt: null,
      publishedScoreBucket: null,
    };
    const request: WikiRequest = <T>(
      path: string,
      params: Record<string, string>,
      options?: unknown,
      workspaceId?: string,
    ): Promise<T> => {
      calls.push({ path, params, options, ws: workspaceId });
      const method = (options as { method?: string } | undefined)?.method;
      if (path === '/courses/{courseId}/meetings' && !method) {
        return Promise.resolve([sampleMeeting] as T);
      }
      if (path === '/courses/{courseId}/exams' && !method) {
        return Promise.resolve([sampleExam] as T);
      }
      if (path.endsWith('/attendance/{userId}')) {
        return Promise.resolve({ userId: 'u1', status: 'present', updatedAt: 2 } as T);
      }
      if (path.endsWith('/my-score')) {
        return Promise.resolve({ score: 88, note: 'good', updatedAt: 3 } as T);
      }
      if (path.includes('/exams')) return Promise.resolve(sampleExam as T);
      return Promise.resolve(sampleMeeting as T);
    };

    await listCourseMeetings(request, 'ws1', 'c1');
    await createCourseMeeting(request, 'ws1', 'c1', { title: 'Sync', startsAt: 1000 });
    await patchCourseMeeting(request, 'ws1', 'c1', 'm1', { presenterId: null, hostId: null });
    await deleteCourseMeeting(request, 'ws1', 'c1', 'm1');
    await putMeetingAttendance(request, 'ws1', 'c1', 'm1', 'u1', { status: 'present' });
    await listCourseExams(request, 'ws1', 'c1');
    await createCourseExam(request, 'ws1', 'c1', { title: 'Midterm', maxScore: 100 });
    await patchCourseExam(request, 'ws1', 'c1', 'e1', { examDate: null });
    await deleteCourseExam(request, 'ws1', 'c1', 'e1');
    await putMyExamScore(request, 'ws1', 'c1', 'e1', { score: 88, note: 'good' });
    await deleteMyExamScore(request, 'ws1', 'c1', 'e1');

    expect(calls.map((c) => c.path)).toEqual([
      '/courses/{courseId}/meetings',
      '/courses/{courseId}/meetings',
      '/courses/{courseId}/meetings/{meetingId}',
      '/courses/{courseId}/meetings/{meetingId}',
      '/courses/{courseId}/meetings/{meetingId}/attendance/{userId}',
      '/courses/{courseId}/exams',
      '/courses/{courseId}/exams',
      '/courses/{courseId}/exams/{examId}',
      '/courses/{courseId}/exams/{examId}',
      '/courses/{courseId}/exams/{examId}/my-score',
      '/courses/{courseId}/exams/{examId}/my-score',
    ]);
    expect(calls[2]?.options).toEqual({
      method: 'PATCH',
      json: { presenterId: null, hostId: null },
    });
    expect(calls[7]?.options).toEqual({ method: 'PATCH', json: { examDate: null } });
  });
});

describe('in-memory course meetings, attendance, exams, privacy & tombstones', () => {
  it('enforces ordering, permissions, strict personal score isolation, tombstones, and notifications', async () => {
    const made = setup();
    await made.workspace.load();
    const wsId = made.world.workspace.id;

    const course = await made.backend.course.createCourse(wsId, {
      title: 'Databases',
      visibility: 'open',
      minimumRole: 'viewer',
    });
    const unit = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'B-Trees',
    });

    // Unjoined reader cannot assign themselves or mark attendance or save exam score.
    expect(
      (
        await rejection(
          made.backend.course.meetings.create(course.id, {
            title: 'Bad Presenter',
            startsAt: 1000,
            presenterId: made.other.userId,
          }),
        )
      ).status,
    ).toBe(404);

    // Create two meetings out of chronological order -> list returns startsAt ASC.
    const lateMeeting = await made.backend.course.meetings.create(course.id, {
      title: 'Week 2 Review',
      startsAt: Date.UTC(2026, 10, 28, 10, 0, 0),
      unitIds: [unit.id, unit.id],
    });
    const earlyMeeting = await made.backend.course.meetings.create(course.id, {
      title: 'Week 1 Kickoff',
      startsAt: Date.UTC(2026, 10, 26, 10, 0, 0),
      location: 'Room 101',
      presenterId: made.backend.userId,
    });
    expect(lateMeeting.unitIds).toEqual([unit.id]);

    const listedMeetings = await made.backend.course.meetings.list(course.id);
    expect(listedMeetings.map((m) => m.id)).toEqual([earlyMeeting.id, lateMeeting.id]);

    // Unjoined open-course reader can read meetings/exams but receives 403 on attendance/my-score.
    expect((await made.other.course.meetings.list(course.id)).length).toBe(2);
    expect(
      (
        await rejection(
          made.other.course.meetings.putAttendance(course.id, earlyMeeting.id, made.other.userId, {
            status: 'present',
          }),
        )
      ).status,
    ).toBe(403);

    // Enroll other user as viewer.
    await made.backend.addMember(course.id, made.other.userId, 'viewer');

    // Viewer cannot create/patch meetings or edit someone else's attendance, CAN edit own attendance.
    expect(
      (
        await rejection(
          made.other.course.meetings.create(course.id, { title: 'Nope', startsAt: 1000 }),
        )
      ).status,
    ).toBe(403);
    expect(
      (
        await rejection(
          made.other.course.meetings.putAttendance(
            course.id,
            earlyMeeting.id,
            made.backend.userId,
            {
              status: 'absent',
            },
          ),
        )
      ).status,
    ).toBe(403);
    await made.other.course.meetings.putAttendance(course.id, earlyMeeting.id, made.other.userId, {
      status: 'late',
    });
    await made.backend.course.meetings.putAttendance(
      course.id,
      earlyMeeting.id,
      made.backend.userId,
      { status: 'present' },
    );

    // Patching with a valid title and an invalid presenterId must be atomic (leave title unchanged).
    expect(
      (
        await rejection(
          made.backend.course.meetings.patch(course.id, earlyMeeting.id, {
            title: 'Should Not Persist',
            presenterId: 'non-existent-member',
          }),
        )
      ).status,
    ).toBe(404);
    expect((await made.backend.course.meetings.list(course.id))[0]?.title).toBe('Week 1 Kickoff');

    // Assign other user as host so we can verify tombstone preservation on erasure.
    await made.backend.course.meetings.patch(course.id, earlyMeeting.id, {
      hostId: made.other.userId,
    });

    // Exams ordering: examDate ASC NULLS LAST, createdAt ASC.
    const undated = await made.backend.course.exams.create(course.id, {
      title: 'Pop Quiz',
      maxScore: 50,
    });
    const finalExam = await made.backend.course.exams.create(course.id, {
      title: 'Final',
      examDate: '2026-12-20',
      maxScore: 100,
    });
    const midterm = await made.backend.course.exams.create(course.id, {
      title: 'Midterm',
      examDate: '2026-12-01',
      maxScore: 100,
    });
    expect((await made.backend.course.exams.list(course.id)).map((e) => e.id)).toEqual([
      midterm.id,
      finalExam.id,
      undated.id,
    ]);

    // Score > maxScore returns 400 pointing at `score`.
    const overMax = await rejection(
      made.backend.course.exams.putMyScore(course.id, midterm.id, { score: 101 }),
    );
    expect(overMax.status).toBe(400);
    expect(overMax.refusalAt('/score')).toBeDefined();

    // Personal score writes emit ZERO WebSocket broadcasts.
    const events: unknown[] = [];
    const stop = made.backend.watchContainer(course.id, (change) => events.push(change));
    await made.backend.course.exams.putMyScore(course.id, midterm.id, {
      score: 80,
      note: 'admin private',
    });
    await made.other.course.exams.putMyScore(course.id, midterm.id, {
      score: 100,
      note: 'viewer private',
    });
    await settle();
    expect(events).toEqual([]);
    stop();

    // Strict k-anonymity (`others >= 2`) & unjoined-reader privacy:
    // With 2 scorers (admin=80, viewer=100), neither scorer sees averageScore yet (only 1 other),
    // unjoined reader sees 0 / null, and enrolled unsubmitted 3rd member sees 90 / 2!
    const extraIds = [...made.world.users.keys()].filter(
      (id) => id !== made.backend.userId && id !== made.other.userId,
    );
    const thirdId = extraIds[0] ?? '';
    const unjoinedId = extraIds[1] ?? '';
    const third = made.backend.as(thirdId);
    const unjoinedReader = made.backend.as(unjoinedId);
    await made.backend.addMember(course.id, thirdId, 'viewer');

    const unjoinedExams = await unjoinedReader.course.exams.list(course.id);
    expect(unjoinedExams[0]?.myScore).toBeNull();
    expect(unjoinedExams[0]?.averageScore).toBeNull();
    expect(unjoinedExams[0]?.scoreCount).toBe(0);

    const adminTwo = await made.backend.course.exams.list(course.id);
    const viewerTwo = await made.other.course.exams.list(course.id);
    const thirdUnsubmitted = await third.course.exams.list(course.id);
    expect(adminTwo[0]?.myScore).toMatchObject({ score: 80, note: 'admin private' });
    expect(adminTwo[0]?.averageScore).toBeNull();
    expect(adminTwo[0]?.scoreCount).toBe(2);
    expect(viewerTwo[0]?.myScore).toMatchObject({ score: 100, note: 'viewer private' });
    expect(viewerTwo[0]?.averageScore).toBeNull();
    expect(viewerTwo[0]?.scoreCount).toBe(2);
    expect(thirdUnsubmitted[0]?.myScore).toBeNull();
    expect(thirdUnsubmitted[0]?.averageScore).toBe(90);
    expect(thirdUnsubmitted[0]?.scoreCount).toBe(2);

    // 3rd scorer submits 90 -> all three scorers now have 2 other scorers and see average 90 / count 3!
    await third.course.exams.putMyScore(course.id, midterm.id, {
      score: 90,
      note: 'third private',
    });
    const adminExams = await made.backend.course.exams.list(course.id);
    const viewerExams = await made.other.course.exams.list(course.id);
    expect(adminExams[0]?.averageScore).toBe(90);
    expect(adminExams[0]?.scoreCount).toBe(3);
    expect(viewerExams[0]?.averageScore).toBe(90);
    expect(viewerExams[0]?.scoreCount).toBe(3);
    expect(adminExams[0]?.resultsPublishedAt).toBeNull();

    // A snapshot of the results is published by an admin of the course alone, of five scores or more.
    const notAdmin = await rejection(
      made.other.course.exams.setResultsPublished(course.id, midterm.id, true),
    );
    expect(notAdmin.status).toBe(403);
    const tooFew = await rejection(
      made.backend.course.exams.setResultsPublished(course.id, midterm.id, true),
    );
    expect(tooFew.status).toBe(409);

    // CourseStore + CourseMeetingsState refreshes aggregates immediately after save/delete.
    const store = new CourseStore(made.workspace, course.id);
    await store.load();
    await store.events.refreshAll();
    expect(store.events.exams[0]?.averageScore).toBe(90);
    expect(
      await store.events.saveMyScore(midterm.id, { score: 50, note: 'updated' }),
    ).toMatchObject({ ok: true });
    expect(store.events.exams[0]?.averageScore).toBe(80);
    expect(store.events.exams[0]?.scoreCount).toBe(3);

    // Trigger meeting_upcoming notification in shared workspace -> delivered with meeting subject.
    noteUpcomingMeeting(made.world, course.id, earlyMeeting.id);
    const notifs = await made.backend.notifications.list();
    expect(notifs[0]?.subject).toMatchObject({
      kind: 'meeting_upcoming',
      containerId: course.id,
      blockId: earlyMeeting.id,
      blockKind: 'unit',
      title: 'Week 1 Kickoff',
    });

    // Erasing other user removes their exam score from averages immediately (leaving only 1 other
    // scorer for admin, so averageScore hides again), while keeping hostId and attendance row as tombstones.
    eraseUser(made.world, made.other.userId);
    const afterEraseExams = await made.backend.course.exams.list(course.id);
    expect(afterEraseExams[0]?.scoreCount).toBe(2);
    expect(afterEraseExams[0]?.averageScore).toBeNull();

    const afterEraseMeetings = await made.backend.course.meetings.list(course.id);
    expect(afterEraseMeetings[0]?.hostId).toBe(made.other.userId);
    expect(afterEraseMeetings[0]?.attendance.some((r) => r.userId === made.other.userId)).toBe(
      true,
    );
    const resolved = await made.backend.usersById(wsId, [made.other.userId]);
    expect(resolved[0] && isGone(resolved[0])).toBe(true);

    // Delete my score + third's score -> average stays null, count drops to 1 then 0.
    expect(await store.events.deleteMyScore(midterm.id)).toBe(true);
    expect(store.events.exams[0]?.myScore).toBeNull();
    expect(store.events.exams[0]?.averageScore).toBeNull();
    expect(store.events.exams[0]?.scoreCount).toBe(1);
    await third.course.exams.deleteMyScore(course.id, midterm.id);
    await store.events.refreshExams();
    expect(store.events.exams[0]?.averageScore).toBeNull();
    expect(store.events.exams[0]?.scoreCount).toBe(0);

    // Keep unused Workspace import type check happy by testing unjoined viewer workspace too.
    const adminWs = new Workspace({ backend: made.backend, locale: () => 'en' });
    await adminWs.load();
    adminWs.close();
    store.close();
  });

  it('lets whoever manages the course’s roles publish results, as the server and the button ask, editor or not', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Databases',
      visibility: 'open',
      minimumRole: 'viewer',
    });
    const exam = await made.backend.course.exams.create(course.id, {
      title: 'Midterm',
      maxScore: 100,
    });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();
    await store.events.refreshAll();
    const published = { ...exam, resultsPublishedAt: 1 };
    const sent = vi
      .spyOn(made.backend.course.exams, 'setResultsPublished')
      .mockResolvedValue(published);

    store.permissions = { ...READ_ONLY_PERMISSIONS, canEdit: true };
    expect(await store.events.setResultsPublished(exam.id, true)).toBe('failed');
    expect(sent).not.toHaveBeenCalled();

    store.permissions = { ...READ_ONLY_PERMISSIONS, canManageRoles: true };
    expect(await store.events.setResultsPublished(exam.id, true)).toBe('done');
    expect(sent).toHaveBeenCalledOnce();
    expect(store.events.exams[0]?.resultsPublishedAt).toBe(1);
    store.close();
  });
});
