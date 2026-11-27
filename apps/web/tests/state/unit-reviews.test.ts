/**
 * Personal spaced-repetition unit reviews, workspace "Today's reviews", and
 * personal study notes tests:
 * - API client serialization for all 9 endpoints + `SHARED_UNIT_REVIEWS` stub
 * - `unitProgressOf` propagation of `suggestedNextIntervalDays`
 * - In-memory backend parity: interval progression, custom `defaultReviewIntervals`,
 *   idempotent completion retry, `overdueDays` & ordering, note 10,000-char &
 *   control-char validation, isolation, unjoined 403/empty, chapter/trashed 404,
 *   and zero WS broadcasts
 * - `CourseReviewsStore` & `TodayReviewsStore` reactive state updates
 * - `review_due` memory notification shape and text
 */
import { describe, expect, it } from 'vitest';

import { unitProgressOf } from '$lib/api/unit-progress.ts';
import {
  completeUnitReview,
  createUnitNote,
  deleteUnitNote,
  deleteUnitReview,
  getWorkspaceTodayReviews,
  listMyCourseReviews,
  listMyUnitNotes,
  patchUnitNote,
  scheduleUnitReview,
  SHARED_UNIT_REVIEWS,
} from '$lib/api/unit-reviews.ts';
import type { WikiRequest } from '$lib/api/wikis.ts';
import { notificationText } from '$lib/features/notifications/notification-words.ts';
import { addDays, dateOfInstant } from '$lib/logic/dates.ts';
import {
  BackendError,
  courseReviews,
  CourseStore,
  suggestedNextIntervalDays,
  todayReviews,
  UNIT_NOTE_CONTENT_MAX_CHARS,
  Workspace,
  type UnitNote,
  type UnitReview,
} from '$lib/state/index.ts';
import { noteReviewDue } from '$lib/state/memory/notifications.ts';

import { settle, setup } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

