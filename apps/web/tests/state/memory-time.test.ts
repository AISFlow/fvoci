import { afterEach, describe, expect, it, vi } from 'vitest';

import { BackendError, TimeTracker, Workspace, timeTracker } from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';
import { closeOwnSpace } from '$lib/state/memory/spaces.ts';

function makeEnv() {
  let clock = 1_700_000_000_000;
  const { world, readerId } = seedWorld([]);
  world.useClock(() => {
    clock += 1000;
    return clock;
  });
  const backend = new MemoryBackend(world, readerId);
  const workspace = new Workspace({ backend, locale: () => 'en' });
  return { world, readerId, backend, workspace, advance: (ms: number) => (clock += ms) };
}

async function makeBoard(backend: MemoryBackend, workspaceId: string) {
  const { board } = await backend.createBoard(
    workspaceId,
    { title: 'Test Board' },
    {
      title: 'Board',
      fields: {
        viewType: 'board',
        groupBy: '',
        sort: [],
        hiddenGroups: [],
        collapsedGroups: [],
        shownProperties: [],
        visibleGroups: [],
        cardOrder: [],
        columnWidths: {},
        columnCalculations: {},
        groupCalculations: {},
        defaultTemplateId: '',
        filter: { operation: 'and', filters: [] },
      },
    },
  );
  return board;
}

