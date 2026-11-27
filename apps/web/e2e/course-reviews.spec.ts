/**
 * Spaced-repetition unit reviews, interval suggestions, "Today's reviews",
 * personal study notes, and public/user isolation on the in-memory backend.
 */
import { expect, test, type Page } from '@playwright/test';

import { WORKSPACE } from './address.ts';
import { openApp, showSidebar, sidebar } from './shell.support.ts';
import { goInside } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });
test.use({ timezoneId: 'UTC' });

interface DevReviews {
  scheduleUnitReview(
    courseId: string,
    unitId: string,
    request: { intervalDays?: number; scheduledDate?: string },
  ): Promise<{ id: string }>;
  createUnitNote(
    courseId: string,
    unitId: string,
    request: { kind: 'question' | 'mistake' | 'review'; content: string },
  ): Promise<{ id: string }>;
}

interface DevCourse {
  createCourse(
    workspaceId: string,
    draft: { title: string; visibility: 'open' | 'private' },
  ): Promise<{ id: string }>;
  createUnit(
    courseId: string,
    draft: { kind?: 'unit' | 'chapter'; title?: string; parentId?: string },
  ): Promise<{ id: string }>;
  reviews: DevReviews;
}

interface Dev {
  backend: {
    userId: string;
    course: DevCourse;
    world: {
      now(): number;
      newId(): string;
      blocks: Map<string, { id: string; containerId: string; type: string; title: string }>;
      containers: Map<string, { id: string; workspaceId: string; kind?: string }>;
      notifications: Map<string, unknown[]>;
      tellUsers(userIds: readonly string[], change: { kind: 'notifications' }): void;
    };
  };
  someoneElse(): { userId: string; course: DevCourse };
}

function isoDateOffset(daysOffset: number): string {
  const now = new Date(Date.now() + daysOffset * 24 * 60 * 60 * 1000);
  const yyyy = String(now.getUTCFullYear()).padStart(4, '0');
  const mm = String(now.getUTCMonth() + 1).padStart(2, '0');
  const dd = String(now.getUTCDate()).padStart(2, '0');
  return `${yyyy}-${mm}-${dd}`;
}

async function openCourseUnit(page: Page, courseId: string, unitId: string): Promise<void> {
  await goInside(page, `/w/${WORKSPACE}/courses/${courseId}/${unitId}`);
  await expect(page.getByTestId('unit-view')).toBeVisible();
}

