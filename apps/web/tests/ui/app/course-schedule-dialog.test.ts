/**
 * Regression tests for:
 * - `CourseScheduleDialog` opened while `course.schedule.view` is still `null` (loading)
 *   populating inputs once `view` arrives without wiping edits on subsequent refreshes.
 * - `DeviceToday` rescheduling its midnight timer when the reactive `timeZone` changes.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import CourseScheduleDialog from '$lib/features/course/CourseScheduleDialog.svelte';
import { CourseStore, STUDY_DAYS_WEEKDAYS, type CourseScheduleView } from '$lib/state/index.ts';

import { setup } from '../../state/support.ts';
import { mountDeviceTodayProbe } from './device-today-probe.svelte.ts';

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

describe('CourseScheduleDialog loading-open snapshot', () => {
  it('populates inputs when schedule view arrives after opening and preserves edits across background refreshes', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Delayed Schedule',
      visibility: 'private',
    });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();
    // Simulate still-loading state where schedule view has not arrived yet.
    store.schedule.take(null);

    render(CourseScheduleDialog, { open: true, course: store });

    const startInput = await screen.findByTestId<HTMLInputElement>('schedule-start-date');
    const targetInput = screen.getByTestId<HTMLInputElement>('schedule-target-date');
    const saveButton = screen.getByTestId<HTMLButtonElement>('schedule-save');

    // While view is null, inputs are blank and save is disabled.
    expect(startInput.value).toBe('');
    expect(saveButton.disabled).toBe(true);

    // Schedule view arrives -> dialog populates snapshot and enables save.
    const arrived: CourseScheduleView = {
      schedule: {
        startDate: '2026-11-10',
        targetDate: '2026-11-28',
        studyDays: STUDY_DAYS_WEEKDAYS,
        updatedAt: 1,
      },
      today: '2026-11-24',
      todayCount: 1,
      overdueCount: 0,
      units: [],
    };
    store.schedule.take(arrived);

    await waitFor(() => {
      expect(startInput.value).toBe('2026-11-10');
      expect(targetInput.value).toBe('2026-11-28');
      expect(saveButton.disabled).toBe(false);
    });

    // User edits target date; a subsequent background refresh must NOT wipe edits.
    await fireEvent.input(targetInput, { target: { value: '2026-12-05' } });
    expect(targetInput.value).toBe('2026-12-05');

    store.schedule.take({
      ...arrived,
      todayCount: 2,
    });
    await waitFor(() => {
      expect(screen.getByTestId('schedule-dialog-today-count')).toHaveTextContent('2');
    });
    expect(targetInput.value).toBe('2026-12-05');

    store.close();
  });
});

describe('DeviceToday timezone rescheduling', () => {
  it('reschedules its midnight timer when the reactive timeZone changes', async () => {
    // 2026-11-24T14:59:55Z is 14:59:55 in UTC (9h until midnight) and 23:59:55 in Asia/Seoul (5s until midnight).
    vi.useFakeTimers({ now: Date.UTC(2026, 10, 24, 14, 59, 55) });
    const probe = mountDeviceTodayProbe('UTC');
    expect(probe.seen).toEqual(['2026-11-24']);

    // Switch timezone to Asia/Seoul: date is still 2026-11-24 (23:59:55 KST), timer must reschedule to 5s.
    probe.setZone('Asia/Seoul');
    expect(probe.seen).toEqual(['2026-11-24', '2026-11-24']);

    // Advance 6 seconds -> crosses midnight in Asia/Seoul (2026-11-25 00:00:01 KST) and notifies subscriber.
    await vi.advanceTimersByTimeAsync(6_000);
    expect(probe.seen.at(-1)).toBe('2026-11-25');

    probe.stop();
  });
});
