/**
 * Course collaboration & sharing features on the in-memory backend:
 * - Unit comments CRUD (write, edit, history diff, delete + undo, live arrival)
 * - Unit body versions (⌘S/Ctrl+S manual keep, list, diff comparison, restore)
 * - `[[` unit references (suggestion, chip navigation, backlinks)
 * - Shared-workspace unit watching toggle (hidden in own space)
 * - Course public sharing dialog + public course reader screen (`/shared/...`)
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { focusAtEnd } from './embeds.support.ts';
import { primaryKey } from './keyboard.support.ts';
import { openApp, pressInBar, showSidebar, sidebar } from './shell.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

const ID = '[0-9a-f-]{36}';
const ON_A_COURSE = new RegExp(`/w/${ID}/courses/(${ID})(/(${ID}))?$`);

function coursePlaceOf(address: string): { workspaceId: string; courseId: string; unitId: string } {
  const path = new URL(address, 'http://here').pathname;
  const match = new RegExp(`^/w/(${ID})/courses/(${ID})(?:/(${ID}))?$`).exec(path);
  return {
    workspaceId: match?.[1] ?? '',
    courseId: match?.[2] ?? '',
    unitId: match?.[3] ?? '',
  };
}

interface Elsewhere {
  userId: string;
  course: {
    unit(courseId: string, unitId: string): Promise<{ updatedAt: number }>;
    patchUnit(courseId: string, unitId: string, patch: object): Promise<unknown>;
    createComment(
      courseId: string,
      unitId: string,
      body: object,
    ): Promise<{ id: string; updatedAt: number }>;
    deleteComment(courseId: string, unitId: string, commentId: string): Promise<void>;
  };
}

interface Dev {
  backend: {
    addMember(containerId: string, userId: string, role: string): Promise<unknown>;
  };
  someoneElse(): Elsewhere;
}

const unitTree = (page: Page): Locator => page.getByTestId('unit-tree');
const comments = (page: Page): Locator => page.getByTestId('card-comments');
const commentRows = (page: Page): Locator => comments(page).locator('[data-comment]');
const composer = (page: Page): Locator => page.getByTestId('comment-composer');
const versions = (page: Page): Locator => page.getByTestId('body-versions');
const versionRows = (page: Page): Locator => versions(page).locator('[data-version]');

const paragraph = (text: string): object => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

async function showTree(page: Page): Promise<void> {
  const opener = page.getByTestId('open-unit-tree');
  if ((await opener.isVisible()) && (await opener.getAttribute('aria-expanded')) !== 'true') {
    await opener.click();
  }
  await expect(unitTree(page)).toBeVisible();
}

async function newCourse(
  page: Page,
  title: string,
  options: { open?: boolean; ownSpace?: boolean } = {},
): Promise<string> {
  await showSidebar(page);
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await expect(picker).toBeVisible();
  await picker.getByTestId('new-title').fill(title);
  if (!options.ownSpace) {
    await picker
      .getByTestId(options.open === false ? 'new-visibility-private' : 'new-visibility-open')
      .check();
  }
  await picker.getByTestId('new-course').click();
  await expect(page).toHaveURL(ON_A_COURSE);
  return coursePlaceOf(page.url()).courseId;
}

async function nameTheUnit(page: Page, title: string): Promise<void> {
  const field = page.getByTestId('unit-title');
  await expect(field).toBeFocused();
  await page.keyboard.type(title);
  await page.keyboard.press('Enter');
  await expect(unitTree(page).locator(`[role="treeitem"][aria-label="${title}"]`)).toBeAttached();
}

async function createFirstUnit(page: Page, title: string) {
  await page.getByTestId('course-first-unit').click();
  await nameTheUnit(page, title);
  return coursePlaceOf(page.url());
}

async function addUnit(page: Page, title: string) {
  await showTree(page);
  await page.getByTestId('new-unit').click();
  await nameTheUnit(page, title);
  return coursePlaceOf(page.url());
}

test('unit comments CRUD, edit history, undo delete, live arrival, and watch toggle', async ({
  page,
}) => {
  await openApp(page);
  await newCourse(page, 'Study Guide');
  const place = await createFirstUnit(page, 'Lesson 1');

  // Watch toggle is active by default in shared workspace and toggles cleanly.
  const watch = page.getByTestId('unit-watch-toggle');
  await expect(watch).toBeVisible();
  await expect(watch).toHaveAttribute('aria-pressed', 'true');
  await watch.click();
  await expect(watch).toHaveAttribute('aria-pressed', 'false');
  await watch.click();
  await expect(watch).toHaveAttribute('aria-pressed', 'true');

  // Write, edit, inspect history diff, delete, and undo.
  await expect(comments(page)).toContainText('No comments yet.');
  await composer(page).getByTestId('comment-field').fill('A first question');
  await composer(page).getByTestId('comment-send').click();
  const row = commentRows(page).first();
  await expect(row).toContainText('A first question');

  await row.locator('[data-action="edit"]').click();
  const field = row.getByRole('textbox', { name: 'Comment text' });
  await field.fill('A revised question');
  await field.press(await primaryKey(page, 'Enter'));
  await expect(row).toContainText('A revised question');
  await expect(row.getByTestId('comment-edited')).toBeVisible();

  await row.locator('[data-action="history"]').click();
  const history = row.getByTestId('comment-history');
  await expect(history).toContainText('A first question');
  await expect(history.locator('[data-version="now"] del')).toHaveText(['first']);
  await expect(history.locator('[data-version="now"] ins')).toHaveText(['revised']);

  await row.getByRole('button', { name: /^Delete the comment from/ }).click();
  await expect(commentRows(page)).toHaveCount(0);
  await pressInBar(page, 'undo');
  await expect(commentRows(page)).toHaveCount(1);
  await expect(commentRows(page).first()).toContainText('A revised question');

  // Another member's comment arrives live without a reload.
  await page.evaluate(
    async ({ at, body }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const other = dev.someoneElse();
      await dev.backend.addMember(at.courseId, other.userId, 'editor');
      await other.course.createComment(at.courseId, at.unitId, body);
    },
    { at: place, body: paragraph('Live reply from classmate') },
  );
  await expect(commentRows(page)).toHaveCount(2);
  await expect(commentRows(page).last()).toContainText('Live reply from classmate');
});

test('unit body versions: ⌘S manual keep, diff compare, and restore', async ({ page }) => {
  await openApp(page);
  await newCourse(page, 'Versioned Course');
  const place = await createFirstUnit(page, 'Core Concept');

  const body = page.getByTestId('unit-body');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await focusAtEnd(body);
  await page.keyboard.type('First draft paragraph');
  await page.keyboard.press(await primaryKey(page, 's'));
  await expect(body.getByTestId('body-kept')).toContainText(/Version kept/i);

  // Another editor writes a second version whole so we have two versions to compare & restore.
  await page.evaluate(
    async ({ at, text }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const other = dev.someoneElse();
      await dev.backend.addMember(at.courseId, other.userId, 'editor');
      const detail = await other.course.unit(at.courseId, at.unitId);
      const doc = {
        type: 'doc',
        content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
      };
      await other.course.patchUnit(at.courseId, at.unitId, { body: doc, readAt: detail.updatedAt });
    },
    { at: place, text: 'Second revised paragraph' },
  );
  await expect(body.locator('[contenteditable]').first()).toContainText('Second revised paragraph');

  // Open versions dialog, pick the earlier draft, inspect diff, and restore it.
  await page.getByTestId('unit-menu').click();
  await page.locator('[data-action="versions"]').click();
  await expect(versions(page)).toBeVisible();
  await expect(versionRows(page)).toHaveCount(2);
  await versionRows(page).last().click();
  await expect(versions(page).getByTestId('body-version')).toContainText('First draft paragraph');

  await page.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(versions(page)).toHaveCount(0);
  await expect(body.locator('[contenteditable]').first()).toContainText('First draft paragraph');
});

test('[[ unit reference links between units and renders backlinks', async ({ page }) => {
  await openApp(page);
  await newCourse(page, 'Linked Course');
  const u1 = await createFirstUnit(page, 'Lesson Alpha');
  const u2 = await addUnit(page, 'Lesson Beta');

  // Return to Lesson Alpha and insert a [[ reference to Lesson Beta.
  await showTree(page);
  await unitTree(page).getByRole('treeitem', { name: 'Lesson Alpha', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/courses/${u1.courseId}/${u1.unitId}$`));

  const body = page.getByTestId('unit-body');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await focusAtEnd(body);
  await page.keyboard.type('Continue with [[Lesson Beta');
  const option = page
    .locator('[data-suggestions="reference"]')
    .getByRole('option', { name: 'Lesson Beta · Unit' });
  await expect(option).toHaveCount(1);
  await page.keyboard.press('Enter');

  const chip = body.locator(`[data-reference][data-block-id="${u2.unitId}"]`);
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await expect(chip).toHaveAttribute('data-kind', 'unit');
  await expect(chip).toHaveText('Lesson Beta');

  // Wait for room flush so backlinks index sees the reference, then click chip to navigate.
  await page.waitForTimeout(2300);
  await chip.getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/courses/${u2.courseId}/${u2.unitId}$`));
  await expect(page.getByTestId('unit-title')).toHaveValue('Lesson Beta');

  // Backlinks on Lesson Beta list Lesson Alpha.
  const backlinks = page.getByTestId('card-backlinks');
  await expect(backlinks.getByRole('link')).toHaveText(['Lesson Alpha']);
});

test('public link dialog works on a course and SharedCourse reads a shared course', async ({
  page,
}) => {
  await openApp(page);
  const courseId = await newCourse(page, 'Public Course');
  await createFirstUnit(page, 'Intro Unit');

  // Enable public link from course header Share button.
  await showTree(page);
  await page.getByTestId('share-course').click();
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog).toBeVisible();
  await dialog.getByTestId('share-public').click();
  const linkInput = dialog.getByTestId('share-link');
  await expect(linkInput).toHaveValue(new RegExp(`/shared/${courseId}\\?r=[^&]+$`));
  const sharedUrl = await linkInput.inputValue();

  // Route the public reader API to verify SharedCourse end to end in memory preview.
  const chapterId = '01930000-0000-7000-8000-00000000aa01';
  const unitA = '01930000-0000-7000-8000-00000000aa02';
  const unitB = '01930000-0000-7000-8000-00000000aa03';
  const outsideId = '01930000-0000-7000-8000-00000000aa99';
  const baseApi = `/api/v1/shared/${courseId}`;

  await page.route(`**${baseApi}**`, async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === baseApi) {
      await route.fulfill({
        json: {
          id: courseId,
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
    if (pathname === `${baseApi}/units`) {
      await route.fulfill({
        json: [
          {
            id: chapterId,
            kind: 'chapter',
            parentId: '',
            title: 'Part 1',
            order: 'a',
            createdAt: 1,
            updatedAt: 1,
          },
          {
            id: unitA,
            kind: 'unit',
            parentId: chapterId,
            title: 'First Lesson',
            order: 'a',
            createdAt: 2,
            updatedAt: 2,
          },
          {
            id: unitB,
            kind: 'unit',
            parentId: chapterId,
            title: 'Second Lesson',
            order: 'b',
            createdAt: 3,
            updatedAt: 3,
          },
        ],
      });
      return;
    }
    if (pathname === `${baseApi}/units/${unitA}`) {
      await route.fulfill({
        json: {
          id: unitA,
          courseId,
          kind: 'unit',
          parentId: chapterId,
          title: 'First Lesson',
          order: 'a',
          body: {
            type: 'doc',
            content: [
              {
                type: 'paragraph',
                content: [
                  { type: 'text', text: 'Read next: ' },
                  { type: 'reference', attrs: { id: unitB } },
                  { type: 'text', text: ' and ' },
                  { type: 'reference', attrs: { id: outsideId } },
                ],
              },
            ],
          },
          createdBy: '',
          modifiedBy: '',
          createdAt: 2,
          updatedAt: 2,
        },
      });
      return;
    }
    if (pathname === `${baseApi}/units/${unitB}`) {
      await route.fulfill({
        json: {
          id: unitB,
          courseId,
          kind: 'unit',
          parentId: chapterId,
          title: 'Second Lesson',
          order: 'b',
          body: paragraph('Welcome to the second lesson.'),
          createdBy: '',
          modifiedBy: '',
          createdAt: 3,
          updatedAt: 3,
        },
      });
      return;
    }
    await route.fulfill({ status: 404, json: { detail: 'not found' } });
  });

  await page.goto(sharedUrl);
  const reader = page.getByTestId('shared-course');
  await expect(reader).toBeVisible();
  await expect(page.getByTestId('shared-title')).toHaveText('Public Course');
  await expect(reader.getByTestId('unit-title')).toHaveText('First Lesson');
  const treeToggle = reader.getByRole('button', { name: 'Units' });
  if (await treeToggle.isVisible()) await treeToggle.click();
  await expect(reader.getByTestId('shared-unit-tree').locator('[data-kind="chapter"]')).toHaveText(
    'Part 1',
  );
  await expect(reader.getByTestId('shared-unit-tree').getByRole('link')).toHaveText([
    'First Lesson',
    'Second Lesson',
  ]);
  if (await treeToggle.isVisible()) await treeToggle.click();
  // Read-only: no comments, versions, editing, or sidebar.
  await expect(page.getByTestId('comment-composer')).toHaveCount(0);
  await expect(page.getByTestId('unit-menu')).toHaveCount(0);
  await expect(page.getByRole('textbox')).toHaveCount(0);
  await expect(page.getByTestId('sidebar')).toHaveCount(0);

  // External reference is unseen; same-course reference opens Second Lesson.
  const external = reader.getByTestId('unit-body').locator(`[data-block-id="${outsideId}"]`);
  await expect(external).toHaveAttribute('data-state', 'unseen');
  await reader.getByTestId('unit-body').getByRole('link', { name: 'Second Lesson' }).click();
  await expect(reader.getByTestId('unit-title')).toHaveText('Second Lesson');
  await expect(reader.getByTestId('unit-body')).toContainText('Welcome to the second lesson.');
});
