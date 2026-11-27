/**
 * Course study schedules, unit study amounts, manual shared/personal
 * redistribution, personal planned dates, and "Today's learning" end to end
 * on the in-memory backend:
 * - Editor sets unit amounts (minutes/pages/problems) and saves a schedule with
 *   auto-assign; overdue units stay overdue (no automatic shifting) and show
 *   badges in the outline tree (chapters omitted).
 * - Enrolled learner adjusts & resets personal plannedDate and redistributes
 *   remaining incomplete units without affecting shared dates.
 * - "Today's learning" view opens from the sidebar button (without changing
 *   sidebar link count), shows overdue & today sections, navigates to units,
 *   and allows one-click completion.
 * - Public shared course (`/shared/...`) renders zero schedule UI and makes
 *   zero `/schedule` or `/learning/today` requests.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { WORKSPACE } from './address.ts';
import { chooseFromMenu, openApp, showSidebar, sidebar, stageOf, strip } from './shell.support.ts';
import { goInside } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });
test.use({ timezoneId: 'UTC' });

const ID = '[0-9a-f-]{36}';
const ON_A_COURSE = new RegExp(`/w/${ID}/courses/(${ID})(/(${ID}))?$`);

interface DevCourse {
  createCourse(
    workspaceId: string,
    draft: { title: string; visibility: 'open' | 'private'; minimumRole?: 'viewer' | 'editor' },
  ): Promise<{ id: string }>;
  createUnit(
    courseId: string,
    draft: {
      kind?: 'unit' | 'chapter';
      title?: string;
      parentId?: string;
      amount?: number;
      amountUnit?: 'minutes' | 'pages' | 'problems';
      scheduledDate?: string;
    },
  ): Promise<{ id: string }>;
  putSchedule(
    courseId: string,
    update: { startDate: string; targetDate: string; studyDays: number; autoAssign?: boolean },
  ): Promise<unknown>;
  putUnitProgress(
    courseId: string,
    unitId: string,
    update: { completed?: boolean },
  ): Promise<unknown>;
  putUnitSchedule(
    courseId: string,
    unitId: string,
    update: { plannedDate: string | null },
  ): Promise<unknown>;
}

interface Dev {
  backend: {
    userId: string;
    addMember(containerId: string, userId: string, role: string): Promise<unknown>;
    course: DevCourse;
  };
  someoneElse(): { userId: string; course: DevCourse };
}

const unitTree = (page: Page): Locator => page.getByTestId('unit-tree');

async function showTree(page: Page): Promise<void> {
  const opener = page.getByTestId('open-unit-tree');
  if ((await opener.isVisible()) && (await opener.getAttribute('aria-expanded')) !== 'true') {
    await opener.click();
  }
  await expect(unitTree(page)).toBeVisible();
}

async function hideTree(page: Page): Promise<void> {
  const opener = page.getByTestId('open-unit-tree');
  if (!(await opener.isVisible()) || (await opener.getAttribute('aria-expanded')) !== 'true') {
    return;
  }
  await page
    .getByTestId('unit-tree-panel')
    .getByRole('button', { name: 'Close the units' })
    .click();
  await expect(opener).toHaveAttribute('aria-expanded', 'false');
}

function isoDateOffset(daysOffset: number): string {
  const date = new Date(Date.now() + daysOffset * 86_400_000);
  const year = date.getUTCFullYear();
  const month = String(date.getUTCMonth() + 1).padStart(2, '0');
  const day = String(date.getUTCDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

async function openLearningToday(page: Page): Promise<void> {
  const stage = await stageOf(page);
  if (stage === 'medium' && (await strip(page).isVisible())) {
    await strip(page).getByTestId('strip-open-learning-today').click();
  } else {
    await showSidebar(page);
    await sidebar(page).getByTestId('open-learning-today').click();
  }
  await expect(page.getByTestId('learning-today-view')).toBeVisible();
}

test('configures amounts & schedule, preserves overdue units without auto-shift, redistributes personally, and uses Today’s learning', async ({
  page,
}) => {
  await openApp(page);

  const courseId = await page.evaluate(async (wsId) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const course = await dev.backend.course.createCourse(wsId, {
      title: 'Distributed Systems',
      visibility: 'open',
    });
    const ch = await dev.backend.course.createUnit(course.id, {
      kind: 'chapter',
      title: 'Foundations',
    });
    await dev.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Logical Clocks',
      parentId: ch.id,
    });
    await dev.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Raft Consensus',
      parentId: ch.id,
      amount: 20,
      amountUnit: 'pages',
    });
    await dev.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Consistency Models',
      parentId: ch.id,
      amount: 10,
      amountUnit: 'problems',
    });
    return course.id;
  }, WORKSPACE);

  await goInside(page, `/w/${WORKSPACE}/courses/${courseId}`);
  await expect(page.getByTestId('unit-title')).toHaveValue('Logical Clocks');

  // Nothing set: no schedule in the unit's head until its menu opens it.
  await expect(page.getByTestId('unit-schedule-controls')).toHaveCount(0);
  await chooseFromMenu(page, page.getByTestId('unit-menu'), 'schedule');
  // Editor sets study amount (45 min) on Logical Clocks.
  await expect(page.getByTestId('unit-amount-add')).toBeFocused();
  await page.getByTestId('unit-amount-add').click();
  await page.getByTestId('unit-amount-input').fill('45');
  await page.getByTestId('unit-amount-unit-select').selectOption('minutes');
  await page.getByTestId('unit-amount-save').click();
  await expect(page.getByTestId('unit-amount-value')).toHaveText('45 min');

  // Open Schedule dialog and configure a past-to-today range with all 7 days + autoAssign.
  await showTree(page);
  await page.getByTestId('open-course-schedule').click();
  const dialog = page.getByTestId('course-schedule-dialog');
  await expect(dialog).toBeVisible();

  const start = isoDateOffset(-4);
  const today = isoDateOffset(0);
  await dialog.getByTestId('schedule-start-date').fill(start);
  await dialog.getByTestId('schedule-target-date').fill(today);
  // Enable Sat and Sun so every day in the 5-day window is a study day.
  for (const key of ['sat', 'sun'] as const) {
    const btn = dialog.getByTestId(`schedule-day-${key}`);
    if ((await btn.getAttribute('aria-pressed')) === 'false') await btn.click();
  }
  await dialog.getByTestId('schedule-save').click();

  // Overdue units stay overdue with explicit no-auto-shift note.
  await expect(dialog.getByTestId('schedule-dialog-overdue-count')).not.toHaveText('Overdue 0');
  await dialog.getByTestId('close-course-schedule').click();
  await expect(dialog).toBeHidden();

  await expect(page.getByTestId('course-schedule-no-auto-shift')).toBeVisible();

  // Chapter has no schedule badges; units show amount + date badges.
  const chapterRow = unitTree(page).locator('[role="treeitem"][aria-label="Foundations"]');
  await expect(chapterRow.getByTestId('unit-tree-amount')).toHaveCount(0);
  await expect(chapterRow.getByTestId('unit-tree-date')).toHaveCount(0);

  const unitOneRow = unitTree(page).locator('[role="treeitem"][aria-label="Logical Clocks"]');
  await expect(unitOneRow.getByTestId('unit-tree-amount')).toHaveText('45m');
  await expect(unitOneRow.getByTestId('unit-tree-date')).toHaveAttribute('data-overdue', 'true');
  await hideTree(page);

  // Set a personal plannedDate for Logical Clocks -> overrides overdue badge, then reset back.
  const futureDate = isoDateOffset(3);
  await page.getByTestId('unit-my-date-input').fill(futureDate);
  await expect(page.getByTestId('unit-status-overdue')).toHaveCount(0);
  await page.getByTestId('unit-my-date-clear').click();
  await expect(page.getByTestId('unit-status-overdue')).toBeVisible();

  // Done: one line of what is set, which opens the controls again.
  await page.getByTestId('unit-schedule-done').click();
  const summary = page.getByTestId('unit-schedule-summary');
  await expect(summary).toContainText('45 min');
  await expect(summary).toBeFocused();
  await expect(page.getByTestId('unit-my-date-input')).toHaveCount(0);
  await expect(page.getByTestId('unit-status-overdue')).toBeVisible();

  // Open "Today's learning" from sidebar button (not a link) and verify overdue & today lists.
  await openLearningToday(page);
  await expect(page.getByTestId('learning-overdue-section')).toBeVisible();
  await expect(page.getByTestId('learning-overdue-hint')).toBeVisible();
  await expect(page.getByTestId('learning-overdue-item').first()).toContainText('Logical Clocks');

  // Complete an overdue unit directly from Today's learning.
  const beforeOverdue = await page.getByTestId('learning-overdue-item').count();
  await page
    .getByTestId('learning-overdue-item')
    .first()
    .getByTestId('learning-item-complete')
    .click();
  await expect(page.getByTestId('learning-overdue-item')).toHaveCount(beforeOverdue - 1);

  // Navigate to a remaining unit directly from Today's learning.
  const remainingItem = page
    .locator('[data-testid="learning-overdue-item"], [data-testid="learning-today-item"]')
    .first();
  await remainingItem.getByTestId('learning-item-title').click();
  await expect(page).toHaveURL(ON_A_COURSE);
  await expect(page.getByTestId('course-screen')).toBeVisible();

  // Redistribute remaining incomplete units personally (does not change shared dates).
  await showTree(page);
  await page.getByTestId('open-course-schedule').click();
  const redistDialog = page.getByTestId('course-schedule-dialog');
  await expect(redistDialog).toBeVisible();
  const nextWeek = isoDateOffset(7);
  await redistDialog.getByTestId('schedule-target-date').fill(nextWeek);
  await redistDialog.getByTestId('redistribute-my-schedule').click();
  await expect(redistDialog.getByTestId('schedule-dialog-overdue-count')).toHaveText('Overdue 0');
  await redistDialog.getByTestId('close-course-schedule').click();
});

test('isolates unjoined open-course readers and strips all schedule UI/requests on public shared links', async ({
  page,
}) => {
  await openApp(page);

  const openCourseId = await page.evaluate(
    async ({ wsId, pastDate, futureDate }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const other = dev.someoneElse();
      const course = await other.course.createCourse(wsId, {
        title: 'Open Physics',
        visibility: 'open',
        minimumRole: 'viewer',
      });
      const u1 = await other.course.createUnit(course.id, {
        kind: 'unit',
        title: 'Kinematics',
        amount: 30,
        amountUnit: 'minutes',
        scheduledDate: pastDate,
      });
      await other.course.putUnitSchedule(course.id, u1.id, { plannedDate: futureDate });
      return course.id;
    },
    { wsId: WORKSPACE, pastDate: isoDateOffset(-3), futureDate: isoDateOffset(5) },
  );

  // Unjoined reader sees shared date, never sees creator's personal date, and no prompt of its own.
  await goInside(page, `/w/${WORKSPACE}/courses/${openCourseId}`);
  await expect(page.getByTestId('course-join')).toBeVisible();
  await expect(page.getByTestId('unit-shared-date-text')).toContainText(isoDateOffset(-3));
  await expect(page.getByTestId('unit-my-date-join-hint')).toHaveCount(0);
  await expect(page.getByTestId('unit-my-date-input')).toHaveCount(0);
  await expect(page.getByTestId('course-screen')).not.toContainText(isoDateOffset(5));

  // Unjoined course does not appear in Today's learning.
  await openLearningToday(page);
  await expect(page.getByTestId('learning-today-empty')).toBeVisible();

  // Public shared course makes zero /schedule or /learning/today calls and shows zero schedule UI.
  const sharedCourseId = '01930000-0000-7000-8000-00000000cc01';
  const sharedUnitId = '01930000-0000-7000-8000-00000000cc02';
  const forbiddenCalls: string[] = [];
  await page.route('**/api/v1/**', async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname.includes('/schedule') || pathname.includes('/learning/today')) {
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
          title: 'Public Course',
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
            amount: 30,
            amountUnit: 'minutes',
            scheduledDate: isoDateOffset(-2),
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
          amount: 30,
          amountUnit: 'minutes',
          scheduledDate: isoDateOffset(-2),
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
  await expect(page.getByTestId('course-schedule-panel')).toHaveCount(0);
  await expect(page.getByTestId('unit-schedule-controls')).toHaveCount(0);
  await expect(page.getByTestId('unit-tree-date')).toHaveCount(0);
  expect(forbiddenCalls).toEqual([]);
});

test('direct Today’s learning navigation syncs auto timeZone without closing the store', async ({
  page,
}) => {
  await openApp(page);

  await page.evaluate(async (wsId) => {
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const browserDate = (offset: number): string => {
      const parts = new Intl.DateTimeFormat('en-US', {
        timeZone: zone,
        year: 'numeric',
        month: '2-digit',
        day: '2-digit',
      }).formatToParts(new Date(Date.now() + offset * 86_400_000));
      const pick = (type: string): string => parts.find((p) => p.type === type)?.value ?? '00';
      return `${pick('year')}-${pick('month')}-${pick('day')}`;
    };
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const course = await dev.backend.course.createCourse(wsId, {
      title: 'Direct Today Course',
      visibility: 'open',
    });
    await dev.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Missed Lecture',
      scheduledDate: browserDate(-2),
    });
    await dev.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Current Lecture',
      scheduledDate: browserDate(0),
    });
  }, WORKSPACE);

  await goInside(page, `/w/${WORKSPACE}/learning/today`);
  await expect(page.getByTestId('learning-today-view')).toBeVisible();
  await expect(page.getByTestId('learning-overdue-item')).toHaveCount(1);
  await expect(page.getByTestId('learning-today-item')).toHaveCount(1);

  await page.getByTestId('learning-today-refresh').click();
  await expect(page.getByTestId('learning-overdue-item')).toHaveCount(1);

  await page
    .getByTestId('learning-overdue-item')
    .first()
    .getByTestId('learning-item-complete')
    .click();
  await expect(page.getByTestId('learning-overdue-item')).toHaveCount(0);
  await expect(page.getByTestId('learning-today-item')).toHaveCount(1);
});
