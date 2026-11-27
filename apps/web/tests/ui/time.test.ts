import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import ActiveTimerPill from '$lib/features/time/ActiveTimerPill.svelte';
import MyTimeScreen from '$lib/features/time/MyTimeScreen.svelte';
import TargetTimerButton from '$lib/features/time/TargetTimerButton.svelte';
import { dateOfInstant, startOfDay } from '$lib/logic/dates.ts';
import type { Regional } from '$lib/logic/regional.ts';
import { regional as deviceRegional } from '$lib/regional.ts';
import { BackendError, timeTracker, Workspace, type TimeEntry } from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';

const SAMPLE_ENTRY: TimeEntry = {
  id: '01930000-0000-7000-8000-000000000001',
  workspaceId: 'ws-1',
  containerId: 'b-1',
  blockId: 'c-1',
  targetKind: 'card',
  targetTitle: 'Linear Algebra Homework',
  kind: 'study',
  startedAt: 1_700_000_000_000,
  endedAt: null,
  note: '',
  createdAt: 1_700_000_000_000,
  updatedAt: 1_700_000_000_000,
};

afterEach(() => {
  vi.useRealTimers();
  cleanup();
  document.body.replaceChildren();
});

describe('ActiveTimerPill', () => {
  it('renders target title, kind badge, HH:MM:SS clock, and triggers open/stop callbacks', async () => {
    const onOpen = vi.fn();
    const onStop = vi.fn();
    render(ActiveTimerPill, {
      props: {
        entry: SAMPLE_ENTRY,
        elapsedMs: 3_723_000,
        busy: false,
        href: '/w/ws-1/boards/b-1/-/c-1',
        onOpen,
        onStop,
      },
    });

    expect(screen.getByTestId('active-timer-title').textContent).toContain(
      'Linear Algebra Homework',
    );
    expect(screen.getByTestId('active-timer-elapsed').textContent.trim()).toBe('01:02:03');

    await fireEvent.click(screen.getByTestId('active-timer-link'));
    expect(onOpen).toHaveBeenCalledOnce();

    await fireEvent.click(screen.getByTestId('active-timer-stop'));
    expect(onStop).toHaveBeenCalledOnce();
  });
});

describe('TargetTimerButton', () => {
  it('starts with default kind when idle and stops when running', async () => {
    const onStart = vi.fn();
    const onStop = vi.fn();
    const { rerender } = render(TargetTimerButton, {
      props: {
        running: false,
        elapsedMs: 0,
        busy: false,
        defaultKind: 'lecture',
        onStart,
        onStop,
      },
    });

    await fireEvent.click(screen.getByTestId('target-timer-start'));
    expect(onStart).toHaveBeenCalledWith('lecture');

    await rerender({
      running: true,
      elapsedMs: 65_000,
      busy: false,
      defaultKind: 'lecture',
      onStart,
      onStop,
    });
    expect(screen.getByTestId('target-timer-elapsed').textContent.trim()).toBe('00:01:05');
    await fireEvent.click(screen.getByTestId('target-timer-stop'));
    expect(onStop).toHaveBeenCalledOnce();
  });
});

