/**
 * Personal unit progress & course summary tests:
 * - API client serialization (`completed: null` never sent, field omission preserved)
 * - In-memory backend rules (isolation, idempotent `completedAt`, `lastPosition` 256-char/control
 *   validation, chapter/trashed/non-course 404, unjoined open-course 403, restore recovery,
 *   active-member summary, zero WS events)
 * - `CourseStore` progress state & derived live counts (including 0-unit guard)
 */
import { describe, expect, it, vi } from 'vitest';

import {
  getCourseProgressSummary,
  getMyCourseProgress,
  putUnitProgress,
} from '$lib/api/unit-progress.ts';
import type { WikiRequest } from '$lib/api/wikis.ts';
import {
  BackendError,
  CourseStore,
  UNIT_PROGRESS_POSITION_MAX_CHARS,
  Workspace,
  type UnitProgress,
} from '$lib/state/index.ts';
import { eraseUser } from '$lib/state/memory/users.ts';

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

describe('unit progress API client', () => {
  it('calls the 3 progress routes and never sends completed: null', async () => {
    const calls: {
      path: string;
      params: Record<string, string>;
      options?: unknown;
      ws?: string;
    }[] = [];
    const sample: UnitProgress = {
      unitId: 'u1',
      completedAt: 1000,
      plannedDate: null,
      lastPosition: '12:30',
      updatedAt: 1000,
    };
    const request: WikiRequest = <T>(
      path: string,
      params: Record<string, string>,
      options?: unknown,
      workspaceId?: string,
    ): Promise<T> => {
      calls.push({ path, params, options, ws: workspaceId });
      if (path === '/courses/{courseId}/progress') return Promise.resolve([sample] as T);
      if (path === '/courses/{courseId}/progress/summary') {
        return Promise.resolve({
          totalUnits: 2,
          members: [{ userId: 'm1', completedUnits: 1 }],
        } as T);
      }
      return Promise.resolve(sample as T);
    };

    expect(await getMyCourseProgress(request, 'ws1', 'c1')).toEqual([sample]);
    expect(await getCourseProgressSummary(request, 'ws1', 'c1')).toEqual({
      totalUnits: 2,
      members: [{ userId: 'm1', completedUnits: 1 }],
    });

    // Runtime-null completed is stripped; only lastPosition is serialized.
    await putUnitProgress(request, 'ws1', 'c1', 'u1', {
      completed: null as unknown as boolean,
      lastPosition: '12:30',
    });
    expect(calls.at(-1)).toEqual({
      path: '/courses/{courseId}/units/{unitId}/progress',
      params: { courseId: 'c1', unitId: 'u1' },
      options: { method: 'PUT', json: { lastPosition: '12:30' } },
      ws: 'ws1',
    });

    // Omitting lastPosition keeps only completed.
    await putUnitProgress(request, 'ws1', 'c1', 'u1', { completed: true });
    expect(calls.at(-1)?.options).toEqual({ method: 'PUT', json: { completed: true } });

    // Clearing lastPosition sends explicit null while omitting completed.
    await putUnitProgress(request, 'ws1', 'c1', 'u1', { lastPosition: null });
    expect(calls.at(-1)?.options).toEqual({ method: 'PUT', json: { lastPosition: null } });
  });
});