test('completes unit with interval suggestion, manages Today reviews, CRUDs personal notes, and navigates from review_due notification', async ({
  page,
}) => {
  await openApp(page);

  const created = await page.evaluate(async (wsId) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const course = await dev.backend.course.createCourse(wsId, {
      title: 'Spaced Repetition 101',
      visibility: 'private',
    });
    const u1 = await dev.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Active Recall',
    });
    const u2 = await dev.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Interleaving',
    });
    return { courseId: course.id, u1Id: u1.id, u2Id: u2.id };
  }, WORKSPACE);

  await openCourseUnit(page, created.courseId, created.u1Id);

  // 1. Complete unit -> interval suggestion bar appears with 1d recommended -> schedule 3d.
  await page.getByTestId('unit-complete-toggle').click();
  const suggestBar = page.getByTestId('review-suggest-bar');
  await expect(suggestBar).toBeVisible();
  await expect(suggestBar.getByTestId('review-suggest-bar-option-1')).toHaveAttribute(
    'data-suggested',
    'true',
  );
  await suggestBar.getByTestId('review-suggest-bar-option-3').click();
  await suggestBar.getByTestId('review-suggest-bar-confirm').click();
  await expect(suggestBar).toBeHidden();

  const reviewsPanel = page.getByTestId('unit-reviews-panel');
  await expect(reviewsPanel.getByTestId('unit-review-item')).toHaveCount(1);

  // 2. Personal notes CRUD + filtering + private badge; without notes, only the button that starts one.
  const notesPanel = page.getByTestId('unit-notes-panel');
  await expect(notesPanel).toHaveCount(0);
  await page.getByTestId('unit-notes-add').click();
  await expect(notesPanel.getByTestId('unit-notes-private-badge')).toBeVisible();

  await notesPanel.getByTestId('unit-note-kind-question').click();
  await notesPanel.getByTestId('unit-note-input').fill('When should I test myself?');
  await notesPanel.getByTestId('unit-note-submit').click();
  await expect(notesPanel.getByTestId('unit-note-item')).toHaveCount(1);

  await notesPanel.getByTestId('unit-note-kind-mistake').click();
  await notesPanel.getByTestId('unit-note-input').fill('Passive re-reading did not stick');
  await notesPanel.getByTestId('unit-note-submit').click();
  await expect(notesPanel.getByTestId('unit-note-item')).toHaveCount(2);

  await notesPanel.getByTestId('unit-notes-filter-mistake').click();
  await expect(notesPanel.getByTestId('unit-note-item')).toHaveCount(1);
  await expect(notesPanel.getByTestId('unit-note-content')).toHaveText(
    'Passive re-reading did not stick',
  );

  // Edit mistake note into review note.
  await notesPanel.getByTestId('unit-note-edit').click();
  await notesPanel.getByTestId('unit-note-edit-kind-review').click();
  await notesPanel.getByTestId('unit-note-edit-input').fill('Flashcards worked much better');
  await notesPanel.getByTestId('unit-note-save').click();
  await expect(notesPanel.getByTestId('unit-notes-empty')).toBeVisible();

  await notesPanel.getByTestId('unit-notes-filter-review').click();
  await expect(notesPanel.getByTestId('unit-note-content')).toHaveText(
    'Flashcards worked much better',
  );
  await notesPanel.getByTestId('unit-note-delete').click();
  await expect(notesPanel.getByTestId('unit-notes-empty')).toBeVisible();

  // 3. Schedule an overdue review on Unit 2 -> open "Today's reviews" via sidebar <button>.
  const pastDate = isoDateOffset(-2);
  await page.evaluate(
    async ({ courseId, u2Id, scheduledDate }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      await dev.backend.course.reviews.scheduleUnitReview(courseId, u2Id, {
        scheduledDate,
        intervalDays: 1,
      });
    },
    { courseId: created.courseId, u2Id: created.u2Id, scheduledDate: pastDate },
  );

  await showSidebar(page);
  await sidebar(page).getByTestId('open-today-reviews').click();
  await expect(page.getByTestId('today-reviews-screen')).toBeVisible();
  await expect(page.getByTestId('today-review-item')).toHaveCount(1);
  await expect(page.getByTestId('today-review-overdue')).toContainText('overdue');

  // Complete review from Today's reviews with next interval scheduled.
  await page.getByTestId('today-review-complete').click();
  await page.getByTestId('today-review-complete-picker-confirm').click();
  await expect(page.getByTestId('today-reviews-empty')).toBeVisible();
  await expect(page.getByTestId('today-reviews-status')).toBeVisible();

  // 4. `review_due` notification jumps straight to Unit 2.
  await page.evaluate(
    ({ courseId, u2Id }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const world = dev.backend.world;
      const now = world.now();
      const theirs = world.notifications.get(dev.backend.userId) ?? [];
      theirs.push({
        id: world.newId(),
        recipientId: dev.backend.userId,
        kind: 'review_due',
        blockId: u2Id,
        containerId: courseId,
        commentId: '',
        actorId: '',
        doers: [],
        times: 1,
        createdAt: now,
        updatedAt: now,
        readAt: 0,
      });
      world.notifications.set(dev.backend.userId, theirs);
      world.tellUsers([dev.backend.userId], { kind: 'notifications' });
    },
    { courseId: created.courseId, u2Id: created.u2Id },
  );

  const openBell = page.getByTestId('open-notifications');
  if (await openBell.isVisible()) {
    await openBell.click();
  } else {
    await page.getByTestId('bar-menu').click();
    await page.getByRole('menuitem', { name: /Notifications/i }).click();
  }
  const notifLink = page.getByTestId('notifications').getByRole('link', { name: /Interleaving/ });
  await expect(notifLink).toBeVisible();
  await notifLink.click();
  await expect(page).toHaveURL(new RegExp(`/courses/${created.courseId}/${created.u2Id}$`));
  await expect(page.getByTestId('unit-title')).toHaveValue('Interleaving');
});

