/**
 * Courses on the real server:
 * - Own space: create course, 1 chapter + 3 units in order, move, delete & restore
 *   from unit trash, verify full DB persistence after `page.reload()`, and co-edit
 *   a unit across two browser contexts of the same account (text and caret sync).
 * - Shared workspace: open course read without joining, join and co-edit live with
 *   `PageViewers`, live outline sync (create/rename/move/delete), and immediate
 *   `docLeft` + read block when removed or made private.
 * - Unit body table: insert, live-sync, persist to DB, and restore on reload.
 * - Boundaries & search: board ID at course URL shows NotFound, course ID at board
 *   URL shows NotFound, and workspace search finds a unit by body word and opens it.
 *
 * With `FVOCI_SCREENS=1`, captures light/dark screenshots at 1280px and 390px
 * (including the mobile outline drawer) into `target/course/screens/`.
 */
import { mkdirSync } from 'node:fs';
import path from 'node:path';

import {
  expect,
  test,
  type APIRequestContext,
  type Browser,
  type Locator,
  type Page,
} from '@playwright/test';

import { lineBoundary, primaryKey } from '../e2e/keyboard.support.ts';
import { showSidebar, sidebar } from '../e2e/shell.support.ts';
import { cellsOf, paste } from '../e2e/tables.support.ts';