describe('MemoryTime parity with server rules', () => {
  it('enforces single active timer with atomic switch and returns stopped entry', async () => {
    const { backend, world, advance } = makeEnv();
    const board = await makeBoard(backend, world.workspace.id);
    const card = await backend.createCard(board.id, { title: 'Write specs' });
    const course = await backend.course.createCourse(world.workspace.id, {
      title: 'Math 101',
      visibility: 'private',
    });
    const unit = await backend.course.createUnit(course.id, { kind: 'unit', title: 'Limits' });

    const first = await backend.time.startTimeEntry({
      workspaceId: world.workspace.id,
      containerId: board.id,
      blockId: card.id,
      kind: 'work',
    });
    expect(first.stopped).toBeUndefined();
    expect(first.entry.endedAt).toBeNull();
    expect(first.entry.targetKind).toBe('card');
    expect(first.entry.targetTitle).toBe('Write specs');

    advance(60_000);
    const second = await backend.time.startTimeEntry({
      workspaceId: world.workspace.id,
      containerId: course.id,
      blockId: unit.id,
      kind: 'lecture',
    });
    expect(second.stopped?.id).toBe(first.entry.id);
    expect(second.stopped?.endedAt).toBeGreaterThan(first.entry.startedAt);
    expect(second.entry.targetKind).toBe('unit');
    expect(second.entry.targetTitle).toBe('Limits');

    const active = await backend.time.getActiveTimeEntry();
    expect(active?.id).toBe(second.entry.id);

    const stopped = await backend.time.stopTimeEntry();
    expect(stopped.id).toBe(second.entry.id);
    expect(stopped.endedAt).not.toBeNull();
    expect(await backend.time.getActiveTimeEntry()).toBeNull();
    await expect(backend.time.stopTimeEntry()).rejects.toMatchObject({ status: 404 });
  });

  it('filters inclusive [from, to] and aggregates summary across pages with active=0ms', async () => {
    const { backend } = makeEnv();
    const e1 = await backend.time.createTimeEntry({
      kind: 'study',
      startedAt: 1000,
      endedAt: 4000,
      note: 'first',
    });
    const e2 = await backend.time.createTimeEntry({
      kind: 'lecture',
      startedAt: 5000,
      endedAt: 10000,
      note: 'second',
    });
    const running = await backend.time.startTimeEntry({ kind: 'work' });

    const page0 = await backend.time.listTimeEntries({
      from: 1000,
      to: running.entry.startedAt,
      page: 0,
      perPage: 2,
    });
    expect(page0.entries.map((e) => e.id)).toEqual([running.entry.id, e2.id]);
    expect(page0.summary.count).toBe(3);
    expect(page0.summary.totalMs).toBe(8000);
    expect(page0.summary.byKind).toEqual({ lecture: 5000, study: 3000, work: 0, other: 0 });

    const exact = await backend.time.listTimeEntries({ from: 1000, to: 1000 });
    expect(exact.entries.map((e) => e.id)).toEqual([e1.id]);
  });

  it('rejects invalid ranges, re-opening stopped entries, templates, chapters, and preserves deleted target snapshots', async () => {
    const { backend, world } = makeEnv();
    const board = await makeBoard(backend, world.workspace.id);

    // Template card rejected
    const tplCard = await backend.createCard(board.id, { title: 'Template', isTemplate: true });
    await expect(
      backend.time.startTimeEntry({
        workspaceId: world.workspace.id,
        containerId: board.id,
        blockId: tplCard.id,
        kind: 'work',
      }),
    ).rejects.toMatchObject({ status: 404 });

    // Chapter rejected
    const course = await backend.course.createCourse(world.workspace.id, {
      title: 'Course',
      visibility: 'private',
    });
    const chapter = await backend.course.createUnit(course.id, {
      kind: 'chapter',
      title: 'Ch 1',
    });
    await expect(
      backend.time.startTimeEntry({
        workspaceId: world.workspace.id,
        containerId: course.id,
        blockId: chapter.id,
        kind: 'lecture',
      }),
    ).rejects.toMatchObject({ status: 404 });

    // endedAt < startedAt on create
    await expect(
      backend.time.createTimeEntry({ kind: 'study', startedAt: 5000, endedAt: 4000 }),
    ).rejects.toMatchObject({ status: 400 });

    // Re-opening stopped entry on PATCH rejected
    const created = await backend.time.createTimeEntry({
      kind: 'study',
      startedAt: 5000,
      endedAt: 8000,
    });
    await expect(
      backend.time.patchTimeEntry(created.id, { endedAt: null as unknown as number }),
    ).rejects.toMatchObject({ status: 400 });

    // Patching startedAt past stored endedAt points to /startedAt
    try {
      await backend.time.patchTimeEntry(created.id, { startedAt: 9000 });
      expect.unreachable('should fail');
    } catch (error) {
      expect(error).toBeInstanceOf(BackendError);
      expect((error as BackendError).refusalAt('/startedAt')).toBeDefined();
    }

    // Deleted target keeps targetTitle snapshot
    const card = await backend.createCard(board.id, { title: 'Snapshot Card' });
    const logged = await backend.time.createTimeEntry({
      workspaceId: world.workspace.id,
      containerId: board.id,
      blockId: card.id,
      kind: 'work',
      startedAt: 10_000,
      endedAt: 20_000,
    });
    await backend.batchDeleteCards(board.id, [card.id]);
    const listed = await backend.time.listTimeEntries({ from: 10_000, to: 20_000 });
    expect(listed.entries[0]?.id).toBe(logged.id);
    expect(listed.entries[0]?.targetTitle).toBe('Snapshot Card');
  });

  it('hides locked personal-space entries on read and enforces password wall on write', async () => {
    const { backend, world, readerId } = makeEnv();
    await backend.time.startTimeEntry({ kind: 'other' });
    closeOwnSpace(world, readerId, 'secret-pass');

    // Read silently omits locked personal-space timer
    expect(await backend.time.getActiveTimeEntry()).toBeNull();
    expect((await backend.time.listTimeEntries()).entries).toEqual([]);

    // Targetless write triggers 401 password_sign_in_required
    await expect(
      backend.time.createTimeEntry({ kind: 'study', startedAt: 1000, endedAt: 2000 }),
    ).rejects.toMatchObject({ status: 401, reason: 'password_sign_in_required' });
  });
});

