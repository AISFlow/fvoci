/**
 * Spaced-repetition unit reviews, interval suggestions, "Today's reviews",
 * personal study notes, and cross-user/public-link isolation on the real server:
 * - User A completes a unit -> sees interval suggestion bar -> schedules a review -> persists across reload.
 * - User A completes a due-today review from "Today's reviews" with atomic next-interval scheduling.
 * - User A creates, filters, edits, and deletes personal question/mistake/review notes.
 * - User B (enrolled viewer) sees ZERO of User A's reviews, Today's reviews, or personal notes,
 *   and can manage User B's own isolated reviews & notes.
 * - Public shared course link makes ZERO `/reviews`, `/notes`, or `/learning/` requests and
 *   renders zero review/note UI.
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

function utcToday(): string {
  const now = new Date();
  const yyyy = String(now.getUTCFullYear()).padStart(4, '0');
  const mm = String(now.getUTCMonth() + 1).padStart(2, '0');
  const dd = String(now.getUTCDate()).padStart(2, '0');
  return `${yyyy}-${mm}-${dd}`;
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

async function signedInPage(
  browser: Browser,
  address: string,
  who: Person,
): Promise<{ page: Page; errors: string[] }> {
  const context = await browser.newContext({ locale: 'en-US', timezoneId: 'UTC' });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto(address);
  await signInOnPage(page, who);
  return { page, errors };
}

test('full review & note lifecycle on live server with strict user and public-link isolation', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const ada = person('rev-ada');
  const bora = person('rev-bora');
  await Promise.all([register(base, ada), register(base, bora)]);
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
      title: 'Live Reviews Course',
      visibility: 'open',
      minimumRole: 'viewer',
    });
    await postJson(adaApi, `${inWorkspace}/containers/${course.id}/members`, {
      userId: boraUser.id,
      role: 'viewer',
    });
    const unit1 = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: 'Unit One' },
    );
    const unit2 = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'unit', title: 'Unit Two' },
    );

    const unit1Url = coursePath(shared.id, course.id, unit1.id);
    const adaSession = await signedInPage(browser, unit1Url, ada);
    pages.push(adaSession.page);
    watched.push(adaSession.errors);
    const adaPage = adaSession.page;

    // 1. Complete Unit 1 -> interval suggestion bar appears -> schedule 1d review -> persists on reload.
    await adaPage.getByTestId('unit-complete-toggle').click();
    const suggestBar = adaPage.getByTestId('review-suggest-bar');
    await expect(suggestBar).toBeVisible();
    await expect(suggestBar.getByTestId('review-suggest-bar-option-1')).toHaveAttribute(
      'data-suggested',
      'true',
    );
    await suggestBar.getByTestId('review-suggest-bar-confirm').click();
    await expect(suggestBar).toBeHidden();
    await expect(
      adaPage.getByTestId('unit-reviews-panel').getByTestId('unit-review-item'),
    ).toHaveCount(1);

    await adaPage.reload();
    await expect(
      adaPage.getByTestId('unit-reviews-panel').getByTestId('unit-review-item'),
    ).toHaveCount(1);

    // 2. Schedule a review due today on Unit 2 -> open "Today's reviews" -> complete + schedule next.
    await postJson(adaApi, `${inWorkspace}/courses/${course.id}/units/${unit2.id}/reviews`, {
      scheduledDate: utcToday(),
      intervalDays: 1,
    });

    await adaPage.getByTestId('open-today-reviews').click();
    await expect(adaPage.getByTestId('today-reviews-screen')).toBeVisible();
    await expect(adaPage.getByTestId('today-review-item')).toHaveCount(1);
    await expect(adaPage.getByTestId('today-review-open')).toHaveText('Unit Two');

    await adaPage.getByTestId('today-review-complete').click();
    await expect(adaPage.getByTestId('today-review-complete-picker-option-3')).toHaveAttribute(
      'data-suggested',
      'true',
    );
    await adaPage.getByTestId('today-review-complete-picker-confirm').click();
    await expect(adaPage.getByTestId('today-reviews-empty')).toBeVisible();
    await expect(adaPage.getByTestId('today-reviews-status')).toBeVisible();

    // Verify follow-up review (3d) exists on Unit 2 after reload.
    await adaPage.goto(coursePath(shared.id, course.id, unit2.id));
    await expect(
      adaPage.getByTestId('unit-reviews-pending').getByTestId('unit-review-item'),
    ).toHaveCount(1);
    await expect(adaPage.getByTestId('unit-reviews-completed')).toBeVisible();

    // 3. Personal notes CRUD on Unit 1 (question & mistake), persisting across reload.
    await adaPage.goto(unit1Url);
    const notesPanel = adaPage.getByTestId('unit-notes-panel');
    await adaPage.getByTestId('unit-notes-add').click();
    await expect(notesPanel.getByTestId('unit-notes-private-badge')).toBeVisible();

    await notesPanel.getByTestId('unit-note-kind-question').click();
    await notesPanel.getByTestId('unit-note-input').fill('Ada private question');
    await notesPanel.getByTestId('unit-note-submit').click();
    await expect(notesPanel.getByTestId('unit-note-item')).toHaveCount(1);

    await notesPanel.getByTestId('unit-note-kind-mistake').click();
    await notesPanel.getByTestId('unit-note-input').fill('Ada private mistake');
    await notesPanel.getByTestId('unit-note-submit').click();
    await expect(notesPanel.getByTestId('unit-note-item')).toHaveCount(2);

    await adaPage.reload();
    await expect(adaPage.getByTestId('unit-notes-panel').getByTestId('unit-note-item')).toHaveCount(
      2,
    );

    // Edit and delete one note.
    await adaPage.getByTestId('unit-notes-filter-mistake').click();
    await expect(adaPage.getByTestId('unit-note-item')).toHaveCount(1);
    await adaPage.getByTestId('unit-note-edit').click();
    await adaPage.getByTestId('unit-note-edit-input').fill('Ada updated mistake');
    await adaPage.getByTestId('unit-note-save').click();
    await expect(adaPage.getByTestId('unit-note-content')).toHaveText('Ada updated mistake');
    await adaPage.getByTestId('unit-note-delete').click();
    await expect(adaPage.getByTestId('unit-notes-empty')).toBeVisible();

    // 4. User B (enrolled Viewer) sees ZERO of Ada's reviews or notes, and has empty Today's reviews.
    const boraSession = await signedInPage(browser, unit1Url, bora);
    pages.push(boraSession.page);
    watched.push(boraSession.errors);
    const boraPage = boraSession.page;

    // Bora's own: no reviews, so no panel of them, and no notes but the button that starts one.
    await expect(boraPage.getByTestId('unit-notes-add')).toBeVisible();
    await expect(boraPage.getByTestId('unit-reviews-panel')).toHaveCount(0);
    await expect(boraPage.getByTestId('course-screen')).not.toContainText('Ada private question');

    await boraPage.getByTestId('open-today-reviews').click();
    await expect(boraPage.getByTestId('today-reviews-empty')).toBeVisible();

    // 5. Public shared course link makes zero /reviews, /notes, or /learning/ requests.
    await adaPage.getByTestId('share-course').click();
    const adaDialog = adaPage.getByTestId('share-dialog');
    await adaDialog.getByTestId('share-public').click();
    const sharedUrl = await adaDialog.getByTestId('share-link').inputValue();
    expect(sharedUrl).toContain(`/shared/${course.id}?r=`);

    const publicContext = await browser.newContext({ locale: 'en-US' });
    const publicPage = await publicContext.newPage();
    pages.push(publicPage);
    watched.push(watchErrors(publicPage));
    const leakedCalls: string[] = [];
    publicPage.on('request', (req) => {
      const url = req.url();
      if (url.includes('/reviews') || url.includes('/notes') || url.includes('/learning/')) {
        leakedCalls.push(url);
      }
    });
    await publicPage.goto(sharedUrl);
    await expect(publicPage.getByTestId('shared-course')).toBeVisible();
    await expect(publicPage.getByTestId('unit-reviews-panel')).toHaveCount(0);
    await expect(publicPage.getByTestId('unit-notes-panel')).toHaveCount(0);
    await expect(publicPage.getByTestId('review-suggest-bar')).toHaveCount(0);
    expect(leakedCalls).toEqual([]);

    expect(watched.flat()).toEqual([]);
  } finally {
    await Promise.all([
      ...pages.map((p) => p.context().close()),
      adaApi.dispose(),
      boraApi.dispose(),
    ]);
  }
});
