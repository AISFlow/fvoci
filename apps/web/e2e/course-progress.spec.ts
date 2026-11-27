/**
 * Course personal progress & member summary, end to end on the in-memory backend:
 * - Toggling unit completion & retaking (incomplete) updates header bar, ARIA attributes,
 *   outline checkmarks (excluding chapters), and members dialog aggregate rates.
 * - Saving, editing, and clearing `lastPosition` preserves completion state.
 * - Unjoined open-course reader is asked by the course's banner alone until joining.
 * - Public shared course (`/shared/...`) renders zero progress UI and makes zero
 *   `/progress` or `/progress/summary` network requests.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { WORKSPACE } from './address.ts';
import { openApp, showSidebar, sidebar } from './shell.support.ts';
import { goInside } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

const ID = '[0-9a-f-]{36}';
const ON_A_COURSE = new RegExp(`/w/${ID}/courses/(${ID})(/(${ID}))?$`);

interface DevCourse {
  createCourse(
    workspaceId: string,
    draft: { title: string; visibility: 'open' | 'private' },
  ): Promise<{ id: string }>;
  createUnit(
    courseId: string,
    draft: { kind?: 'unit' | 'chapter'; title?: string; parentId?: string },
  ): Promise<{ id: string }>;
  putUnitProgress(
    courseId: string,
    unitId: string,
    update: { completed?: boolean; lastPosition?: string | null },
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

async function newCourse(page: Page, title: string): Promise<string> {
  await showSidebar(page);
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await picker.getByTestId('new-title').fill(title);
  await picker.getByTestId('new-course').click();
  await expect(page).toHaveURL(ON_A_COURSE);
  await expect(page.getByTestId('course-screen')).toBeVisible();
  const match = new RegExp(`/courses/(${ID})`).exec(new URL(page.url()).pathname);
  return match?.[1] ?? '';
}

async function addChapter(page: Page, title: string): Promise<void> {
  await showTree(page);
  await page.getByTestId('new-chapter').click();
  const dialog = page.getByTestId('chapter-name-dialog');
  await dialog.getByTestId('chapter-name-input').fill(title);
  await dialog.getByTestId('chapter-name-submit').click();
  await expect(dialog).toBeHidden();
}

async function nameTheUnit(page: Page, title: string): Promise<void> {
  const field = page.getByTestId('unit-title');
  await expect(field).toBeFocused();
  await page.keyboard.type(title);
  await page.keyboard.press('Enter');
  await expect(unitTree(page).locator(`[role="treeitem"][aria-label="${title}"]`)).toBeAttached();
}

test('tracks completion, retake, lastPosition, tree checkmarks (no chapter mark), and members dialog rates', async ({
  page,
}) => {
  await openApp(page);
  const courseId = await newCourse(page, 'Progress Course');

  // Empty 0-unit course shows 0% (0/0 units).
  await expect(page.getByTestId('course-empty')).toBeVisible();
  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('0% (0/0 units)');
  const bar = page.getByTestId('course-progress-bar');
  await expect(bar).toHaveAttribute('aria-valuenow', '0');
  await expect(bar).toHaveAttribute('aria-valuemax', '0');

  // Add 1 chapter + 2 units.
  await addChapter(page, 'Module 1');
  await hideTree(page);
  await page.getByTestId('course-first-unit').click();
  await nameTheUnit(page, 'Unit One');

  await showTree(page);
  await page.getByTestId('new-unit').click();
  await nameTheUnit(page, 'Unit Two');

  // Add another enrolled member so the members dialog shows multiple learners.
  await page.evaluate(async (id) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    await dev.backend.addMember(id, dev.someoneElse().userId, 'viewer');
  }, courseId);

  // Header shows 0% (0/2 units); chapter has no progress icon; units have incomplete icons.
  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('0% (0/2 units)');
  await expect(bar).toHaveAttribute('aria-valuemax', '2');
  const chapterRow = unitTree(page).locator('[role="treeitem"][aria-label="Module 1"]');
  await expect(chapterRow.getByTestId('unit-tree-progress')).toHaveCount(0);
  const unitTwoRow = unitTree(page).locator('[role="treeitem"][aria-label="Unit Two"]');
  await expect(unitTwoRow.getByTestId('unit-tree-progress')).toHaveAttribute(
    'data-completed',
    'false',
  );
  await hideTree(page);

  // Mark Unit Two complete -> button & header & tree update.
  const toggle = page.getByTestId('unit-complete-toggle');
  await expect(toggle).toHaveAttribute('aria-pressed', 'false');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
  await expect(toggle).toContainText('Completed');

  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('50% (1/2 units)');
  await expect(bar).toHaveAttribute('aria-valuenow', '1');
  await expect(unitTwoRow.getByTestId('unit-tree-progress')).toHaveAttribute(
    'data-completed',
    'true',
  );
  await hideTree(page);

  // Save, edit, and clear lastPosition; verify completion stays intact.
  await page.getByTestId('unit-last-position-add').click();
  await page.getByTestId('unit-last-position-input').fill('12:30');
  await page.getByTestId('unit-last-position-save').click();
  await expect(page.getByTestId('unit-last-position-value')).toHaveText('12:30');

  await page.getByTestId('unit-last-position-value').click();
  await page.getByTestId('unit-last-position-input').fill('p.42');
  await page.getByTestId('unit-last-position-save').click();
  await expect(page.getByTestId('unit-last-position-value')).toHaveText('p.42');
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');

  // Toggle back to incomplete (retake): lastPosition stays preserved.
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'false');
  await expect(page.getByTestId('unit-last-position-value')).toHaveText('p.42');

  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('0% (0/2 units)');
  await hideTree(page);

  // Clear lastPosition and re-complete.
  await page.getByTestId('unit-last-position-clear').click();
  await expect(page.getByTestId('unit-last-position-add')).toBeVisible();
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');

  // Share / Members dialog displays aggregate progress rates only.
  await showTree(page);
  await page.getByTestId('share-course').click();
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog).toBeVisible();
  const badges = dialog.getByTestId('member-course-progress');
  await expect(badges).toHaveCount(2);
  await expect
    .poll(async () => (await badges.allTextContents()).map((s) => s.trim()).sort())
    .toEqual(['0% (0/2 units)', '50% (1/2 units)']);
  await expect(dialog).not.toContainText('Unit Two');
});

test('keeps unjoined open-course reader read-only until joining, and isolates public shared links', async ({
  page,
}) => {
  await openApp(page);

  // Another user creates an open course with 2 units and completes 1 of them.
  const openCourseId = await page.evaluate(async (wsId) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const other = dev.someoneElse();
    const course = await other.course.createCourse(wsId, {
      title: 'Open Seminar',
      visibility: 'open',
    });
    const u1 = await other.course.createUnit(course.id, { kind: 'unit', title: 'Reading 1' });
    await other.course.createUnit(course.id, { kind: 'unit', title: 'Reading 2' });
    await other.course.putUnitProgress(course.id, u1.id, {
      completed: true,
      lastPosition: 'private-note-99',
    });
    return course.id;
  }, WORKSPACE);

  await goInside(page, `/w/${WORKSPACE}/courses/${openCourseId}`);
  await expect(page.getByTestId('course-join')).toBeVisible();

  // Unjoined reader: asked by the banner alone, no toggle, no tree checkmarks, no private note.
  await expect(page.getByTestId('unit-complete-toggle')).toHaveCount(0);
  await expect(page.getByTestId('unit-progress-join-hint')).toHaveCount(0);
  await expect(page.getByTestId('unit-last-position-add')).toHaveCount(0);
  await expect(page.getByTestId('course-screen')).not.toContainText('private-note-99');

  await showTree(page);
  await expect(page.getByTestId('course-progress-join-hint')).toHaveCount(0);
  await expect(page.getByTestId('course-my-progress')).toHaveCount(0);
  await expect(unitTree(page).getByTestId('unit-tree-progress')).toHaveCount(0);

  // Unjoined reader can still inspect the Members dialog and see the creator's aggregate rate only.
  await page.getByTestId('share-course').click();
  const membersDialog = page.getByTestId('share-dialog');
  await expect(membersDialog.getByTestId('member-course-progress')).toHaveText(['50% (1/2 units)']);
  await expect(membersDialog).not.toContainText('private-note-99');
  await page.keyboard.press('Escape');
  await expect(membersDialog).toBeHidden();
  await hideTree(page);

  // Joining unlocks personal progress tracking.
  await page.getByTestId('course-join').getByRole('button').click();
  await expect(page.getByTestId('course-join')).toHaveCount(0);
  await expect(page.getByTestId('unit-complete-toggle')).toBeEnabled();
  await page.getByTestId('unit-complete-toggle').click();
  await expect(page.getByTestId('unit-complete-toggle')).toHaveAttribute('aria-pressed', 'true');
  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('50% (1/2 units)');

  // Public shared course makes zero /progress or /progress/summary requests and renders no progress UI.
  const sharedCourseId = '01930000-0000-7000-8000-00000000bb01';
  const sharedUnitId = '01930000-0000-7000-8000-00000000bb02';
  const progressCalls: string[] = [];
  await page.route('**/api/v1/**', async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname.includes('/progress')) {
      progressCalls.push(pathname);
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
  await expect(page.getByTestId('course-my-progress')).toHaveCount(0);
  await expect(page.getByTestId('unit-complete-toggle')).toHaveCount(0);
  await expect(page.getByTestId('unit-last-position')).toHaveCount(0);
  await expect(page.getByTestId('unit-tree-progress')).toHaveCount(0);
  expect(progressCalls).toEqual([]);
});
