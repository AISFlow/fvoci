/**
 * UI component tests for spaced-repetition unit reviews, personal study notes,
 * workspace "Today's reviews", and `review_due` notification navigation.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import NotificationsHost from '../../../src/app/NotificationsHost.svelte';
import { Router } from '../../../src/app/router.svelte.ts';
import TodayReviewsPage from '../../../src/app/TodayReviewsPage.svelte';
import TodayReviewsView from '$lib/features/course/TodayReviewsView.svelte';
import UnitNotesPanel from '$lib/features/course/UnitNotesPanel.svelte';
import UnitProgressControls from '$lib/features/course/UnitProgressControls.svelte';
import UnitReviewsPanel from '$lib/features/course/UnitReviewsPanel.svelte';
import { UnitOpenings } from '$lib/features/course/unit-openings.svelte.ts';
import { addDays, dateOfInstant } from '$lib/logic/dates.ts';
import {
  courseReviews,
  CourseStore,
  todayReviews,
  Workspace,
  type TodayReviewItem,
} from '$lib/state/index.ts';
import { noteReviewDue } from '$lib/state/memory/notifications.ts';

import { setup } from '../../state/support.ts';

afterEach(() => {
  vi.useRealTimers();
  cleanup();
});

describe('course unit reviews & notes UI', () => {
  it('shows interval suggestion bar on unit completion with suggested interval highlighted and schedules on confirm', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Algorithms',
      visibility: 'open',
    });
    const u1 = await made.backend.course.createUnit(course.id, { kind: 'unit', title: 'Sorting' });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    render(UnitProgressControls, { course: store, unitId: u1.id });
    await fireEvent.click(screen.getByTestId('unit-complete-toggle'));

    const suggestBar = await screen.findByTestId('review-suggest-bar');
    expect(suggestBar).toBeVisible();
    const recommended = screen.getByTestId('review-suggest-bar-option-1');
    expect(recommended).toHaveAttribute('data-suggested', 'true');
    expect(recommended).toHaveAttribute('aria-pressed', 'true');

    await fireEvent.click(screen.getByTestId('review-suggest-bar-option-3'));
    await fireEvent.click(screen.getByTestId('review-suggest-bar-confirm'));

    await waitFor(() => {
      expect(screen.queryByTestId('review-suggest-bar')).toBeNull();
      expect(screen.getByTestId('review-scheduled-notice')).toBeVisible();
    });

    const reviews = await made.backend.course.reviews.listMyCourseReviews(course.id);
    expect(reviews).toHaveLength(1);
    expect(reviews[0]?.intervalDays).toBe(3);

    store.close();
  });

  it('lists, completes with next interval, and deletes reviews in UnitReviewsPanel; shows join hint when unjoined', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Systems',
      visibility: 'open',
    });
    const u1 = await made.backend.course.createUnit(course.id, { kind: 'unit', title: 'Threads' });
    const today = dateOfInstant(made.world.now(), 'UTC');
    await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: today,
      intervalDays: 1,
    });

    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    const first = render(UnitReviewsPanel, {
      course: store,
      unitId: u1.id,
      openings: new UnitOpenings(),
    });
    await waitFor(() => {
      expect(screen.getByTestId('unit-reviews-pending')).toBeVisible();
    });

    await fireEvent.click(screen.getByTestId('unit-review-complete'));
    const picker = await screen.findByTestId('unit-review-complete-picker');
    expect(picker).toBeVisible();
    expect(screen.getByTestId('unit-review-complete-picker-option-3')).toHaveAttribute(
      'data-suggested',
      'true',
    );
    await fireEvent.click(screen.getByTestId('unit-review-complete-picker-confirm'));

    await waitFor(() => {
      expect(screen.getByTestId('unit-reviews-completed')).toBeInTheDocument();
      expect(screen.getByText('Completed (1)')).toBeVisible();
      expect(courseReviews(store).reviews).toHaveLength(2);
    });
    first.unmount();

    // Unjoined open-course reader sees join hint only.
    const otherWs = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWs.load();
    const otherStore = new CourseStore(otherWs, course.id);
    await otherStore.load();

    render(UnitReviewsPanel, { course: otherStore, unitId: u1.id, openings: new UnitOpenings() });
    expect(screen.queryByTestId('unit-reviews-panel')).toBeNull();

    otherStore.close();
    store.close();
  });

  it('creates, filters, edits (kind & content), validates, and deletes personal notes in UnitNotesPanel', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Compilers',
      visibility: 'private',
    });
    const u1 = await made.backend.course.createUnit(course.id, { kind: 'unit', title: 'Parsing' });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    render(UnitNotesPanel, { course: store, unitId: u1.id });
    // Without notes, only the button that starts one.
    expect(screen.queryByTestId('unit-notes-panel')).toBeNull();
    await fireEvent.click(screen.getByTestId('unit-notes-add'));
    expect(screen.getByTestId('unit-notes-private-badge')).toBeVisible();
    expect(screen.getByTestId('unit-note-input')).toHaveFocus();
    expect(screen.queryByTestId('unit-notes-filter-all')).toBeNull();

    // Empty submit triggers inline validation error.
    await fireEvent.click(screen.getByTestId('unit-note-submit'));
    expect(screen.getByTestId('unit-note-error')).toBeVisible();

    // Create question note and mistake note.
    await fireEvent.input(screen.getByTestId('unit-note-input'), {
      target: { value: 'Difference between LL(1) and LR(1)?' },
    });
    await fireEvent.click(screen.getByTestId('unit-note-submit'));
    await waitFor(() => {
      expect(screen.getAllByTestId('unit-note-item')).toHaveLength(1);
    });

    await fireEvent.click(screen.getByTestId('unit-note-kind-mistake'));
    await fireEvent.input(screen.getByTestId('unit-note-input'), {
      target: { value: 'Missed shift/reduce conflict' },
    });
    await fireEvent.click(screen.getByTestId('unit-note-submit'));
    await waitFor(() => {
      expect(screen.getAllByTestId('unit-note-item')).toHaveLength(2);
    });

    // Filter to mistake only.
    await fireEvent.click(screen.getByTestId('unit-notes-filter-mistake'));
    expect(screen.getAllByTestId('unit-note-item')).toHaveLength(1);
    expect(screen.getByTestId('unit-note-content')).toHaveTextContent(
      'Missed shift/reduce conflict',
    );

    // Edit mistake note into review note with new content.
    await fireEvent.click(screen.getByTestId('unit-note-edit'));
    await fireEvent.click(screen.getByTestId('unit-note-edit-kind-review'));
    await fireEvent.input(screen.getByTestId('unit-note-edit-input'), {
      target: { value: 'Resolved conflict via precedence' },
    });
    await fireEvent.click(screen.getByTestId('unit-note-save'));
    await waitFor(() => {
      expect(screen.getByTestId('unit-notes-empty')).toBeVisible();
    });

    await fireEvent.click(screen.getByTestId('unit-notes-filter-review'));
    expect(screen.getByTestId('unit-note-content')).toHaveTextContent(
      'Resolved conflict via precedence',
    );

    await fireEvent.click(screen.getByTestId('unit-note-delete'));
    await waitFor(() => {
      expect(screen.getByTestId('unit-notes-empty')).toBeVisible();
    });

    store.close();
  });

  it('renders overdue & today items in TodayReviewsView, opens unit, and completes inline', async () => {
    const made = setup();
    await made.workspace.load();
    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'Physics',
      visibility: 'private',
    });
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Kinematics',
    });
    const today = dateOfInstant(made.world.now(), 'UTC');
    await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: addDays(today, -3),
      intervalDays: 1,
    });

    const onOpenUnit = vi.fn();
    render(TodayReviewsView, {
      store: todayReviews(made.workspace),
      workspaceId: wsId,
      unitHref: (item: TodayReviewItem) => `/w/${wsId}/courses/${item.containerId}/${item.unitId}`,
      onOpenUnit,
    });

    await waitFor(() => {
      expect(screen.getByTestId('today-reviews-list')).toBeVisible();
    });
    expect(screen.getByTestId('today-review-overdue')).toHaveTextContent('3d overdue');

    await fireEvent.click(screen.getByTestId('today-review-go'));
    expect(onOpenUnit).toHaveBeenCalledTimes(1);

    await fireEvent.click(screen.getByTestId('today-review-complete'));
    await fireEvent.click(screen.getByTestId('today-review-complete-picker-confirm'));
    await waitFor(() => {
      expect(screen.getByTestId('today-reviews-empty')).toBeVisible();
      expect(screen.getByTestId('today-reviews-status')).toBeVisible();
    });
  });

  it('navigates directly to the course unit when clicking a review_due notification', async () => {
    const made = setup();
    await made.workspace.load();
    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'Biology',
      visibility: 'private',
    });
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Cell Cycle',
    });
    noteReviewDue(made.world, made.backend.userId, course.id, u1.id);

    const router = new Router('');
    router.go({ workspaceId: wsId });

    render(NotificationsHost, { workspace: made.workspace, router });
    await waitFor(() => {
      expect(screen.getByTestId('unread-notifications')).toHaveTextContent('1');
    });

    await fireEvent.click(screen.getByTestId('open-notifications'));
    const link = await screen.findByRole('link', { name: /Cell Cycle/ });
    expect(link).toHaveAttribute('href', `/w/${wsId}/courses/${course.id}/${u1.id}`);

    await fireEvent.click(link);
    await waitFor(() => {
      expect(router.route).toMatchObject({ kind: 'course', courseId: course.id, unitId: u1.id });
    });
  });

  it('validates custom interval days, supports date-only scheduling, and respects custom course intervals in TodayReviewsView', async () => {
    const made = setup();
    await made.workspace.load();
    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'Custom Intervals',
      visibility: 'private',
    });
    const storedCourse = made.world.containers.get(course.id);
    if (storedCourse) {
      (storedCourse.properties as Record<string, unknown>).defaultReviewIntervals = [2, 5, 12];
    }
    const u1 = await made.backend.course.createUnit(course.id, { kind: 'unit', title: 'Unit X' });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    // A unit without reviews has no panel until its menu plans one.
    const openings = new UnitOpenings();
    const first = render(UnitReviewsPanel, { course: store, unitId: u1.id, openings });
    expect(screen.queryByTestId('unit-reviews-panel')).toBeNull();
    openings.review = u1.id;
    await fireEvent.click(await screen.findByTestId('unit-review-manual-picker-custom'));

    // Invalid custom days -> localized inline error without crashing.
    const daysInput = screen.getByTestId('unit-review-manual-picker-custom-days');
    await fireEvent.input(daysInput, { target: { value: '0' } });
    await fireEvent.click(screen.getByTestId('unit-review-manual-picker-confirm'));
    expect(screen.getByTestId('unit-review-manual-picker-error')).toBeVisible();
    expect(daysInput).toHaveAttribute('aria-invalid', 'true');

    // Valid custom days -> schedules successfully.
    await fireEvent.input(daysInput, { target: { value: '9' } });
    await fireEvent.click(screen.getByTestId('unit-review-manual-picker-confirm'));
    await waitFor(() => {
      expect(courseReviews(store).reviews).toHaveLength(1);
    });
    expect(courseReviews(store).reviews[0]?.intervalDays).toBe(9);

    // Date-only scheduling derives intervalDays from the date (does not send stale suggested interval).
    const today = dateOfInstant(made.world.now(), 'UTC');
    const inFiveDays = addDays(today, 5);
    await fireEvent.click(screen.getByTestId('unit-review-add-toggle'));
    await fireEvent.click(screen.getByTestId('unit-review-manual-picker-custom'));
    await fireEvent.input(screen.getByTestId('unit-review-manual-picker-custom-date'), {
      target: { value: inFiveDays },
    });
    await fireEvent.click(screen.getByTestId('unit-review-manual-picker-confirm'));
    await waitFor(() => {
      expect(courseReviews(store).reviews).toHaveLength(2);
    });
    expect(
      courseReviews(store).reviews.some(
        (r) => r.scheduledDate === inFiveDays && r.intervalDays === 5,
      ),
    ).toBe(true);
    first.unmount();

    // TodayReviewsView respects course defaultReviewIntervals ([2, 5, 12] -> after 2d suggests 5d).
    await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: today,
      intervalDays: 2,
    });
    await made.workspace.load();
    render(TodayReviewsView, {
      store: todayReviews(made.workspace),
      workspaceId: wsId,
      unitHref: (item: TodayReviewItem) => `/w/${wsId}/courses/${item.containerId}/${item.unitId}`,
      onOpenUnit: vi.fn(),
    });
    await waitFor(() => {
      expect(screen.getByTestId('today-reviews-list')).toBeVisible();
    });
    await fireEvent.click(screen.getByTestId('today-review-complete'));
    expect(screen.getByTestId('today-review-complete-picker-option-5')).toHaveAttribute(
      'data-suggested',
      'true',
    );
    expect(screen.getByTestId('today-review-complete-picker-option-12')).toBeVisible();

    store.close();
  });

  it('reads TodayReviewsPage anew when settings.timeZone changes', async () => {
    // 2026-11-24T16:30:00Z is 2026-11-24 in UTC and 2026-11-25 in Asia/Seoul.
    const made = setup();
    made.world.useClock(() => Date.UTC(2026, 10, 24, 16, 30, 0));
    await made.workspace.load();
    const wsId = made.world.workspace.id;
    const course = await made.backend.course.createCourse(wsId, {
      title: 'TZ Course',
      visibility: 'private',
    });
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Seoul Due',
    });
    // Schedule for 2026-11-25: NOT due yet in UTC, DUE TODAY in Asia/Seoul.
    await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: '2026-11-25',
      intervalDays: 1,
    });

    const router = new Router('');
    router.go({ workspaceId: wsId, reviewsToday: true });
    render(TodayReviewsPage, { workspace: made.workspace, router });

    // Explicitly switch to UTC -> 2026-11-25 review is in the future (empty).
    await made.workspace.changeSettings({ timeZone: 'UTC' });
    await waitFor(() => {
      expect(screen.getByTestId('today-reviews-empty')).toBeVisible();
    });

    // Switch to Asia/Seoul -> TodayReviewsPage automatically reloads and shows the due-today item.
    await made.workspace.changeSettings({ timeZone: 'Asia/Seoul' });
    await waitFor(() => {
      expect(screen.getByTestId('today-reviews-list')).toBeVisible();
      expect(screen.getByTestId('today-review-open')).toHaveTextContent('Seoul Due');
      expect(screen.getByTestId('today-review-due-today')).toBeVisible();
    });
  });

  it('updates due-today badge to overdue automatically across midnight in UnitReviewsPanel', async () => {
    vi.useFakeTimers({ now: Date.UTC(2026, 10, 24, 23, 59, 55) });
    const made = setup();
    made.world.useClock(() => Date.now());
    await made.workspace.load();
    await made.workspace.changeSettings({ timeZone: 'UTC' });
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Midnight Course',
      visibility: 'private',
    });
    const u1 = await made.backend.course.createUnit(course.id, { kind: 'unit', title: 'Rollover' });
    await made.backend.course.reviews.scheduleUnitReview(course.id, u1.id, {
      scheduledDate: '2026-11-24',
      intervalDays: 1,
    });

    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    render(UnitReviewsPanel, { course: store, unitId: u1.id, openings: new UnitOpenings() });
    await vi.waitFor(() => {
      expect(screen.getByTestId('unit-review-due-today')).toBeVisible();
    });

    await vi.advanceTimersByTimeAsync(10_000);
    await vi.waitFor(() => {
      expect(screen.getByTestId('unit-review-overdue')).toHaveTextContent('1d overdue');
    });

    store.close();
  });
});