test('isolates unjoined open-course reader and public shared links from reviews and notes', async ({
  page,
}) => {
  await openApp(page);

  const openCourse = await page.evaluate(async (wsId) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const other = dev.someoneElse();
    const course = await other.course.createCourse(wsId, {
      title: 'Open Course Privacy',
      visibility: 'open',
    });
    const u1 = await other.course.createUnit(course.id, { kind: 'unit', title: 'Lesson 1' });
    await other.course.reviews.scheduleUnitReview(course.id, u1.id, { intervalDays: 1 });
    await other.course.reviews.createUnitNote(course.id, u1.id, {
      kind: 'mistake',
      content: 'Secret mistake only for creator',
    });
    return { courseId: course.id, unitId: u1.id };
  }, WORKSPACE);

  await openCourseUnit(page, openCourse.courseId, openCourse.unitId);
  await expect(page.getByTestId('unit-reviews-join-hint')).toHaveCount(0);
  await expect(page.getByTestId('unit-notes-join-hint')).toHaveCount(0);
  await expect(page.getByTestId('course-screen')).not.toContainText(
    'Secret mistake only for creator',
  );

  // Public shared course makes zero /reviews, /notes, or /learning/reviews/today calls.
  const sharedCourseId = '01930000-0000-7000-8000-00000000cc01';
  const sharedUnitId = '01930000-0000-7000-8000-00000000cc02';
  const forbiddenCalls: string[] = [];
  await page.route('**/api/v1/**', async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (
      pathname.includes('/reviews') ||
      pathname.includes('/notes') ||
      pathname.includes('/learning/')
    ) {
      forbiddenCalls.push(pathname);
      await route.fulfill({ status: 500, json: { detail: 'forbidden on shared link' } });
      return;
    }
    if (pathname === `/api/v1/shared/${sharedCourseId}`) {
      await route.fulfill({
        json: {
          id: sharedCourseId,
          workspaceId: '',
          kind: 'course',
          title: 'Public Study',
          visibility: 'private',
          icon: '',
          description: '',
          cardProperties: [],
          properties: {},
          createdBy: '',
          modifiedBy: '',
          createdAt: 1,
          updatedAt: 1,
        },
      });
      return;
    }
    if (pathname === `/api/v1/shared/${sharedCourseId}/units`) {
      await route.fulfill({
        json: [
          {
            id: sharedUnitId,
            kind: 'unit',
            parentId: '',
            title: 'Public Lesson',
            order: 'a',
            createdAt: 1,
            updatedAt: 1,
          },
        ],
      });
      return;
    }
    if (pathname === `/api/v1/shared/${sharedCourseId}/units/${sharedUnitId}`) {
      await route.fulfill({
        json: {
          id: sharedUnitId,
          courseId: sharedCourseId,
          kind: 'unit',
          parentId: '',
          title: 'Public Lesson',
          order: 'a',
          body: null,
          createdBy: '',
          modifiedBy: '',
          createdAt: 1,
          updatedAt: 1,
        },
      });
      return;
    }
    await route.fulfill({ status: 404, json: { detail: 'not found' } });
  });

  await page.goto(`/shared/${sharedCourseId}?r=tok`);
  await expect(page.getByTestId('shared-course')).toBeVisible();
  await expect(page.getByTestId('unit-title')).toHaveText('Public Lesson');
  await expect(page.getByTestId('unit-reviews-panel')).toHaveCount(0);
  await expect(page.getByTestId('unit-notes-panel')).toHaveCount(0);
  await expect(page.getByTestId('review-suggest-bar')).toHaveCount(0);
  expect(forbiddenCalls).toEqual([]);
});
