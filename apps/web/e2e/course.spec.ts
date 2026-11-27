/**
 * Courses, end to end, on the in-memory backend: created in both the shared
 * workspace and the reader's own space, with chapters and units (depth 2),
 * moved with Alt+Arrow and pointer drag, deleted and restored from the unit
 * trash, preserved across store reloads, searched, and rejected at board URLs.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath, ON_A_BOARD, placeOf, WORKSPACE } from './address.ts';
import { nextFrame, pointOn, touch } from './kanban.support.ts';
import {
  boardLink,
  FIRST_BOARD,
  openApp,
  showSidebar,
  sidebar,
  stageOf,
  strip,
  topBar,
} from './shell.support.ts';
import { goInside } from './wiki.support.ts';

const ID = '[0-9a-f-]{36}';
const ON_A_COURSE = new RegExp(`/w/${ID}/courses/(${ID})(/(${ID}))?$`);
const ON_A_UNIT = new RegExp(`/w/${ID}/courses/(${ID})/(${ID})$`);

function coursePath(workspaceId: string, courseId: string, unitId?: string): string {
  return `/w/${workspaceId}/courses/${courseId}${unitId ? `/${unitId}` : ''}`;
}

function coursePlaceOf(address: string): { workspaceId: string; courseId: string; unitId: string } {
  const path = new URL(address, 'http://here').pathname;
  const match = new RegExp(`^/w/(${ID})/courses/(${ID})(?:/(${ID}))?$`).exec(path);
  return {
    workspaceId: match?.[1] ?? '',
    courseId: match?.[2] ?? '',
    unitId: match?.[3] ?? '',
  };
}

const courseScreen = (page: Page): Locator => page.getByTestId('course-screen');
const unitTree = (page: Page): Locator => page.getByTestId('unit-tree');
const treeItem = (page: Page, name: string): Locator =>
  unitTree(page).getByRole('treeitem', { name, exact: true });
const row = (page: Page, name: string): Locator =>
  unitTree(page).locator(`[data-page][aria-label="${name}"]`);
const unitBody = (page: Page): Locator =>
  page.getByTestId('unit-body').locator('[contenteditable]').first();

async function treeShape(page: Page): Promise<string[]> {
  return unitTree(page)
    .getByRole('treeitem')
    .evaluateAll((items) =>
      items.map(
        (item) =>
          `${'  '.repeat(Number(item.getAttribute('aria-level')) - 1)}${item.getAttribute('aria-label') ?? ''}`,
      ),
    );
}

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
  await page.getByTestId('unit-tree-panel').press('Escape');
  await expect(opener).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByTestId('unit-tree-panel')).toBeHidden();
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
  if (options.ownSpace) {
    await expect(picker.getByRole('radio')).toHaveCount(0);
  } else {
    await picker
      .getByTestId(options.open === false ? 'new-visibility-private' : 'new-visibility-open')
      .check();
  }
  await picker.getByTestId('new-course').click();
  await expect(page).toHaveURL(ON_A_COURSE);
  await expect(courseScreen(page)).toBeVisible();
  return coursePlaceOf(page.url()).courseId;
}

async function addChapter(page: Page, title: string): Promise<void> {
  await showTree(page);
  await page.getByTestId('new-chapter').click();
  const dialog = page.getByTestId('chapter-name-dialog');
  await expect(dialog).toBeVisible();
  await dialog.getByTestId('chapter-name-input').fill(title);
  await dialog.getByTestId('chapter-name-submit').click();
  await expect(dialog).toBeHidden();
  await expect(unitTree(page).locator(`[role="treeitem"][aria-label="${title}"]`)).toBeAttached();
}

async function nameTheUnit(page: Page, title: string): Promise<void> {
  const field = page.getByTestId('unit-title');
  await expect(field).toBeFocused();
  await page.keyboard.type(title);
  await page.keyboard.press('Enter');
  await expect(unitTree(page).locator(`[role="treeitem"][aria-label="${title}"]`)).toBeAttached();
}

async function unitAction(page: Page, label: string, action: string): Promise<void> {
  await showTree(page);
  await unitTree(page).getByRole('button', { name: label, exact: true }).click();
  await page.locator(`[data-action="${action}"]`).click();
}

async function switchWorkspace(page: Page, name: string): Promise<void> {
  await showSidebar(page);
  await sidebar(page).getByTestId('workspace-switcher').click();
  await page
    .getByRole('menu', { name: 'Switch workspace', exact: true })
    .getByRole('menuitem', { name, exact: true })
    .click();
}

test.describe('a course in a shared workspace', () => {
  test('is created from New, shown empty at its address, and filed in the sidebar with its course menu', async ({
    page,
  }) => {
    await openApp(page);
    const courseId = await newCourse(page, 'Foundations');
    await expect(page).toHaveURL(coursePath(WORKSPACE, courseId));

    // Empty state offers making the first unit.
    await expect(page.getByTestId('course-empty')).toBeVisible();
    await expect(page.getByTestId('course-first-unit')).toHaveText('Create first unit');
    await expect(topBar(page)).toContainText('Foundations');

    // Sidebar lists the course with its graduation-cap icon and course menu (no duplicate/template).
    await showSidebar(page);
    const link = sidebar(page).getByRole('link', { name: 'Foundations, course', exact: true });
    await expect(link).toHaveAttribute('aria-current', 'page');
    await expect(link.locator('[data-board-icon]')).toHaveAttribute('data-kind', 'course');
    const menuBtn = sidebar(page).getByRole('button', { name: 'Menu of the course Foundations' });
    await menuBtn.click();
    await expect(page.locator('[data-action="export"]')).toBeVisible();
    await expect(page.locator('[data-action="duplicate"]')).toBeVisible();
    await expect(page.locator('[data-action="delete"]')).toBeVisible();
    await expect(page.locator('[data-action="template-from-board"]')).toHaveCount(0);
    await page.keyboard.press('Escape');
    if ((await stageOf(page)) === 'medium') {
      await expect(strip(page).getByRole('link', { name: 'Foundations, course' })).toBeVisible();
    }
  });

  test('builds 1 chapter + 3 units, moves by keyboard and drag, deletes/restores, and keeps order & body', async ({
    page,
  }, info) => {
    await openApp(page);
    const courseId = await newCourse(page, 'Foundations');

    // Add 1 chapter + 3 units in order.
    await addChapter(page, 'Part 1');
    await unitAction(page, 'Menu of the chapter Part 1', 'add-child');
    await expect(page).toHaveURL(ON_A_UNIT);
    await nameTheUnit(page, 'Unit A');
    const unitAId = coursePlaceOf(page.url()).unitId;

    // Write body content into Unit A.
    await expect(page.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await unitBody(page).click();
    await page.keyboard.type('Welcome to Unit A.');
    await expect(unitBody(page)).toHaveText('Welcome to Unit A.');

    await showTree(page);
    await page.getByTestId('new-unit').click();
    await nameTheUnit(page, 'Unit B');

    await showTree(page);
    await page.getByTestId('new-unit').click();
    await nameTheUnit(page, 'Unit C');

    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Part 1', '  Unit A', 'Unit B', 'Unit C']);

    // Keyboard moves with Alt+Arrow (desktop keyboard).
    if (!touch(info)) {
      // Move Unit B into Part 1 with Alt+ArrowRight.
      await treeItem(page, 'Unit B').focus();
      await page.keyboard.press('Alt+ArrowRight');
      await expect
        .poll(() => treeShape(page))
        .toEqual(['Part 1', '  Unit A', '  Unit B', 'Unit C']);

      // Trying to nest Unit B under Unit A (depth 3) is ignored.
      await page.keyboard.press('Alt+ArrowRight');
      expect(await treeShape(page)).toEqual(['Part 1', '  Unit A', '  Unit B', 'Unit C']);

      // Reorder Unit B before Unit A inside Part 1.
      await page.keyboard.press('Alt+ArrowUp');
      await expect
        .poll(() => treeShape(page))
        .toEqual(['Part 1', '  Unit B', '  Unit A', 'Unit C']);

      // Drag Unit C into Part 1 with the mouse.
      const from = await pointOn(row(page, 'Unit C'));
      await page.mouse.move(from.x, from.y);
      await page.mouse.down();
      await page.mouse.move(from.x + 4, from.y - 6, { steps: 3 });
      await expect(page.getByTestId('drag-chip')).toHaveText('Unit C');
      const onto = await pointOn(row(page, 'Part 1'));
      await page.mouse.move(onto.x, onto.y, { steps: 6 });
      await nextFrame(page);
      await expect(row(page, 'Part 1')).toHaveAttribute('data-drop', 'inside');
      await page.mouse.up();
      await expect
        .poll(() => treeShape(page))
        .toEqual(['Part 1', '  Unit B', '  Unit A', '  Unit C']);
    }

    // Delete Unit C and restore it from the unit trash dialog.
    await unitAction(page, 'Menu of the unit Unit C', 'delete');
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    await showTree(page);
    const expectedAfterDelete = touch(info)
      ? ['Part 1', '  Unit A', 'Unit B']
      : ['Part 1', '  Unit B', '  Unit A'];
    await expect.poll(() => treeShape(page)).toEqual(expectedAfterDelete);

    await showTree(page);
    await page.getByTestId('open-unit-trash').click();
    const trash = page.getByTestId('unit-trash');
    await expect(trash.getByRole('heading')).toHaveText('Deleted units');
    await trash.getByRole('button', { name: 'Restore Unit C' }).click();
    await expect(trash.getByTestId('trash-empty')).toBeVisible();
    await trash.getByRole('button', { name: 'Close' }).last().click();
    await expect(trash).toBeHidden();

    const expectedAfterRestore = touch(info)
      ? ['Part 1', '  Unit A', 'Unit B', 'Unit C']
      : ['Part 1', '  Unit B', '  Unit A', '  Unit C'];
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(expectedAfterRestore);

    // Navigate away to a board and back (reloads CourseStore & UnitStore from backend):
    // order and body content are preserved.
    await hideTree(page);
    await showSidebar(page);
    await boardLink(page, FIRST_BOARD).click();
    await expect(page).toHaveURL(ON_A_BOARD);
    await expect(page.getByTestId('board-page')).toBeVisible();

    await showSidebar(page);
    await sidebar(page).getByRole('link', { name: 'Foundations, course', exact: true }).click();
    await expect(courseScreen(page)).toBeVisible();
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(expectedAfterRestore);
    await treeItem(page, 'Unit A').click();
    await expect(page).toHaveURL(coursePath(WORKSPACE, courseId, unitAId));
    await expect(page.getByTestId('unit-title')).toHaveValue('Unit A');
    await expect(unitBody(page)).toHaveText('Welcome to Unit A.');
  });

  test('seeded course keeps its address and content across a full browser reload', async ({
    page,
  }) => {
    await openApp(page);
    await showSidebar(page);
    await sidebar(page).getByRole('link', { name: 'Workspace Onboarding, course' }).click();
    await expect(page).toHaveURL(ON_A_UNIT);
    await expect(courseScreen(page)).toBeVisible();
    const beforeUrl = page.url();
    const beforeTitle = await page.getByTestId('unit-title').inputValue();
    expect(beforeTitle.length).toBeGreaterThan(0);

    await page.reload();
    await expect(page).toHaveURL(beforeUrl);
    await expect(courseScreen(page)).toBeVisible();
    await expect(page.getByTestId('unit-title')).toHaveValue(beforeTitle);
  });

  test('rejects a board ID at a course address and a course ID at a board address, and excludes courses from templates', async ({
    page,
  }) => {
    await openApp(page);
    const boardId = new URL(page.url()).pathname.split('/')[4] ?? '';
    const courseId = await newCourse(page, 'Curriculum');

    await goInside(page, coursePath(WORKSPACE, boardId));
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(courseScreen(page)).toHaveCount(0);

    await goInside(page, boardPath(courseId));
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(page.getByTestId('board-page')).toHaveCount(0);

    await goInside(page, boardPath(boardId));
    await expect(page.getByTestId('board-page')).toBeVisible();
    await showSidebar(page);
    await sidebar(page).getByTestId('add-board').click();
    const picker = page.getByTestId('template-picker');
    await expect(picker.getByTestId('templates')).toBeAttached();
    await expect(picker.getByText('Curriculum')).toHaveCount(0);
  });

  test('search finds a course unit and opens its unit address', async ({ page }) => {
    await openApp(page);
    const courseId = await newCourse(page, 'Searchable Course');
    await page.getByTestId('course-first-unit').click();
    await nameTheUnit(page, 'Quokka Orientation');
    const unitId = coursePlaceOf(page.url()).unitId;
    // The window is named by the unit before its course.
    await expect(page).toHaveTitle('Quokka Orientation · Searchable Course — fvoci');

    // Leave the course so navigation from search is observable.
    await showSidebar(page);
    await boardLink(page, FIRST_BOARD).click();
    await expect(page.getByTestId('board-page')).toBeVisible();

    await topBar(page).getByTestId('open-search').click();
    const dialog = page.getByTestId('search-dialog');
    await dialog.getByTestId('search-field').fill('Quokka');
    const hit = dialog.getByRole('option', { name: /Quokka Orientation/ });
    await expect(hit).toBeVisible();
    await hit.click();
    await expect(page).toHaveURL(coursePath(WORKSPACE, courseId, unitId));
    await expect(page.getByTestId('unit-title')).toHaveValue('Quokka Orientation');
  });
});

test.describe('a course in the reader’s own space', () => {
  test('creates a private course without visibility radios, adds 1 chapter + 3 units, moves, deletes/restores, and persists across switches', async ({
    page,
  }, info) => {
    await openApp(page);
    await switchWorkspace(page, 'My space');
    const ownId = placeOf(page.url()).workspaceId;

    const courseId = await newCourse(page, 'Personal Study', { ownSpace: true });
    await expect(page).toHaveURL(coursePath(ownId, courseId));

    await addChapter(page, 'Week 1');
    await unitAction(page, 'Menu of the chapter Week 1', 'add-child');
    await nameTheUnit(page, 'Lesson 1');
    await expect(page.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await unitBody(page).click();
    await page.keyboard.type('Private notes for Lesson 1.');
    await expect(unitBody(page)).toHaveText('Private notes for Lesson 1.');

    await showTree(page);
    await page.getByTestId('new-unit').click();
    await nameTheUnit(page, 'Lesson 2');
    await showTree(page);
    await page.getByTestId('new-unit').click();
    await nameTheUnit(page, 'Lesson 3');

    await showTree(page);
    await expect
      .poll(() => treeShape(page))
      .toEqual(['Week 1', '  Lesson 1', 'Lesson 2', 'Lesson 3']);

    if (!touch(info)) {
      await treeItem(page, 'Lesson 3').focus();
      await page.keyboard.press('Alt+ArrowUp');
      await expect
        .poll(() => treeShape(page))
        .toEqual(['Week 1', '  Lesson 1', 'Lesson 3', 'Lesson 2']);
    }

    // Delete Lesson 2 and restore it from unit trash.
    await unitAction(page, 'Menu of the unit Lesson 2', 'delete');
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Week 1', '  Lesson 1', 'Lesson 3']);

    await showTree(page);
    await page.getByTestId('open-unit-trash').click();
    const trash = page.getByTestId('unit-trash');
    await trash.getByRole('button', { name: 'Restore Lesson 2' }).click();
    await trash.getByRole('button', { name: 'Close' }).last().click();
    await expect(trash).toBeHidden();

    const expectedOrder = touch(info)
      ? ['Week 1', '  Lesson 1', 'Lesson 2', 'Lesson 3']
      : ['Week 1', '  Lesson 1', 'Lesson 3', 'Lesson 2'];
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(expectedOrder);

    // Switch to shared workspace and back to My space: course, order and body remain.
    await hideTree(page);
    await switchWorkspace(page, 'fvoci');
    await expect(page.getByTestId('board-page')).toBeVisible();
    await switchWorkspace(page, 'My space');
    await showSidebar(page);
    await sidebar(page).getByRole('link', { name: 'Personal Study, course' }).click();
    await expect(courseScreen(page)).toBeVisible();
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(expectedOrder);
    await treeItem(page, 'Lesson 1').click();
    await expect(unitBody(page)).toHaveText('Private notes for Lesson 1.');
  });
});

test.describe('a course at a phone’s width', () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test('puts the outline tree behind a drawer button and closes on unit selection', async ({
    page,
  }) => {
    await openApp(page);
    await newCourse(page, 'Mobile Study');
    await page.getByTestId('course-first-unit').click();
    await nameTheUnit(page, 'Intro');

    const opener = page.getByTestId('open-unit-tree');
    await expect(opener).toBeVisible();
    const drawer = page.getByRole('dialog', { name: 'Units' });
    await opener.click();
    await expect(drawer).toBeVisible();
    await expect(treeItem(page, 'Intro')).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(drawer).toBeHidden();
    await expect(opener).toBeFocused();
  });
});