describe('unit progress in memory backend', () => {
  it('enforces per-user isolation, idempotent completedAt, validation, 404/403, trash restore, and zero WS events', async () => {
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
    const u2 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Borrowing',
      parentId: ch.id,
    });

    const events: unknown[] = [];
    const stop = made.backend.watchContainer(course.id, (change) => events.push(change));

    // Unjoined open-course reader gets [] on progress, 403 on PUT, 200 on summary.
    expect(await made.other.course.myProgress(course.id)).toEqual([]);
    expect(
      (await rejection(made.other.course.putUnitProgress(course.id, u1.id, { completed: true })))
        .status,
    ).toBe(403);
    expect(await made.other.course.progressSummary(course.id)).toEqual({
      totalUnits: 2,
      members: [{ userId: made.backend.userId, completedUnits: 0 }],
    });

    // Chapters and non-course containers return 404.
    expect(
      (await rejection(made.backend.course.putUnitProgress(course.id, ch.id, { completed: true })))
        .status,
    ).toBe(404);
    const [board] = await made.backend.boards(wsId);
    expect((await rejection(made.backend.course.myProgress(board?.id ?? ''))).status).toBe(404);

    // Validation: completed: null, control chars, > 256 Unicode scalar values.
    expect(
      (
        await rejection(
          made.backend.course.putUnitProgress(course.id, u1.id, {
            completed: null as unknown as boolean,
          }),
        )
      ).status,
    ).toBe(400);
    const ctrl = await rejection(
      made.backend.course.putUnitProgress(course.id, u1.id, { lastPosition: 'line1\nline2' }),
    );
    expect([ctrl.status, ctrl.refusalAt('/lastPosition')?.reason]).toEqual([
      400,
      'invalid_characters',
    ]);
    const tooLong = await rejection(
      made.backend.course.putUnitProgress(course.id, u1.id, {
        lastPosition: '가'.repeat(UNIT_PROGRESS_POSITION_MAX_CHARS + 1),
      }),
    );
    expect([tooLong.status, tooLong.refusalAt('/lastPosition')?.reason]).toEqual([400, 'too_long']);

    // Idempotent completedAt + exact 256 scalar chars trimmed.
    const first = await made.backend.course.putUnitProgress(course.id, u1.id, {
      completed: true,
      lastPosition: `  ${'가'.repeat(UNIT_PROGRESS_POSITION_MAX_CHARS)}  `,
    });
    expect(first.completedAt).toBeTypeOf('number');
    expect(first.lastPosition).toBe('가'.repeat(UNIT_PROGRESS_POSITION_MAX_CHARS));

    const repeated = await made.backend.course.putUnitProgress(course.id, u1.id, {
      completed: true,
    });
    expect(repeated.completedAt).toBe(first.completedAt);
    expect(repeated.lastPosition).toBe(first.lastPosition);

    // Clearing completed preserves lastPosition; blank lastPosition clears it.
    const undone = await made.backend.course.putUnitProgress(course.id, u1.id, {
      completed: false,
    });
    expect(undone.completedAt).toBeNull();
    expect(undone.lastPosition).toBe(first.lastPosition);
    const cleared = await made.backend.course.putUnitProgress(course.id, u1.id, {
      completed: true,
      lastPosition: '   ',
    });
    expect(cleared.completedAt).toBeTypeOf('number');
    expect(cleared.lastPosition).toBeNull();

    // Zero WS events emitted for any progress write.
    await settle();
    expect(events).toEqual([]);
    stop();

    // Viewer can track their own progress, isolated from admin.
    await made.backend.addMember(course.id, made.other.userId, 'viewer');
    expect(await made.other.course.myProgress(course.id)).toEqual([]);
    await made.other.course.putUnitProgress(course.id, u2.id, {
      completed: true,
      lastPosition: '12:30',
    });
    expect((await made.backend.course.myProgress(course.id)).map((r) => r.unitId)).toEqual([u1.id]);
    expect(await made.backend.course.progressSummary(course.id)).toEqual({
      totalUnits: 2,
      members: [made.backend.userId, made.other.userId]
        .sort()
        .map((userId) => ({ userId, completedUnits: 1 })),
    });

    // Deleting a unit omits it from progress & summary and rejects PUT (404), then restore recovers it.
    await made.backend.course.deleteUnit(course.id, u1.id);
    expect(await made.backend.course.myProgress(course.id)).toEqual([]);
    expect(
      (await rejection(made.backend.course.putUnitProgress(course.id, u1.id, { completed: true })))
        .status,
    ).toBe(404);
    expect((await made.backend.course.progressSummary(course.id)).totalUnits).toBe(1);

    await made.backend.course.restoreUnit(course.id, u1.id);
    const restored = await made.backend.course.myProgress(course.id);
    expect(restored).toHaveLength(1);
    expect(restored[0]?.completedAt).toBe(cleared.completedAt);

    // Erased account is excluded from summary.
    eraseUser(made.world, made.other.userId);
    expect(await made.backend.course.progressSummary(course.id)).toEqual({
      totalUnits: 2,
      members: [{ userId: made.backend.userId, completedUnits: 1 }],
    });
  });
});