import {
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  type Person,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

const ID = '[0-9a-f-]{36}';
const ON_A_COURSE = new RegExp(`/w/${ID}/courses/(${ID})(/(${ID}))?$`);
const ON_A_UNIT = new RegExp(`/w/${ID}/courses/(${ID})/(${ID})$`);
const COURSE_SCREENS = path.resolve(import.meta.dirname, '../../../target/course/screens');

const coursePath = (workspaceId: string, courseId: string, unitId?: string): string =>
  `/w/${workspaceId}/courses/${courseId}${unitId ? `/${unitId}` : ''}`;

function coursePlaceOf(address: string): { workspaceId: string; courseId: string; unitId: string } {
  const pathname = new URL(address, 'http://here').pathname;
  const match = new RegExp(`^/w/(${ID})/courses/(${ID})(?:/(${ID}))?$`).exec(pathname);
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
const unitBody = (page: Page): Locator =>
  page.getByTestId('unit-body').locator('[contenteditable]').first();
const carets = (page: Page): Locator => unitBody(page).locator('.doc-caret');

interface DocNode {
  type?: string;
  text?: string;
  content?: DocNode[];
}

const textOf = (node: DocNode | null | undefined): string =>
  node ? (node.text ?? '') + (node.content ?? []).map(textOf).join('') : '';

function tableCellsOf(body: unknown): string[][] {
  const table = (body as DocNode | null)?.content?.find((block) => block.type === 'table');
  return (table?.content ?? []).map((row) => (row.content ?? []).map((cell) => textOf(cell)));
}

async function getJson<T>(api: APIRequestContext, url: string): Promise<T> {
  const response = await api.get(url);
  expect(response.ok(), `${url}: ${await response.text()}`).toBe(true);
  return (await response.json()) as T;
}

async function postJson<T>(api: APIRequestContext, url: string, data: object): Promise<T> {
  const response = await api.post(url, { data });
  expect(response.ok(), `${url}: ${await response.text()}`).toBe(true);
  return (await response.json()) as T;
}

async function userId(api: APIRequestContext): Promise<string> {
  return (await getJson<Schemas['User']>(api, '/api/v1/users/me')).id;
}

async function workspacesOf(api: APIRequestContext): Promise<Schemas['UserWorkspace'][]> {
  return getJson<Schemas['UserWorkspace'][]>(api, '/api/v1/users/me/workspaces');
}

async function storedUnit(
  api: APIRequestContext,
  workspaceId: string,
  courseId: string,
  unitId: string,
): Promise<Schemas['Unit']> {
  return getJson<Schemas['Unit']>(
    api,
    `/api/v1/w/${workspaceId}/courses/${courseId}/units/${unitId}`,
  );
}

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

async function switchWorkspace(page: Page, name: string): Promise<void> {
  await showSidebar(page);
  await sidebar(page).getByTestId('workspace-switcher').click();
  await page
    .getByRole('menu', { name: 'Switch workspace', exact: true })
    .getByRole('menuitem', { name, exact: true })
    .click();
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
  await expect(treeItem(page, title)).toBeAttached();
}

async function nameTheUnit(page: Page, title: string): Promise<void> {
  const field = page.getByTestId('unit-title');
  await expect(field).toBeFocused();
  await page.keyboard.type(title);
  await page.keyboard.press('Enter');
  await expect(treeItem(page, title)).toBeAttached();
}

async function unitAction(page: Page, label: string, action: string): Promise<void> {
  await showTree(page);
  await unitTree(page).getByRole('button', { name: label, exact: true }).click();
  await page.locator(`[data-action="${action}"]`).click();
}

async function signedInPage(
  browser: Browser,
  address: string,
  who: Person,
  onWsJson?: (message: Record<string, unknown>) => void,
): Promise<{ page: Page; errors: string[] }> {
  const context = await browser.newContext({ locale: 'en-US' });
  const page = await context.newPage();
  const errors = watchErrors(page);
  if (onWsJson) {
    await page.routeWebSocket(/\/ws$/, (socket) => {
      const server = socket.connectToServer();
      server.onMessage((message) => {
        if (typeof message === 'string') {
          try {
            const parsed = JSON.parse(message) as unknown;
            if (typeof parsed === 'object' && parsed !== null) {
              onWsJson(parsed as Record<string, unknown>);
            }
          } catch {
            // Non-JSON text frame; ignore.
          }
        }
        socket.send(message);
      });
    });
  }
  await page.goto(address);
  await signInOnPage(page, who);
  return { page, errors };
}

/** Finds the latest room `doc` number joined for `cardId` in captured WS frames. */
function joinedDocOf(messages: readonly Record<string, unknown>[], cardId: string): number {
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const msg = messages[i];
    if (msg?.type === 'docJoined' && msg.cardId === cardId && typeof msg.doc === 'number') {
      return msg.doc;
    }
  }
  throw new Error(`no docJoined observed for ${cardId}`);
}

async function captureCourseScreens(page: Page): Promise<void> {
  if (process.env.FVOCI_SCREENS !== '1') return;
  mkdirSync(COURSE_SCREENS, { recursive: true });
  const address = page.url();
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' });

    // Wide desktop (1280px): outline tree + unit body editor + table + viewers.
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(address);
    await expect(page.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await expect(unitBody(page).locator('table')).toBeVisible();
    await expect(unitTree(page)).toBeVisible();
    await expect(page.getByTestId('unit-heading').getByTestId('page-viewers')).toBeVisible();
    await page.screenshot({
      path: path.join(COURSE_SCREENS, `course-wide-${colorScheme}.png`),
      animations: 'disabled',
    });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      1280,
    );

    // Mobile (390px): unit body editor with drawer button.
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(address);
    await expect(page.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await expect(unitBody(page).locator('table')).toBeVisible();
    await expect(page.getByTestId('open-unit-tree')).toBeVisible();
    await page.screenshot({
      path: path.join(COURSE_SCREENS, `course-mobile-${colorScheme}.png`),
      animations: 'disabled',
    });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      390,
    );

    // Mobile (390px): outline drawer open showing chapter and units.
    await page.getByTestId('open-unit-tree').click();
    await expect(page.getByRole('dialog', { name: 'Units' })).toBeVisible();
    await expect(unitTree(page)).toBeVisible();
    await page.screenshot({
      path: path.join(COURSE_SCREENS, `course-mobile-drawer-${colorScheme}.png`),
      animations: 'disabled',
    });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      390,
    );
  }
  await page.emulateMedia({ colorScheme: null, reducedMotion: 'no-preference' });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto(address);
}