describe('MyTimeScreen', () => {
  it('renders entries, switches periods, filters by kind, and validates manual entry end < start', async () => {
    vi.useFakeTimers({ toFake: ['Date'], now: Date.UTC(2026, 10, 24, 14, 0, 0) });
    const { world, readerId } = seedWorld([]);
    const backend = new MemoryBackend(world, readerId);
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load(world.workspace.id);
    const tz = deviceRegional('en').timeZone;
    const dayStart = startOfDay(dateOfInstant(Date.now(), tz), tz);
    await backend.time.createTimeEntry({
      kind: 'study',
      startedAt: dayStart + 3_600_000,
      endedAt: dayStart + 5_400_000,
      note: 'Review notes',
    });
    const tracker = timeTracker(workspace);
    if (!tracker) throw new Error('expected tracker');

    render(MyTimeScreen, {
      props: {
        tracker,
        locale: 'en',
        targetHref: () => undefined,
        onOpenTarget: () => undefined,
      },
    });

    await waitFor(() => {
      expect(screen.getByTestId('time-entry-row')).toBeTruthy();
    });
    expect(screen.getByTestId('time-summary-total').textContent).toContain('30m');
    expect(screen.getByTestId('time-entry-note').textContent).toBe('Review notes');

    // Switch to lecture filter -> empty state
    await fireEvent.click(screen.getByTestId('time-filter-lecture'));
    await waitFor(() => {
      expect(screen.getByTestId('time-empty-state')).toBeTruthy();
    });

    // Switch back to all and switch period to week
    await fireEvent.click(screen.getByTestId('time-filter-all'));
    await fireEvent.click(screen.getByTestId('time-period-week'));
    await waitFor(() => {
      expect(screen.getByTestId('time-entry-row')).toBeTruthy();
    });

    // Open manual add dialog and test endedAt < startedAt inline validation
    await fireEvent.click(screen.getByTestId('time-add-button'));
    const startInput = await screen.findByTestId('time-form-started-at');
    const endInput = screen.getByTestId('time-form-ended-at');
    await fireEvent.input(startInput, { target: { value: '2026-11-24T12:00' } });
    await fireEvent.input(endInput, { target: { value: '2026-11-24T11:00' } });
    await fireEvent.click(screen.getByTestId('time-form-submit'));
    await waitFor(() => {
      expect(screen.getByTestId('time-form-error').textContent).not.toBe('');
    });

    // Editing only note on a sub-minute entry with non-zero seconds omits unchanged timestamps
    await fireEvent.click(screen.getByRole('button', { name: /cancel/i }));
    const preciseStart = dayStart + 7_200_000 + 12_345;
    const preciseEnd = dayStart + 7_200_000 + 45_678;
    const precise = await backend.time.createTimeEntry({
      kind: 'work',
      startedAt: preciseStart,
      endedAt: preciseEnd,
      note: 'Original',
    });
    await tracker.refreshAfterUnlock();
    let editButtons: HTMLElement[] = [];
    await waitFor(() => {
      editButtons = screen.getAllByTestId('time-entry-edit');
      expect(editButtons).toHaveLength(2);
    });
    const firstEdit = editButtons[0];
    if (!firstEdit) throw new Error('expected edit button');
    await fireEvent.click(firstEdit);
    const noteInput = await screen.findByTestId('time-form-note');
    await fireEvent.input(noteInput, { target: { value: 'Updated note only' } });
    await fireEvent.click(screen.getByTestId('time-form-submit'));
    await waitFor(async () => {
      const listed = await backend.time.listTimeEntries();
      const updated = listed.entries.find((item) => item.id === precise.id);
      expect(updated?.note).toBe('Updated note only');
      expect(updated?.startedAt).toBe(preciseStart);
      expect(updated?.endedAt).toBe(preciseEnd);
    });

    workspace.close();
  });

  it('reacts immediately when regional timeZone or hourCycle changes', async () => {
    // 2026-11-24T16:30:00Z is 2026-11-24 in UTC and 2026-11-25 in Asia/Seoul.
    vi.useFakeTimers({ toFake: ['Date'], now: Date.UTC(2026, 10, 24, 16, 30, 0) });
    const { world, readerId } = seedWorld([]);
    const backend = new MemoryBackend(world, readerId);
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load(world.workspace.id);

    // Entry at 2026-11-24T10:00..10:30Z (Nov 24 in UTC, Nov 24 19:00 in Seoul — NOT Nov 25 in Seoul).
    await backend.time.createTimeEntry({
      kind: 'study',
      startedAt: Date.UTC(2026, 10, 24, 10, 0, 0),
      endedAt: Date.UTC(2026, 10, 24, 10, 30, 0),
      note: 'UTC morning',
    });
    // Entry at 2026-11-25T01:00..02:00Z (Nov 25 in UTC, Nov 25 10:00 in Seoul).
    await backend.time.createTimeEntry({
      kind: 'lecture',
      startedAt: Date.UTC(2026, 10, 25, 1, 0, 0),
      endedAt: Date.UTC(2026, 10, 25, 2, 0, 0),
      note: 'Seoul next day',
    });

    const tracker = timeTracker(workspace);
    if (!tracker) throw new Error('expected tracker');

    const utcRegional: Regional = { timeZone: 'UTC', weekStart: 1, hourCycle: 'h23' };
    const seoulRegional: Regional = { timeZone: 'Asia/Seoul', weekStart: 7, hourCycle: 'h12' };

    const { rerender } = render(MyTimeScreen, {
      props: {
        tracker,
        locale: 'en',
        regional: utcRegional,
        targetHref: () => undefined,
        onOpenTarget: () => undefined,
      },
    });

    await waitFor(() => {
      expect(screen.getByTestId('time-entry-note').textContent).toBe('UTC morning');
    });
    expect(screen.getByTestId('time-period-label').textContent).toContain('24');

    await rerender({
      tracker,
      locale: 'en',
      regional: seoulRegional,
      targetHref: () => undefined,
      onOpenTarget: () => undefined,
    });

    await waitFor(() => {
      expect(screen.getByTestId('time-entry-note').textContent).toBe('Seoul next day');
    });
    expect(screen.getByTestId('time-period-label').textContent).toContain('25');

    workspace.close();
  });

  it('auto-advances today anchor across midnight rollover', async () => {
    vi.useFakeTimers({ now: Date.UTC(2026, 10, 24, 23, 59, 55) });
    const { world, readerId } = seedWorld([]);
    const backend = new MemoryBackend(world, readerId);
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load(world.workspace.id);
    const tracker = timeTracker(workspace);
    if (!tracker) throw new Error('expected tracker');

    render(MyTimeScreen, {
      props: {
        tracker,
        locale: 'en',
        regional: { timeZone: 'UTC', weekStart: 1, hourCycle: 'h23' },
        targetHref: () => undefined,
        onOpenTarget: () => undefined,
      },
    });

    await vi.waitFor(() => {
      expect(screen.getByTestId('time-period-label').textContent).toContain('24');
    });

    await vi.advanceTimersByTimeAsync(10_000);
    await vi.waitFor(() => {
      expect(screen.getByTestId('time-period-label').textContent).toContain('25');
    });

    workspace.close();
  });

  it('reports signed-out 401 on list and manual create without getting stuck in error UI', async () => {
    vi.useFakeTimers({ toFake: ['Date'], now: Date.UTC(2026, 10, 24, 14, 0, 0) });
    const { world, readerId } = seedWorld([]);
    const backend = new MemoryBackend(world, readerId);
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load(world.workspace.id);
    const tracker = timeTracker(workspace);
    if (!tracker) throw new Error('expected tracker');

    // Signed-out 401 on initial list transitions workspace to signedOut without flashing retry alert.
    vi.spyOn(backend.time, 'listTimeEntries').mockRejectedValueOnce(
      new BackendError(401, { detail: 'session expired' }),
    );
    const first = render(MyTimeScreen, {
      props: {
        tracker,
        locale: 'en',
        regional: { timeZone: 'UTC', weekStart: 1, hourCycle: 'h23' },
        targetHref: () => undefined,
        onOpenTarget: () => undefined,
      },
    });
    await waitFor(() => {
      expect(workspace.status).toBe('signedOut');
    });
    expect(screen.queryByRole('alert')).toBeNull();
    first.unmount();

    // Re-load session; signed-out 401 on manual create transitions workspace to signedOut and closes dialog.
    await workspace.load(world.workspace.id);
    vi.spyOn(backend.time, 'createTimeEntry').mockRejectedValueOnce(
      new BackendError(401, { detail: 'session expired' }),
    );

    render(MyTimeScreen, {
      props: {
        tracker,
        locale: 'en',
        regional: { timeZone: 'UTC', weekStart: 1, hourCycle: 'h23' },
        targetHref: () => undefined,
        onOpenTarget: () => undefined,
      },
    });

    await waitFor(() => {
      expect(screen.getByTestId('time-empty-state')).toBeTruthy();
    });

    await fireEvent.click(screen.getByTestId('time-add-button'));
    await screen.findByTestId('time-form-started-at');
    await fireEvent.click(screen.getByTestId('time-form-submit'));
    await waitFor(() => {
      expect(workspace.status).toBe('signedOut');
      expect(screen.queryByTestId('time-entry-dialog')).toBeNull();
    });

    workspace.close();
  });
});
