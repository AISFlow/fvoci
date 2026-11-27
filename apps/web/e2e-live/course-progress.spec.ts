/**
 * Course personal progress & member progress summary on the real server:
 * - User A (admin) completes Unit 1 and saves `lastPosition`, persisting across reload.
 * - User B (viewer) sees Unit 1 incomplete, never sees A's `lastPosition`, sees only A's
 *   aggregate completion rate in the Members dialog, and can record B's own completion &
 *   `lastPosition` (persisting across reload).
 * - Unjoined User C opens an open course read-only (no progress toggle, the course's banner asks),
 *   joins, and tracks progress.
 * - Public shared course link makes zero `/progress` or `/progress/summary` requests and
 *   displays no progress UI.
 */
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test';

import {
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  type Person,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

const coursePath = (workspaceId: string, courseId: string, unitId?: string): string =>
  `/w/${workspaceId}/courses/${courseId}${unitId ? `/${unitId}` : ''}`;

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

async function signedInPage(
  browser: Browser,
  address: string,
  who: Person,
): Promise<{ page: Page; errors: string[] }> {
  const context = await browser.newContext({ locale: 'en-US' });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto(address);
  await signInOnPage(page, who);
  return { page, errors };
}

test('isolates personal progress & lastPosition between members, shows only aggregate rate in Members dialog, and persists across reload', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const ada = person('prog-ada');
  const bora = person('prog-bora');
  const chloe = person('prog-chloe');
  await Promise.all([register(base, ada), register(base, bora), register(base, chloe)]);
  const [adaApi, boraApi] = await Promise.all([signedIn(base, ada), signedIn(base, bora)]);
  const pages: Page[] = [];
  const watched: string[][] = [];

  try {
    const boraUser = await getJson<Schemas['User']>(boraApi, '/api/v1/users/me');
    const workspaces = await getJson<Schemas['UserWorkspace'][]>(
      adaApi,
      '/api/v1/users/me/workspaces',
    );
    const shared = workspaces.find((ws) => ws.kind !== 'personal');
    if (!shared) throw new Error('no shared workspace');
    const inWorkspace = `/api/v1/w/${shared.id}`;

    const course = await postJson<Schemas['Container']>(adaApi, `${inWorkspace}/courses`, {
      title: 'Live Progress Study',
      visibility: 'open',
      minimumRole: 'viewer',
    });
    // Add Bora as a Viewer (read-only on content, allowed to track personal progress).
    await postJson(adaApi, `${inWorkspace}/containers/${course.id}/members`, {
      userId: boraUser.id,
      role: 'viewer',
    });
    const chapter = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'chapter', title: 'Part 1' },
    );
    const unit1 = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: 'Unit Alpha', parentId: chapter.id },
    );
    const unit2 = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: 'Unit Beta', parentId: chapter.id },
    );

    const unit1Url = coursePath(shared.id, course.id, unit1.id);
    const adaSession = await signedInPage(browser, unit1Url, ada);
    pages.push(adaSession.page);
    watched.push(adaSession.errors);
    const adaPage = adaSession.page;

    await expect(adaPage.getByTestId('course-progress-text')).toHaveText('0% (0/2 units)');
    await adaPage.getByTestId('unit-complete-toggle').click();
    await expect(adaPage.getByTestId('unit-complete-toggle')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(adaPage.getByTestId('course-progress-text')).toHaveText('50% (1/2 units)');

    await adaPage.getByTestId('unit-last-position-add').click();
    await adaPage.getByTestId('unit-last-position-input').fill('12:30');
    await adaPage.getByTestId('unit-last-position-save').click();
    await expect(adaPage.getByTestId('unit-last-position-value')).toHaveText('12:30');

    // Reload Ada's browser: completion and lastPosition persist in PostgreSQL.
    await adaPage.reload();
    await expect(adaPage.getByTestId('course-progress-text')).toHaveText('50% (1/2 units)');
    await expect(adaPage.getByTestId('unit-complete-toggle')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(adaPage.getByTestId('unit-last-position-value')).toHaveText('12:30');

    // Bora (Viewer) opens Unit Alpha: sees it incomplete, never sees Ada's lastPosition.
    const boraSession = await signedInPage(browser, unit1Url, bora);
    pages.push(boraSession.page);
    watched.push(boraSession.errors);
    const boraPage = boraSession.page;

    await expect(boraPage.getByTestId('course-progress-text')).toHaveText('0% (0/2 units)');
    await expect(boraPage.getByTestId('unit-complete-toggle')).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    await expect(boraPage.getByTestId('course-screen')).not.toContainText('12:30');

    // Bora opens Members dialog: sees Ada's aggregate rate (50%) and own (0%), never Ada's unit or position.
    await boraPage.getByTestId('share-course').click();
    const boraDialog = boraPage.getByTestId('share-dialog');
    await expect(boraDialog).toBeVisible();
    await expect(
      boraDialog.locator(`[data-member="${ada.username}"]`).getByTestId('member-course-progress'),
    ).toHaveText('50% (1/2 units)');
    await expect(
      boraDialog.locator(`[data-member="${bora.username}"]`).getByTestId('member-course-progress'),
    ).toHaveText('0% (0/2 units)');
    await expect(boraDialog).not.toContainText('12:30');
    await expect(boraDialog).not.toContainText('Unit Alpha');
    await boraPage.keyboard.press('Escape');
    await expect(boraDialog).toBeHidden();

    // Bora (Viewer) navigates to Unit Beta, marks it complete & records own lastPosition.
    await boraPage.goto(coursePath(shared.id, course.id, unit2.id));
    await boraPage.getByTestId('unit-complete-toggle').click();
    await expect(boraPage.getByTestId('unit-complete-toggle')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await boraPage.getByTestId('unit-last-position-add').click();
    await boraPage.getByTestId('unit-last-position-input').fill('p.42');
    await boraPage.getByTestId('unit-last-position-save').click();
    await expect(boraPage.getByTestId('unit-last-position-value')).toHaveText('p.42');

    await boraPage.reload();
    await expect(boraPage.getByTestId('course-progress-text')).toHaveText('50% (1/2 units)');
    await expect(boraPage.getByTestId('unit-complete-toggle')).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    await expect(boraPage.getByTestId('unit-last-position-value')).toHaveText('p.42');

    // Ada opens Share dialog: sees Bora's refreshed aggregate rate (50%) without unit or position.
    await adaPage.getByTestId('share-course').click();
    const adaDialog = adaPage.getByTestId('share-dialog');
    await expect(
      adaDialog.locator(`[data-member="${bora.username}"]`).getByTestId('member-course-progress'),
    ).toHaveText('50% (1/2 units)');
    await expect(adaDialog).not.toContainText('p.42');
    await expect(adaDialog).not.toContainText('Unit Beta');

    // Enable public sharing link from Ada's dialog and verify public reader makes zero progress calls.
    await adaDialog.getByTestId('share-public').click();
    const sharedUrl = await adaDialog.getByTestId('share-link').inputValue();
    expect(sharedUrl).toContain(`/shared/${course.id}?r=`);

    const publicContext = await browser.newContext({ locale: 'en-US' });
    const publicPage = await publicContext.newPage();
    pages.push(publicPage);
    watched.push(watchErrors(publicPage));
    const progressRequests: string[] = [];
    publicPage.on('request', (req) => {
      if (req.url().includes('/progress')) progressRequests.push(req.url());
    });
    await publicPage.goto(sharedUrl);
    await expect(publicPage.getByTestId('shared-course')).toBeVisible();
    await expect(publicPage.getByTestId('unit-title')).toHaveText('Unit Alpha');
    await expect(publicPage.getByTestId('course-my-progress')).toHaveCount(0);
    await expect(publicPage.getByTestId('unit-complete-toggle')).toHaveCount(0);
    await expect(publicPage.getByTestId('unit-tree-progress')).toHaveCount(0);
    expect(progressRequests).toEqual([]);

    // Unjoined Chloe opens the open course: no toggle, the banner asks; joins -> tracks progress.
    const chloeSession = await signedInPage(browser, unit1Url, chloe);
    pages.push(chloeSession.page);
    watched.push(chloeSession.errors);
    const chloePage = chloeSession.page;

    await expect(chloePage.getByTestId('course-join')).toBeVisible();
    await expect(chloePage.getByTestId('unit-complete-toggle')).toHaveCount(0);
    await expect(chloePage.getByTestId('course-progress-join-hint')).toHaveCount(0);
    await chloePage.getByTestId('course-join').getByRole('button').click();
    await expect(chloePage.getByTestId('course-join')).toHaveCount(0);
    await expect(chloePage.getByTestId('unit-complete-toggle')).toBeEnabled();
    await chloePage.getByTestId('unit-complete-toggle').click();
    await expect(chloePage.getByTestId('course-progress-text')).toHaveText('50% (1/2 units)');

    expect(watched.flat()).toEqual([]);
  } finally {
    await Promise.all([
      ...pages.map((p) => p.context().close()),
      adaApi.dispose(),
      boraApi.dispose(),
    ]);
  }
});