test('own space: builds chapter and units, moves, restores, persists across reload, and co-edits across two browsers', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const learner = person('course-own');
  await register(base, learner);
  const api = await signedIn(base, learner);
  const pages: Page[] = [];
  const watched: string[][] = [];

  try {
    const learnerId = await userId(api);
    const own = (await workspacesOf(api)).find((workspace) => workspace.kind === 'personal');
    if (!own) throw new Error('no personal space');

    const firstSession = await signedInPage(browser, '/', learner);
    const first = firstSession.page;
    pages.push(first);
    watched.push(firstSession.errors);

    await switchWorkspace(first, 'My space');
    await expect(first.getByTestId('workspace-start')).toBeVisible();

    const courseId = await newCourse(first, 'Personal Study', { ownSpace: true });
    await expect(first).toHaveURL(coursePath(own.id, courseId));
    await expect(first.getByTestId('course-empty')).toBeVisible();

    // Add 1 chapter + 3 units in order.
    await addChapter(first, 'Part 1');
    await unitAction(first, 'Menu of the chapter Part 1', 'add-child');
    await expect(first).toHaveURL(ON_A_UNIT);
    await nameTheUnit(first, 'Unit A');
    const unitAId = coursePlaceOf(first.url()).unitId;

    await expect(first.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await unitBody(first).click();
    await first.keyboard.type('Welcome to Unit A.');
    await expect(unitBody(first)).toHaveText('Welcome to Unit A.');

    await showTree(first);
    await first.getByTestId('new-unit').click();
    await nameTheUnit(first, 'Unit B');

    await showTree(first);
    await first.getByTestId('new-unit').click();
    await nameTheUnit(first, 'Unit C');

    await showTree(first);
    await expect.poll(() => treeShape(first)).toEqual(['Part 1', '  Unit A', 'Unit B', 'Unit C']);

    // Move Unit B into Part 1 and before Unit A.
    await treeItem(first, 'Unit B').focus();
    await first.keyboard.press('Alt+ArrowRight');
    await expect.poll(() => treeShape(first)).toEqual(['Part 1', '  Unit A', '  Unit B', 'Unit C']);
    await first.keyboard.press('Alt+ArrowUp');
    await expect.poll(() => treeShape(first)).toEqual(['Part 1', '  Unit B', '  Unit A', 'Unit C']);

    // Delete Unit C and restore from unit trash.
    await unitAction(first, 'Menu of the unit Unit C', 'delete');
    await first.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    await showTree(first);
    await expect.poll(() => treeShape(first)).toEqual(['Part 1', '  Unit B', '  Unit A']);

    await first.getByTestId('open-unit-trash').click();
    const trash = first.getByTestId('unit-trash');
    await expect(trash.getByRole('heading')).toHaveText('Deleted units');
    await trash.getByRole('button', { name: 'Restore Unit C' }).click();
    await expect(trash.getByTestId('trash-empty')).toBeVisible();
    await trash.getByRole('button', { name: 'Close' }).last().click();
    await expect(trash).toBeHidden();
    await expect.poll(() => treeShape(first)).toEqual(['Part 1', '  Unit B', '  Unit A', 'Unit C']);

    // Return to Unit A, wait for DB persistence, and verify full reload restores DB state.
    await treeItem(first, 'Unit A').click();
    await expect(first).toHaveURL(coursePath(own.id, courseId, unitAId));
    await expect
      .poll(
        async () => textOf((await storedUnit(api, own.id, courseId, unitAId)).body as DocNode),
        {
          timeout: 15_000,
        },
      )
      .toBe('Welcome to Unit A.');

    await first.reload();
    await expect(first).toHaveURL(coursePath(own.id, courseId, unitAId));
    await expect(first.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await expect(first.getByTestId('unit-title')).toHaveValue('Unit A');
    await expect(unitBody(first)).toHaveText('Welcome to Unit A.');
    await showTree(first);
    await expect.poll(() => treeShape(first)).toEqual(['Part 1', '  Unit B', '  Unit A', 'Unit C']);

    // Same account in a second browser context co-edits Unit A: text and carets sync.
    const secondSession = await signedInPage(
      browser,
      coursePath(own.id, courseId, unitAId),
      learner,
    );
    const second = secondSession.page;
    pages.push(second);
    watched.push(secondSession.errors);
    await expect(second.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await expect(unitBody(second)).toHaveText('Welcome to Unit A.');

    await unitBody(first).click();
    await lineBoundary(first, 'end');
    await first.keyboard.press('Enter');
    await first.keyboard.type('From desktop.');
    await expect(unitBody(second).locator('p')).toHaveText(['Welcome to Unit A.', 'From desktop.']);

    await unitBody(second).locator('p').nth(1).click();
    await lineBoundary(second, 'end');
    await expect(carets(first)).toHaveCount(1);
    await expect(carets(first)).toHaveAttribute('data-user', learnerId);
    await expect(carets(first).locator('.doc-caret-name')).toHaveAttribute(
      'data-name',
      new RegExp(learner.username),
    );
    await expect(carets(second)).toHaveCount(1);
    await expect(carets(second)).toHaveAttribute('data-user', learnerId);

    await second.keyboard.type(' From tablet.');
    await expect(unitBody(first).locator('p')).toHaveText([
      'Welcome to Unit A.',
      'From desktop. From tablet.',
    ]);
    await expect
      .poll(
        async () => textOf((await storedUnit(api, own.id, courseId, unitAId)).body as DocNode),
        {
          timeout: 15_000,
        },
      )
      .toBe('Welcome to Unit A.From desktop. From tablet.');

    expect(watched.flat()).toEqual([]);
  } finally {
    await Promise.all([...pages.map((p) => p.context().close()), api.dispose()]);
  }
});

test('shared workspace: open course read without joining, joined co-editing with viewers, and immediate docLeft on removal or privatization', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const ada = person('course-ada');
  const bora = person('course-bora');
  await register(base, ada);
  await register(base, bora);
  const adaApi = await signedIn(base, ada);
  const boraApi = await signedIn(base, bora);
  const pages: Page[] = [];
  const watched: string[][] = [];

  try {
    const boraId = await userId(boraApi);
    const shared = (await workspacesOf(adaApi)).find((workspace) => workspace.kind !== 'personal');
    if (!shared) throw new Error('no shared workspace');
    const inWorkspace = `/api/v1/w/${shared.id}`;

    // Joined, an editor: a course left to its default gives who joins it the commenter's role.
    const course = await postJson<Schemas['Container']>(adaApi, `${inWorkspace}/courses`, {
      title: 'Shared Curriculum',
      visibility: 'open',
      minimumRole: 'editor',
    });
    const chapter = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'chapter', title: 'Module 1' },
    );
    const unit = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: 'Lesson 1', parentId: chapter.id },
    );

    const unitUrl = coursePath(shared.id, course.id, unit.id);
    const boraWs: Record<string, unknown>[] = [];
    const firstSession = await signedInPage(browser, unitUrl, ada);
    const first = firstSession.page;
    pages.push(first);
    watched.push(firstSession.errors);
    const secondSession = await signedInPage(
      browser,
      coursePath(shared.id, course.id),
      bora,
      (message) => {
        boraWs.push(message);
      },
    );
    const second = secondSession.page;
    pages.push(second);
    watched.push(secondSession.errors);

    await expect(first.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');

    // Bora sees the open course in the sidebar, opens its first unit read-only without joining.
    await expect(
      sidebar(second).getByRole('link', { name: 'Shared Curriculum, course' }),
    ).toBeVisible();
    await expect(second).toHaveURL(unitUrl);
    await expect(second.locator('h1[data-testid="unit-title"]')).toHaveText('Lesson 1');
    const joinBar = second.getByTestId('course-join');
    await expect(joinBar).toBeVisible();
    await expect(second.getByTestId('unit-body')).toHaveAttribute('data-editor', 'reading');
    await expect(second.getByTestId('unit-body').locator('[contenteditable="true"]')).toHaveCount(
      0,
    );

    // Live updates from Ada reach Bora even before joining.
    await unitBody(first).click();
    await first.keyboard.type('Ada writes.');
    await expect(unitBody(second)).toHaveText('Ada writes.');

    // Bora joins and co-edits with Ada; both see PageViewers and synced edits.
    await joinBar.getByRole('button').click();
    await expect(joinBar).toHaveCount(0);
    await expect(second.getByTestId('unit-title')).toHaveValue('Lesson 1');
    await expect(second.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');

    await unitBody(second).click();
    await lineBoundary(second, 'end');
    await second.keyboard.type(' Bora adds.');
    for (const page of [first, second]) {
      await expect(unitBody(page)).toHaveText('Ada writes. Bora adds.');
      await expect(page.getByTestId('unit-heading').getByTestId('page-viewers')).toBeVisible();
    }
    await expect
      .poll(
        async () =>
          textOf((await storedUnit(adaApi, shared.id, course.id, unit.id)).body as DocNode),
        { timeout: 15_000 },
      )
      .toBe('Ada writes. Bora adds.');

    // Live outline sync over WebSocket: Ada creates, renames, moves, and deletes units;
    // Bora's tree updates live, and deleting Bora's open unit notices and moves her to the next unit.
    const unit2 = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: 'Lesson 2', parentId: chapter.id },
    );
    await showTree(second);
    await expect.poll(() => treeShape(second)).toEqual(['Module 1', '  Lesson 1', '  Lesson 2']);

    const renamedSecond = await adaApi.patch(
      `${inWorkspace}/courses/${course.id}/units/${unit2.id}`,
      { data: { title: 'Lesson 2 (Revised)' } },
    );
    expect(renamedSecond.ok(), await renamedSecond.text()).toBe(true);
    await expect(treeItem(second, 'Lesson 2 (Revised)')).toBeVisible();

    const movedSecond = await adaApi.post(
      `${inWorkspace}/courses/${course.id}/units/${unit2.id}/move`,
      { data: { parentId: chapter.id, after: '' } },
    );
    expect(movedSecond.ok(), await movedSecond.text()).toBe(true);
    await expect
      .poll(() => treeShape(second))
      .toEqual(['Module 1', '  Lesson 2 (Revised)', '  Lesson 1']);

    const deletedFirst = await adaApi.delete(
      `${inWorkspace}/courses/${course.id}/units/${unit.id}`,
    );
    expect(deletedFirst.ok(), await deletedFirst.text()).toBe(true);
    await expect(second.getByTestId('course-notice')).toContainText('Lesson 1');
    const unit2Url = coursePath(shared.id, course.id, unit2.id);
    await expect(second).toHaveURL(unit2Url);
    await expect(second.getByTestId('unit-title')).toHaveValue('Lesson 2 (Revised)');
    await expect(second.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');

    // 1) Removing Bora from the open course demotes her to read-only, then making the course
    //    private immediately kicks her out of the room (docLeft) and blocks reading.
    const removedOpen = await adaApi.delete(
      `${inWorkspace}/containers/${course.id}/members/${boraId}`,
    );
    expect(removedOpen.ok(), await removedOpen.text()).toBe(true);
    await expect(second.getByTestId('course-join')).toBeVisible();
    await expect(second.getByTestId('unit-body')).toHaveAttribute('data-editor', 'reading');

    const openDocId = joinedDocOf(boraWs, unit2.id);
    boraWs.length = 0;
    const madePrivate = await adaApi.patch(`${inWorkspace}/containers/${course.id}`, {
      data: { visibility: 'private' },
    });
    expect(madePrivate.ok(), await madePrivate.text()).toBe(true);
    await expect
      .poll(
        () =>
          boraWs.some(
            (msg) => msg.type === 'docLeft' && msg.doc === openDocId && msg.reason === 'not_found',
          ),
        { timeout: 15_000 },
      )
      .toBe(true);
    await expect(second.getByTestId('not-found')).toBeVisible();
    await expect(courseScreen(second)).toHaveCount(0);
    expect(
      (await boraApi.get(`${inWorkspace}/courses/${course.id}/units/${unit2.id}`)).status(),
    ).toBe(404);

    // 2) Re-adding Bora to the private course and then removing her directly also triggers
    //    immediate docLeft and blocks reading.
    await postJson(adaApi, `${inWorkspace}/containers/${course.id}/members`, {
      userId: boraId,
      role: 'editor',
    });
    boraWs.length = 0;
    await second.goto(unit2Url);
    await expect(second.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');

    const privateDocId = joinedDocOf(boraWs, unit2.id);
    boraWs.length = 0;
    const removedPrivate = await adaApi.delete(
      `${inWorkspace}/containers/${course.id}/members/${boraId}`,
    );
    expect(removedPrivate.ok(), await removedPrivate.text()).toBe(true);
    await expect
      .poll(
        () =>
          boraWs.some(
            (msg) =>
              msg.type === 'docLeft' && msg.doc === privateDocId && msg.reason === 'not_found',
          ),
        { timeout: 15_000 },
      )
      .toBe(true);
    await expect(second.getByTestId('not-found')).toBeVisible();
    await expect(courseScreen(second)).toHaveCount(0);
    expect(
      (await boraApi.get(`${inWorkspace}/courses/${course.id}/units/${unit2.id}`)).status(),
    ).toBe(404);

    expect(watched.flat()).toEqual([]);
  } finally {
    await Promise.all([
      ...pages.map((p) => p.context().close()),
      adaApi.dispose(),
      boraApi.dispose(),
    ]);
  }
});