describe('TimeTracker store', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('tracks active timer, ticks elapsedMs, surfaces switch notice, and captures session wall', async () => {
    vi.useFakeTimers();
    const { backend, workspace, world, readerId } = makeEnv();
    vi.setSystemTime(world.now() + 10_000);
    world.useClock(() => Date.now());
    await workspace.load(world.workspace.id);
    const tracker = timeTracker(workspace);
    expect(tracker).toBeInstanceOf(TimeTracker);
    if (!tracker) throw new Error('expected tracker');

    const stopSync = tracker.start();
    const board = await makeBoard(backend, world.workspace.id);
    const c1 = await backend.createCard(board.id, { title: 'Card A' });
    const c2 = await backend.createCard(board.id, { title: 'Card B' });

    await tracker.startTimer({
      workspaceId: world.workspace.id,
      containerId: board.id,
      blockId: c1.id,
      kind: 'work',
    });
    expect(tracker.isRunningFor(c1.id)).toBe(true);
    expect(tracker.lastSwitched).toBeNull();

    vi.advanceTimersByTime(4000);
    expect(tracker.elapsedMs).toBeGreaterThanOrEqual(3000);

    await tracker.startTimer({
      workspaceId: world.workspace.id,
      containerId: board.id,
      blockId: c2.id,
      kind: 'study',
    });
    expect(tracker.isRunningFor(c2.id)).toBe(true);
    expect(tracker.isRunningFor(c1.id)).toBe(false);
    expect(tracker.lastSwitched?.stopped.blockId).toBe(c1.id);

    await tracker.stopTimer();
    expect(tracker.active).toBeNull();

    // Password session wall captured instead of generic error
    closeOwnSpace(world, readerId, 'pw');
    await tracker.startTimer({ kind: 'other' });
    expect(tracker.wall).toEqual({ kind: 'password' });

    stopSync();
    workspace.close();
  });

  it('reports signed-out 401 in listEntries, createEntry, and patchEntry while keeping 400 validation errors local', async () => {
    const { backend, workspace, world } = makeEnv();
    await workspace.load(world.workspace.id);
    const tracker = timeTracker(workspace);
    if (!tracker) throw new Error('expected tracker');

    // Validation 400 re-throws without reporting a workspace problem or signing out.
    await expect(
      tracker.createEntry({ kind: 'study', startedAt: 5000, endedAt: 4000 }),
    ).rejects.toMatchObject({ status: 400 });
    expect(workspace.status).toBe('ready');
    expect(workspace.problems).toEqual([]);

    // Signed-out 401 on listEntries reports load error and transitions workspace to signedOut.
    const reportSpy = vi.spyOn(workspace, 'report');
    vi.spyOn(backend.time, 'listTimeEntries').mockRejectedValueOnce(
      new BackendError(401, { detail: 'expired' }),
    );
    await expect(tracker.listEntries()).rejects.toMatchObject({ status: 401 });
    expect(reportSpy).toHaveBeenCalledWith('load', expect.objectContaining({ status: 401 }));
    expect(workspace.status).toBe('signedOut');

    // Signed-out 401 on createEntry reports save error.
    reportSpy.mockClear();
    vi.spyOn(backend.time, 'createTimeEntry').mockRejectedValueOnce(
      new BackendError(401, { detail: 'expired' }),
    );
    await expect(
      tracker.createEntry({ kind: 'study', startedAt: 1000, endedAt: 2000 }),
    ).rejects.toMatchObject({ status: 401 });
    expect(reportSpy).toHaveBeenCalledWith('save', expect.objectContaining({ status: 401 }));

    // Signed-out 401 on patchEntry reports save error.
    reportSpy.mockClear();
    vi.spyOn(backend.time, 'patchTimeEntry').mockRejectedValueOnce(
      new BackendError(401, { detail: 'expired' }),
    );
    await expect(tracker.patchEntry('entry-1', { note: 'x' })).rejects.toMatchObject({
      status: 401,
    });
    expect(reportSpy).toHaveBeenCalledWith('save', expect.objectContaining({ status: 401 }));

    // Password wall 401 on createEntry/patchEntry captures wall instead of reporting signed-out.
    reportSpy.mockClear();
    vi.spyOn(backend.time, 'createTimeEntry').mockRejectedValueOnce(
      new BackendError(401, { reason: 'password_sign_in_required', detail: 'pw required' }),
    );
    await expect(
      tracker.createEntry({ kind: 'study', startedAt: 1000, endedAt: 2000 }),
    ).rejects.toMatchObject({ status: 401, reason: 'password_sign_in_required' });
    expect(reportSpy).not.toHaveBeenCalled();
    expect(tracker.wall).toEqual({ kind: 'password' });

    workspace.close();
  });
});