describe('CourseStore progress integration', () => {
  it('loads, toggles, saves lastPosition, guards 0 units, and updates live counts across unit changes', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Type Systems',
      visibility: 'open',
    });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    // 0-unit course safely yields 0%.
    expect([
      store.progress.totalUnits,
      store.progress.completedUnits,
      store.progress.percent,
    ]).toEqual([0, 0, 0]);

    const ch = await store.createUnit({ kind: 'chapter', title: 'Part 1' });
    const u1 = await store.createUnit({ kind: 'unit', title: 'Unit 1', parentId: ch?.id });
    const u2 = await store.createUnit({ kind: 'unit', title: 'Unit 2', parentId: ch?.id });
    if (!ch || !u1 || !u2) throw new Error('expected units');

    // Chapters cannot be completed.
    expect(await store.toggleUnitComplete(ch.id)).toBe(false);
    expect(store.progress.totalUnits).toBe(2);

    // Toggle Unit 1 complete and verify myProgress + reconciled progressSummary.
    expect(await store.toggleUnitComplete(u1.id)).toBe(true);
    expect([store.progress.completedUnits, store.progress.percent]).toEqual([1, 50]);
    expect(
      store.progressSummary?.members.find((m) => m.userId === made.backend.userId)?.completedUnits,
    ).toBe(1);

    // Save and clear lastPosition without changing completion.
    expect(await store.setUnitLastPosition(u1.id, ' 12:30 ')).toBe(true);
    expect(store.progress.of(u1.id)?.lastPosition).toBe('12:30');
    expect(store.progress.isCompleted(u1.id)).toBe(true);
    expect(await store.setUnitLastPosition(u1.id, null)).toBe(true);
    expect(store.progress.of(u1.id)?.lastPosition).toBeNull();
    expect(store.progress.isCompleted(u1.id)).toBe(true);

    // Delete & restore Unit 1 keeps live counts accurate and restores progress.
    await store.deleteUnit(u1.id);
    expect([
      store.progress.totalUnits,
      store.progress.completedUnits,
      store.progress.percent,
    ]).toEqual([1, 0, 0]);
    await store.restoreUnit(u1.id);
    await settle();
    expect([
      store.progress.totalUnits,
      store.progress.completedUnits,
      store.progress.percent,
    ]).toEqual([2, 1, 50]);

    // Unjoined reader cannot track until joining; removing clears myProgress.
    const otherWorkspace = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWorkspace.load();
    const otherStore = new CourseStore(otherWorkspace, course.id);
    await otherStore.load();
    expect([otherStore.joinable, otherStore.canTrackProgress]).toEqual([true, false]);
    expect(await otherStore.toggleUnitComplete(u1.id)).toBe(false);

    expect(await otherStore.join()).toBe(true);
    expect(otherStore.canTrackProgress).toBe(true);
    expect(await otherStore.toggleUnitComplete(u2.id)).toBe(true);
    expect(otherStore.progress.completedUnits).toBe(1);

    // Refreshing summary on admin store sees other's completion count (never unit details).
    const summarySpy = vi.spyOn(made.backend.course, 'progressSummary');
    await store.refreshProgressSummary();
    expect(summarySpy).toHaveBeenCalledTimes(1);
    expect(
      store.progressSummary?.members.find((m) => m.userId === made.other.userId)?.completedUnits,
    ).toBe(1);

    await made.backend.removeMember(course.id, made.other.userId);
    await settle();
    expect(otherStore.canTrackProgress).toBe(false);
    expect(otherStore.myProgress.size).toBe(0);

    // An in-flight refreshSummary started before a completion toggle must not overwrite reconciled state.
    let resolveStaleSummary:
      | ((value: {
          totalUnits: number;
          members: { userId: string; completedUnits: number }[];
        }) => void)
      | undefined;
    summarySpy.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveStaleSummary = resolve;
        }),
    );
    const staleRefresh = store.refreshProgressSummary();
    expect(await store.toggleUnitComplete(u2.id)).toBe(true);
    expect(
      store.progressSummary?.members.find((m) => m.userId === made.backend.userId)?.completedUnits,
    ).toBe(2);
    resolveStaleSummary?.({
      totalUnits: 2,
      members: [{ userId: made.backend.userId, completedUnits: 1 }],
    });
    await staleRefresh;
    expect(
      store.progressSummary?.members.find((m) => m.userId === made.backend.userId)?.completedUnits,
    ).toBe(2);

    otherStore.close();
    store.close();
  });
});