test('unit body table: inserts, syncs live between two editors, persists in DB across reload, and captures screens', async ({
  browser,
  baseURL,
}) => {
  test.slow(Boolean(process.env.FVOCI_SCREENS));
  const base = baseURL ?? '';
  const ada = person('table-ada');
  const bora = person('table-bora');
  await register(base, ada);
  await register(base, bora);
  const adaApi = await signedIn(base, ada);
  const boraApi = await signedIn(base, bora);
  const pages: Page[] = [];
  const watched: string[][] = [];

  try {
    const boraId = await userId(boraApi);
    const shared = (await workspacesOf(adaApi)).find((workspace) => workspace.kind !== 'personal');
    if (!shared) throw new Error('no shared workspace');
    const inWorkspace = `/api/v1/w/${shared.id}`;

    const course = await postJson<Schemas['Container']>(adaApi, `${inWorkspace}/courses`, {
      title: 'Calculus 101',
      description: 'Limits, derivatives, and integrals.',
      visibility: 'private',
    });
    await postJson(adaApi, `${inWorkspace}/containers/${course.id}/members`, {
      userId: boraId,
      role: 'editor',
    });

    const chapter = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'chapter', title: 'Chapter 1: Foundations' },
    );
    const unit1 = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: '1.1 Limits and Continuity', parentId: chapter.id },
    );
    await postJson<Schemas['Unit']>(adaApi, `${inWorkspace}/courses/${course.id}/units`, {
      kind: 'unit',
      title: '1.2 Derivatives',
      parentId: chapter.id,
    });
    await postJson<Schemas['Unit']>(adaApi, `${inWorkspace}/courses/${course.id}/units`, {
      kind: 'unit',
      title: 'Practice Exercises',
    });

    const address = coursePath(shared.id, course.id, unit1.id);
    const firstSession = await signedInPage(browser, address, ada);
    const first = firstSession.page;
    pages.push(first);
    watched.push(firstSession.errors);
    const secondSession = await signedInPage(browser, address, bora);
    const second = secondSession.page;
    pages.push(second);
    watched.push(secondSession.errors);

    for (const page of [first, second]) {
      await expect(page.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    }

    await unitBody(first).click();
    await first.keyboard.type('## Weekly schedule');
    await first.keyboard.press('Enter');
    await first.keyboard.type('Each session covers a core concept and practice problems.');
    await first.keyboard.press('Enter');
    await paste(unitBody(first), {
      'text/plain': '| Day | Topic |\n|---|---|\n| Mon | Limits |\n| Wed | Derivatives |\n',
    });

    const initial = [
      ['Day', 'Topic'],
      ['Mon', 'Limits'],
      ['Wed', 'Derivatives'],
    ];
    await expect.poll(() => cellsOf(unitBody(second)), { timeout: 15_000 }).toEqual(initial);

    // Second editor updates a cell; first editor and DB both get the update.
    await unitBody(second).locator('table td').last().click();
    await lineBoundary(second, 'end');
    await second.keyboard.type(' & Integrals');
    const updated = [
      ['Day', 'Topic'],
      ['Mon', 'Limits'],
      ['Wed', 'Derivatives & Integrals'],
    ];
    await expect.poll(() => cellsOf(unitBody(first)), { timeout: 15_000 }).toEqual(updated);
    await expect
      .poll(
        async () => tableCellsOf((await storedUnit(adaApi, shared.id, course.id, unit1.id)).body),
        { timeout: 15_000 },
      )
      .toEqual(updated);

    // Full browser reload restores the table intact from the database.
    await first.reload();
    await expect(first.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');
    await expect.poll(() => cellsOf(unitBody(first)), { timeout: 15_000 }).toEqual(updated);

    // Move second editor's focus off the table cell so Bora stays visible in PageViewers
    // without her caret label overlapping the mobile table cells in the screenshots.
    await second.getByTestId('unit-title').focus();
    await second.getByTestId('unit-title').blur();
    await captureCourseScreens(first);

    expect(watched.flat()).toEqual([]);
  } finally {
    await Promise.all([
      ...pages.map((p) => p.context().close()),
      adaApi.dispose(),
      boraApi.dispose(),
    ]);
  }
});