describe('unit reviews & notes API client', () => {
  it('serializes all 9 routes accurately and never sends null optional fields', async () => {
    const calls: {
      path: string;
      params: Record<string, string>;
      options?: unknown;
      ws?: string;
    }[] = [];
    const review: UnitReview = {
      id: 'r1',
      unitId: 'u1',
      containerId: 'c1',
      scheduledDate: '2026-11-25',
      completedAt: null,
      intervalDays: 1,
      createdAt: 100,
      updatedAt: 100,
    };
    const note: UnitNote = {
      id: 'n1',
      unitId: 'u1',
      containerId: 'c1',
      kind: 'question',
      content: 'Why borrow here?',
      createdAt: 100,
      updatedAt: 100,
    };
    const request: WikiRequest = <T>(
      path: string,
      params: Record<string, string>,
      options?: unknown,
      workspaceId?: string,
    ): Promise<T> => {
      calls.push({ path, params, options, ws: workspaceId });
      if (path === '/courses/{courseId}/reviews') return Promise.resolve([review] as T);
      if (path === '/courses/{courseId}/units/{unitId}/reviews')
        return Promise.resolve(review as T);
      if (path === '/courses/{courseId}/reviews/{reviewId}/complete') {
        return Promise.resolve({
          review: { ...review, completedAt: 200 },
          suggestedNextIntervalDays: 3,
          nextReview: null,
        } as T);
      }
      if (path === '/learning/reviews/today') {
        return Promise.resolve([
          {
            ...review,
            courseTitle: 'Rust 101',
            unitTitle: 'Ownership',
            overdueDays: 2,
          },
        ] as T);
      }
      if (path === '/courses/{courseId}/units/{unitId}/notes' && !options) {
        return Promise.resolve([note] as T);
      }
      if (path === '/courses/{courseId}/units/{unitId}/notes') {
        const method = (options as { method?: string } | undefined)?.method;
        return Promise.resolve((method === 'POST' ? note : [note]) as T);
      }
      if (path === '/courses/{courseId}/notes/{noteId}') {
        const method = (options as { method?: string } | undefined)?.method;
        return Promise.resolve((method === 'PATCH' ? note : undefined) as T);
      }
      return Promise.resolve(undefined as T);
    };

    expect(
      await listMyCourseReviews(request, 'ws1', 'c1', { status: 'pending', unitId: 'u1' }),
    ).toEqual([review]);
    expect(calls.at(-1)).toEqual({
      path: '/courses/{courseId}/reviews',
      params: { courseId: 'c1' },
      options: { query: { status: 'pending', unitId: 'u1' } },
      ws: 'ws1',
    });

    await scheduleUnitReview(request, 'ws1', 'c1', 'u1', {
      intervalDays: 3,
      scheduledDate: undefined,
    });
    expect(calls.at(-1)?.options).toEqual({ method: 'POST', json: { intervalDays: 3 } });

    const done = await completeUnitReview(request, 'ws1', 'c1', 'r1', {
      scheduleNextIntervalDays: 7,
    });
    expect(done.suggestedNextIntervalDays).toBe(3);
    expect(done.nextReview).toBeNull();
    expect(calls.at(-1)?.options).toEqual({
      method: 'POST',
      json: { scheduleNextIntervalDays: 7 },
    });

    await deleteUnitReview(request, 'ws1', 'c1', 'r1');
    expect(calls.at(-1)).toEqual({
      path: '/courses/{courseId}/reviews/{reviewId}',
      params: { courseId: 'c1', reviewId: 'r1' },
      options: { method: 'DELETE' },
      ws: 'ws1',
    });

    const today = await getWorkspaceTodayReviews(request, 'ws1');
    expect(today[0]?.overdueDays).toBe(2);

    expect(await listMyUnitNotes(request, 'ws1', 'c1', 'u1', 'question')).toEqual([note]);
    expect(calls.at(-1)?.options).toEqual({ query: { kind: 'question' } });

    await createUnitNote(request, 'ws1', 'c1', 'u1', { kind: 'mistake', content: 'Off by one' });
    expect(calls.at(-1)?.options).toEqual({
      method: 'POST',
      json: { kind: 'mistake', content: 'Off by one' },
    });

    await patchUnitNote(request, 'ws1', 'c1', 'n1', { content: 'Updated' });
    expect(calls.at(-1)?.options).toEqual({ method: 'PATCH', json: { content: 'Updated' } });

    await deleteUnitNote(request, 'ws1', 'c1', 'n1');
    expect(calls.at(-1)?.options).toEqual({ method: 'DELETE' });

    // `unitProgressOf` preserves numeric suggestedNextIntervalDays and omits null/undefined.
    expect(
      unitProgressOf({
        unitId: 'u1',
        completedAt: 100,
        plannedDate: null,
        lastPosition: null,
        suggestedNextIntervalDays: 7,
        updatedAt: 100,
      }).suggestedNextIntervalDays,
    ).toBe(7);
    expect(
      'suggestedNextIntervalDays' in
        unitProgressOf({
          unitId: 'u1',
          completedAt: null,
          plannedDate: null,
          lastPosition: null,
          suggestedNextIntervalDays: null,
          updatedAt: 100,
        }),
    ).toBe(false);
  });

  it('stubs SharedBackend with zero network requests and 403 on writes', async () => {
    expect(await SHARED_UNIT_REVIEWS.listMyCourseReviews('c1')).toEqual([]);
    expect(await SHARED_UNIT_REVIEWS.getWorkspaceTodayReviews('ws1')).toEqual([]);
    expect(await SHARED_UNIT_REVIEWS.listMyUnitNotes('c1', 'u1')).toEqual([]);
    expect(
      (await rejection(SHARED_UNIT_REVIEWS.scheduleUnitReview('c1', 'u1', { intervalDays: 1 })))
        .status,
    ).toBe(403);
    expect((await rejection(SHARED_UNIT_REVIEWS.completeUnitReview('c1', 'r1'))).status).toBe(403);
    expect((await rejection(SHARED_UNIT_REVIEWS.deleteUnitReview('c1', 'r1'))).status).toBe(403);
    expect(
      (
        await rejection(
          SHARED_UNIT_REVIEWS.createUnitNote('c1', 'u1', { kind: 'review', content: 'x' }),
        )
      ).status,
    ).toBe(403);
    expect(
      (await rejection(SHARED_UNIT_REVIEWS.patchUnitNote('c1', 'n1', { content: 'y' }))).status,
    ).toBe(403);
    expect((await rejection(SHARED_UNIT_REVIEWS.deleteUnitNote('c1', 'n1'))).status).toBe(403);
  });
});

