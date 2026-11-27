/**
 * UI component tests for course personal progress & member summary:
 * - `CourseProgressBar`: progress percentage/counts, ARIA valuenow/valuemax, 0-unit guard, unjoined hint
 * - `UnitProgressControls`: completion toggle for viewers, lastPosition add/edit/clear & 256-char validation, unjoined read-only state
 * - `ShareDialog`: per-member aggregate progress rate without disclosing unit details
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import CourseProgressBar from '$lib/features/course/CourseProgressBar.svelte';
import UnitProgressControls from '$lib/features/course/UnitProgressControls.svelte';
import ShareDialog from '$lib/features/shell/ShareDialog.svelte';
import { CourseStore, UNIT_PROGRESS_POSITION_MAX_CHARS, Workspace } from '$lib/state/index.ts';

import { setup } from '../../state/support.ts';

afterEach(cleanup);

describe('course progress UI components', () => {
  it('renders CourseProgressBar with ARIA progressbar, 0-unit guard, and unjoined hint', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Algorithms',
      visibility: 'open',
    });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    const first = render(CourseProgressBar, { course: store });
    expect(screen.getByTestId('course-progress-text')).toHaveTextContent('0% (0/0 units)');
    const bar = screen.getByRole('progressbar', { name: 'My progress' });
    expect(bar).toHaveAttribute('aria-valuenow', '0');
    expect(bar).toHaveAttribute('aria-valuemax', '0');

    const u1 = await store.createUnit({ kind: 'unit', title: 'Sorting' });
    await store.createUnit({ kind: 'unit', title: 'Graphs' });
    if (!u1) throw new Error('expected unit');
    await store.toggleUnitComplete(u1.id);

    await waitFor(() => {
      expect(screen.getByTestId('course-progress-text')).toHaveTextContent('50% (1/2 units)');
    });
    expect(bar).toHaveAttribute('aria-valuenow', '1');
    expect(bar).toHaveAttribute('aria-valuemax', '2');
    first.unmount();

    // Unjoined open-course reader sees join hint instead of personal progress bar.
    const otherWs = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWs.load();
    const otherStore = new CourseStore(otherWs, course.id);
    await otherStore.load();
    render(CourseProgressBar, { course: otherStore });
    expect(screen.queryByTestId('course-my-progress')).toBeNull();
    expect(screen.queryByTestId('course-progress-join-hint')).toBeNull();

    otherStore.close();
    store.close();
  });

  it('lets a viewer toggle completion and save/validate/clear lastPosition, and shows no controls unjoined', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Design Systems',
      visibility: 'open',
    });
    const u1 = await made.backend.course.createUnit(course.id, { kind: 'unit', title: 'Tokens' });

    // An unjoined reader is asked by the course's banner alone: no controls here.
    const otherWs = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWs.load();
    const otherStore = new CourseStore(otherWs, course.id);
    await otherStore.load();

    const unjoinedView = render(UnitProgressControls, { course: otherStore, unitId: u1.id });
    expect(screen.queryByTestId('unit-complete-toggle')).toBeNull();
    expect(screen.queryByTestId('unit-progress-join-hint')).toBeNull();
    expect(screen.queryByTestId('unit-last-position-add')).toBeNull();
    unjoinedView.unmount();

    // Add other as Viewer (read-only on content, but allowed to track own progress!).
    await made.backend.addMember(course.id, made.other.userId, 'viewer');
    await otherStore.load();
    expect([otherStore.role, otherStore.canEdit, otherStore.canTrackProgress]).toEqual([
      'viewer',
      false,
      true,
    ]);

    render(UnitProgressControls, { course: otherStore, unitId: u1.id });
    const toggle = screen.getByTestId('unit-complete-toggle');
    expect(toggle).not.toBeDisabled();
    expect(toggle).toHaveAttribute('aria-pressed', 'false');

    await fireEvent.click(toggle);
    await waitFor(() => {
      expect(toggle).toHaveAttribute('aria-pressed', 'true');
      expect(toggle).toHaveTextContent('Completed');
    });

    // Save lastPosition: > 256 chars rejected inline, valid value saved, then cleared.
    await fireEvent.click(screen.getByTestId('unit-last-position-add'));
    const input = screen.getByTestId('unit-last-position-input');
    await fireEvent.input(input, {
      target: { value: 'x'.repeat(UNIT_PROGRESS_POSITION_MAX_CHARS + 1) },
    });
    await fireEvent.click(screen.getByTestId('unit-last-position-save'));
    expect(screen.getByTestId('unit-last-position-error')).toBeVisible();

    await fireEvent.input(input, { target: { value: '  p.42  ' } });
    await fireEvent.click(screen.getByTestId('unit-last-position-save'));
    await waitFor(() => {
      expect(screen.getByTestId('unit-last-position-value')).toHaveTextContent('p.42');
    });

    await fireEvent.click(screen.getByTestId('unit-last-position-clear'));
    await waitFor(() => {
      expect(screen.getByTestId('unit-last-position-add')).toBeVisible();
    });

    otherStore.close();
  });

  it('displays aggregate member completion rates in ShareDialog without exposing unit details', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Shared Study',
      visibility: 'private',
    });
    await made.backend.addMember(course.id, made.other.userId, 'viewer');
    const u1 = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Secret Unit Title',
    });
    await made.backend.course.createUnit(course.id, { kind: 'unit', title: 'Second Unit' });
    await made.backend.course.putUnitProgress(course.id, u1.id, {
      completed: true,
      lastPosition: 'secret-bookmark',
    });

    const store = new CourseStore(made.workspace, course.id);
    await store.load();

    render(ShareDialog, {
      open: true,
      workspace: made.workspace,
      shareable: store,
      linkOf: (token: string) => `https://example.test/shared/${course.id}?r=${token}`,
      memberProgress: (userId: string) => {
        const summary = store.progressSummary;
        const entry = summary?.members.find((m) => m.userId === userId);
        if (!summary || !entry) return undefined;
        const pct =
          summary.totalUnits > 0
            ? Math.round((entry.completedUnits / summary.totalUnits) * 100)
            : 0;
        return `${pct}% (${entry.completedUnits}/${summary.totalUnits} units)`;
      },
    });

    const dialog = screen.getByTestId('share-dialog');
    const badges = dialog.querySelectorAll('[data-testid="member-course-progress"]');
    expect(
      Array.from(badges)
        .map((el) => el.textContent.trim())
        .sort(),
    ).toEqual(['0% (0/2 units)', '50% (1/2 units)']);
    expect(dialog.textContent).not.toContain('Secret Unit Title');
    expect(dialog.textContent).not.toContain('secret-bookmark');

    store.close();
  });
});