test('boundaries and search: rejects board ID at course URL and finds a unit via workspace search', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('course-search');
  await register(base, owner);
  const api = await signedIn(base, owner);

  try {
    const board = await makeBoard(api, { title: 'Roadmap' });
    const { workspaceId } = board;
    const inWorkspace = `/api/v1/w/${workspaceId}`;

    const word = `narwhal${Date.now().toString(36)}`;
    const course = await postJson<Schemas['Container']>(api, `${inWorkspace}/courses`, {
      title: 'Marine Biology',
      visibility: 'private',
    });
    const created = await postJson<Schemas['Unit']>(
      api,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: 'Arctic Migration' },
    );
    const patched = await api.patch(`${inWorkspace}/courses/${course.id}/units/${created.id}`, {
      data: {
        body: {
          type: 'doc',
          content: [
            { type: 'paragraph', content: [{ type: 'text', text: `Tracking ${word} pods` }] },
          ],
        },
        readAt: created.updatedAt,
      },
    });
    expect(patched.ok(), await patched.text()).toBe(true);

    await expect
      .poll(
        async () => {
          const answer = await api.get(`${inWorkspace}/search`, { params: { q: word } });
          expect(answer.ok(), await answer.text()).toBe(true);
          return ((await answer.json()) as Schemas['SearchHit'][]).map((hit) => hit.id);
        },
        { timeout: 15_000 },
      )
      .toEqual([created.id]);

    const errors = watchErrors(page);
    await page.goto('/');
    await signInOnPage(page, owner);

    // A board ID at a course URL shows NotFound, and a course ID at a board URL shows NotFound.
    await page.goto(coursePath(workspaceId, board.boardId));
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(courseScreen(page)).toHaveCount(0);

    await page.goto(boardPath({ workspaceId, boardId: course.id }));
    await expect(page.getByTestId('not-found')).toBeVisible();
    await expect(page.getByTestId('board-page')).toHaveCount(0);

    // From the board, search for the unit's body word and navigate to the unit.
    await page.goto(boardPath(board));
    await expect(page.getByTestId('board-page')).toBeVisible();
    await expect(page.getByTestId('open-search')).toBeVisible();
    await page.keyboard.press(await primaryKey(page, 'k'));
    const dialog = page.getByTestId('search-dialog');
    await dialog.getByTestId('search-field').fill(word);
    await expect(dialog).toHaveAttribute('data-status', 'found');
    const hits = dialog.getByRole('option');
    await expect(hits).toHaveCount(1);
    await expect(hits.first()).toHaveAttribute('data-kind', 'unit');
    await expect(hits.first().locator('[data-hit-container]')).toHaveText('Marine Biology');
    await expect(hits.first().locator('mark')).toHaveText(word);
    await page.keyboard.press('Enter');

    await expect(page).toHaveURL(coursePath(workspaceId, course.id, created.id));
    await expect(page.getByTestId('unit-title')).toHaveValue('Arctic Migration');
    await expect(unitBody(page)).toContainText(word);

    expect(errors).toEqual([]);
  } finally {
    await api.dispose();
  }
});