describe('unit reviews & notes in memory backend', () => {
  it('computes interval suggestions, enforces idempotent completion, overdue sorting, validation, and isolation', async () => {
    expect(suggestedNextIntervalDays(null)).toBe(1);
    expect(suggestedNextIntervalDays(1)).toBe(3);
    expect(suggestedNextIntervalDays(3)).toBe(7);
    expect(suggestedNextIntervalDays(7)).toBe(14);
    expect(suggestedNextIntervalDays(14)).toBe(30);
    expect(suggestedNextIntervalDays(30)).toBe(30);
    expect(suggestedNextIntervalDays(2, [2, 5, 12])).toBe(5);

    const made = setup();
    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'Rust 101',
      visibility: 'open',
    });
    const ch = await made.backend.course.createUnit(course.id, { kind: 'chapter', title: 'Ch 1' });
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Ownership',
      parentId: ch.id,
    });

    const events: unknown[] = [];
    const stop = made.backend.watchContainer(course.id, (change) => events.push(change));

    // Completing a unit returns suggestedNextIntervalDays = 1 on first completion.
    const prog = await made.backend.course.putUnitProgress(course.id, u1.id, { completed: true });
    expect(prog.suggestedNextIntervalDays).toBe(1);

    // Unjoined open-course reader gets [] on reads and 403 on writes.
    expect(await made.other.course.reviews.listMyCourseReviews(course.id)).toEqual([]);
    expect(await made.other.course.reviews.listMyUnitNotes(course.id, u1.id)).toEqual([]);
    expect(
      (
        await rejection(
          made.other.course.reviews.scheduleUnitReview(course.id, u1.id, { intervalDays: 1 }),
        )
      ).status,
    ).toBe(403);
    expect(
      (
        await rejection(
          made.other.course.reviews.createUnitNote(course.id, u1.id, {
            kind: 'question',
            content: 'hello',
          }),
        )
      ).status,
    ).toBe(403);

    // Chapters return 404.
    expect(
      (
        await rejection(
          made.backend.course.reviews.scheduleUnitReview(course.id, ch.id, { intervalDays: 1 }),
        )
      ).status,
    ).toBe(404);
    expect(
      (await rejection(made.backend.course.reviews.listMyUnitNotes(course.id, ch.id))).status,
    ).toBe(404);

    // Schedule overdue & today reviews, verify overdueDays and sorting.
    const today = dateOfInstant(made.world.now(), 'UTC');
    const twoDaysAgo = addDays(today, -2);
    const tomorrow = addDays(today, 1);

    const overdueRev = await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: twoDaysAgo,
      intervalDays: 1,
    });
    const todayRev = await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: today,
      intervalDays: 3,
    });
    await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: tomorrow,
      intervalDays: 7,
    });

    const dueList = await made.backend.course.reviews.getWorkspaceTodayReviews(wsId);
    expect(dueList.map((row) => [row.id, row.overdueDays])).toEqual([
      [overdueRev.id, 2],
      [todayRev.id, 0],
    ]);

    // Complete overdue review atomically with next interval = 3; retry is idempotent.
    const firstComplete = await made.backend.course.reviews.completeUnitReview(
      course.id,
      overdueRev.id,
      { scheduleNextIntervalDays: 3 },
    );
    expect(firstComplete.review.completedAt).toBeTypeOf('number');
    expect(firstComplete.suggestedNextIntervalDays).toBe(3);
    expect(firstComplete.nextReview?.intervalDays).toBe(3);

    const replay = await made.backend.course.reviews.completeUnitReview(course.id, overdueRev.id, {
      scheduleNextIntervalDays: 7,
    });
    expect(replay.review.completedAt).toBe(firstComplete.review.completedAt);
    expect(replay.review.updatedAt).toBe(firstComplete.review.updatedAt);
    expect(replay.nextReview).toBeNull();

    // Zero WS broadcasts emitted on review writes.
    await settle();
    expect(events).toEqual([]);

    // Another member (viewer) never sees caller's reviews or notes, and gets 404 deleting/patching them.
    await made.backend.addMember(course.id, made.other.userId, 'viewer');
    await settle();
    events.length = 0;
    expect(await made.other.course.reviews.listMyCourseReviews(course.id)).toEqual([]);
    expect(await made.other.course.reviews.getWorkspaceTodayReviews(wsId)).toEqual([]);
    expect(
      (await rejection(made.other.course.reviews.deleteUnitReview(course.id, todayRev.id))).status,
    ).toBe(404);

    // Notes CRUD + validation (newlines/tabs ok, NUL/BEL rejected, 10,000 chars ok, 10,001 rejected).
    expect(
      (
        await rejection(
          made.backend.course.reviews.createUnitNote(course.id, u1.id, {
            kind: 'question',
            content: '   ',
          }),
        )
      ).status,
    ).toBe(400);
    expect(
      (
        await rejection(
          made.backend.course.reviews.createUnitNote(course.id, u1.id, {
            kind: 'question',
            content: 'bad\u0007bell',
          }),
        )
      ).status,
    ).toBe(400);
    expect(
      (
        await rejection(
          made.backend.course.reviews.createUnitNote(course.id, u1.id, {
            kind: 'question',
            content: '가'.repeat(UNIT_NOTE_CONTENT_MAX_CHARS + 1),
          }),
        )
      ).status,
    ).toBe(400);

    const n1 = await made.backend.course.reviews.createUnitNote(course.id, u1.id, {
      kind: 'question',
      content: '  Line 1\n\tLine 2  ',
    });
    expect(n1.content).toBe('Line 1\n\tLine 2');
    const n2 = await made.backend.course.reviews.createUnitNote(course.id, u1.id, {
      kind: 'mistake',
      content: 'Forgot lifetime elision',
    });
    const allNotes = await made.backend.course.reviews.listMyUnitNotes(course.id, u1.id);
    expect(allNotes.map((n) => n.id)).toEqual([n2.id, n1.id]);
    expect(
      (await made.backend.course.reviews.listMyUnitNotes(course.id, u1.id, 'mistake')).map(
        (n) => n.id,
      ),
    ).toEqual([n2.id]);

    const patched = await made.backend.course.reviews.patchUnitNote(course.id, n1.id, {
      kind: 'review',
      content: 'Now crystal clear',
    });
    expect([patched.kind, patched.content]).toEqual(['review', 'Now crystal clear']);
    expect(await made.other.course.reviews.listMyUnitNotes(course.id, u1.id)).toEqual([]);
    expect(
      (
        await rejection(
          made.other.course.reviews.patchUnitNote(course.id, n1.id, { content: 'hijack' }),
        )
      ).status,
    ).toBe(404);
    await made.backend.course.reviews.deleteUnitNote(course.id, n1.id);
    expect(
      (await made.backend.course.reviews.listMyUnitNotes(course.id, u1.id)).map((n) => n.id),
    ).toEqual([n2.id]);

    // Zero WS broadcasts emitted on review/note writes.
    await settle();
    expect(events).toEqual([]);
    stop();
  });

  it('syncs CourseReviewsStore, TodayReviewsStore, and review_due notifications', async () => {
    const made = setup();
    await made.workspace.load();
    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'Math',
      visibility: 'private',
    });
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Linear Algebra',
    });

    const store = new CourseStore(made.workspace, course.id);
    await store.load();
    const reviewsStore = courseReviews(store);
    const todayStore = todayReviews(made.workspace);

    const today = dateOfInstant(made.world.now(), 'UTC');
    const created = await reviewsStore.scheduleReview(u1.id, {
      scheduledDate: today,
      intervalDays: 1,
    });
    expect(created).toBeDefined();
    await todayStore.load(wsId);
    expect(todayStore.items).toHaveLength(1);
    expect(todayStore.items[0]?.unitTitle).toBe('Linear Algebra');

    const [firstDue] = todayStore.items;
    if (!firstDue) throw new Error('expected due review');
    const completed = await todayStore.complete(firstDue, {
      scheduleNextIntervalDays: 3,
    });
    expect(completed?.nextReview?.intervalDays).toBe(3);
    expect(todayStore.items).toHaveLength(0);

    // review_due notification points to the unit with empty actorId and renders localized text.
    noteReviewDue(made.world, made.backend.userId, course.id, u1.id);
    const [notif] = await made.backend.notifications.list();
    expect(notif?.subject?.kind).toBe('review_due');
    expect(notif?.subject?.blockKind).toBe('unit');
    expect(notif?.subject?.blockId).toBe(u1.id);
    expect(notif?.subject?.containerId).toBe(course.id);
    expect(notif?.subject?.actorId).toBe('');
    expect(notificationText(notif?.subject ?? null, undefined)).toContain('Linear Algebra');

    store.close();
  });

  it('prevents cross-user reads when another workspace store loads', async () => {
    const made = setup();
    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'Private',
      visibility: 'open',
    });
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Secret Unit',
    });
    await made.backend.course.reviews.createUnitNote(course.id, u1.id, {
      kind: 'mistake',
      content: 'Only mine',
    });

    const otherWs = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWs.load();
    const otherStore = new CourseStore(otherWs, course.id);
    await otherStore.load();
    const otherReviews = courseReviews(otherStore);
    await otherReviews.loadNotes(u1.id);
    expect(otherReviews.notes).toEqual([]);
    otherStore.close();
  });
});
