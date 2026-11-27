/**
 * Course study schedules, proportional auto-assignment, overdue retention
 * (no automatic shifting), personal redistribution isolation, "Today's learning",
 * and public-link isolation against the real server (`e2e:live`).
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
  const context = await browser.newContext({ locale: 'en-US', timezoneId: 'UTC' });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto(address);
  await signInOnPage(page, who);
  return { page, errors };
}

function utcOffsetDate(days: number): string {
  const date = new Date(Date.now() + days * 86_400_000);
  const y = date.getUTCFullYear();
  const m = String(date.getUTCMonth() + 1).padStart(2, '0');
  const d = String(date.getUTCDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

test('saves schedule with auto-assign, preserves overdue units, isolates viewer personal redistribution, surfaces Today’s learning, and hides schedule on public links', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const ada = person('sched-ada');
  const bora = person('sched-bora');
  const chloe = person('sched-chloe');
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
      title: 'Live Schedule Course',
      visibility: 'open',
      minimumRole: 'viewer',
    });
    await postJson(adaApi, `${inWorkspace}/containers/${course.id}/members`, {
      userId: boraUser.id,
      role: 'viewer',
    });
    const chapter = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      { kind: 'chapter', title: 'Week 1' },
    );
    const unit1 = await postJson<Schemas['Unit']>(
      adaApi,
      `${inWorkspace}/courses/${course.id}/units`,
      {
        kind: 'unit',
        title: 'Unit Past',
        parentId: chapter.id,
        amount: 40,
        amountUnit: 'minutes',
      },
    );
    await postJson<Schemas['Unit']>(adaApi, `${inWorkspace}/courses/${course.id}/units`, {
      kind: 'unit',
      title: 'Unit Today',
      parentId: chapter.id,
      amount: 20,
      amountUnit: 'pages',
    });

    const unit1Url = coursePath(shared.id, course.id, unit1.id);
    const adaSession = await signedInPage(browser, unit1Url, ada);
    pages.push(adaSession.page);
    watched.push(adaSession.errors);
    const adaPage = adaSession.page;

    // Ada saves schedule from 3 days ago to today with all 7 study days and autoAssign.
    await expect(adaPage.getByTestId('unit-amount-value')).toHaveText('40 min');
    await adaPage.getByTestId('open-course-schedule').click();
    const adaDialog = adaPage.getByTestId('course-schedule-dialog');
    await expect(adaDialog).toBeVisible();
    await adaDialog.getByTestId('schedule-start-date').fill(utcOffsetDate(-3));
    await adaDialog.getByTestId('schedule-target-date').fill(utcOffsetDate(0));
    for (const key of ['sat', 'sun'] as const) {
      const btn = adaDialog.getByTestId(`schedule-day-${key}`);
      if ((await btn.getAttribute('aria-pressed')) === 'false') await btn.click();
    }
    await adaDialog.getByTestId('schedule-save').click();
    await expect(adaDialog.getByTestId('schedule-dialog-overdue-count')).toHaveText('Overdue 1');
    await expect(adaDialog.getByTestId('schedule-dialog-today-count')).toHaveText('Today 1');
    await adaDialog.getByTestId('close-course-schedule').click();
    await expect(adaPage.getByTestId('course-schedule-no-auto-shift')).toBeVisible();

    // Reload Ada: schedule & overdue status persist (no auto-shift).
    await adaPage.reload();
    await expect(adaPage.getByTestId('course-schedule-overdue-count')).toHaveText('Overdue 1');
    await expect(adaPage.getByTestId('course-schedule-today-count')).toHaveText('Today 1');
    await expect(adaPage.getByTestId('unit-status-overdue')).toBeVisible();

    // Bora (Viewer) opens the course: sees overdue, cannot redistribute shared, CAN redistribute own.
    const boraSession = await signedInPage(browser, unit1Url, bora);
    pages.push(boraSession.page);
    watched.push(boraSession.errors);
    const boraPage = boraSession.page;

    await expect(boraPage.getByTestId('course-schedule-overdue-count')).toHaveText('Overdue 1');
    await boraPage.getByTestId('open-course-schedule').click();
    const boraDialog = boraPage.getByTestId('course-schedule-dialog');
    await expect(boraDialog).toBeVisible();
    await expect(boraDialog.getByTestId('schedule-save')).toHaveCount(0);
    await expect(boraDialog.getByTestId('redistribute-shared-schedule')).toHaveCount(0);

    // Bora redistributes remaining units into future dates (tomorrow..+4 days).
    await boraDialog.getByTestId('schedule-from-date').fill(utcOffsetDate(1));
    await boraDialog.getByTestId('schedule-target-date').fill(utcOffsetDate(4));
    await boraDialog.getByTestId('redistribute-my-schedule').click();
    await expect(boraDialog.getByTestId('schedule-dialog-overdue-count')).toHaveText('Overdue 0');
    await boraDialog.getByTestId('close-course-schedule').click();

    await boraPage.reload();
    await expect(boraPage.getByTestId('course-schedule-overdue-count')).toHaveText('Overdue 0');
    await expect(boraPage.getByTestId('unit-my-date-text')).toBeVisible();

    // Ada reloads: Bora's personal redistribution did NOT touch Ada's schedule or overdue count!
    await adaPage.reload();
    await expect(adaPage.getByTestId('course-schedule-overdue-count')).toHaveText('Overdue 1');
    await expect(adaPage.getByTestId('unit-my-date-text')).toHaveCount(0);

    // Ada opens "Today's learning" from sidebar button and completes Unit Past.
    await adaPage.getByTestId('open-learning-today').click();
    await expect(adaPage.getByTestId('learning-today-view')).toBeVisible();
    await expect(adaPage.getByTestId('learning-overdue-item')).toHaveCount(1);
    await expect(adaPage.getByTestId('learning-overdue-item')).toContainText('Unit Past');
    await expect(adaPage.getByTestId('learning-today-item')).toHaveCount(1);
    await expect(adaPage.getByTestId('learning-today-item')).toContainText('Unit Today');

    await adaPage
      .getByTestId('learning-overdue-item')
      .getByTestId('learning-item-complete')
      .click();
    await expect(adaPage.getByTestId('learning-overdue-item')).toHaveCount(0);
    await adaPage.reload();
    await expect(adaPage.getByTestId('learning-overdue-item')).toHaveCount(0);
    await expect(adaPage.getByTestId('learning-today-item')).toHaveCount(1);

    // Unjoined Chloe sees empty Today's learning and the banner alone on the open course.
    const chloeSession = await signedInPage(browser, `/w/${shared.id}/learning/today`, chloe);
    pages.push(chloeSession.page);
    watched.push(chloeSession.errors);
    const chloePage = chloeSession.page;
    await expect(chloePage.getByTestId('learning-today-empty')).toBeVisible();
    await chloePage.goto(unit1Url);
    await expect(chloePage.getByTestId('unit-my-date-join-hint')).toHaveCount(0);
    await expect(chloePage.getByTestId('unit-my-date-input')).toHaveCount(0);

    // Public sharing link makes zero /schedule or /learning/today requests and shows no schedule UI.
    await adaPage.goto(unit1Url);
    await adaPage.getByTestId('share-course').click();
    const shareDialog = adaPage.getByTestId('share-dialog');
    await shareDialog.getByTestId('share-public').click();
    const sharedUrl = await shareDialog.getByTestId('share-link').inputValue();

    const publicContext = await browser.newContext({ locale: 'en-US' });
    const publicPage = await publicContext.newPage();
    pages.push(publicPage);
    watched.push(watchErrors(publicPage));
    const forbiddenRequests: string[] = [];
    publicPage.on('request', (req) => {
      if (req.url().includes('/schedule') || req.url().includes('/learning/today')) {
        forbiddenRequests.push(req.url());
      }
    });
    await publicPage.goto(sharedUrl);
    await expect(publicPage.getByTestId('shared-course')).toBeVisible();
    await expect(publicPage.getByTestId('unit-title')).toHaveText('Unit Past');
    await expect(publicPage.getByTestId('course-schedule-panel')).toHaveCount(0);
    await expect(publicPage.getByTestId('unit-schedule-controls')).toHaveCount(0);
    expect(forbiddenRequests).toEqual([]);

    expect(watched.flat()).toEqual([]);
  } finally {
    await Promise.all([
      ...pages.map((p) => p.context().close()),
      adaApi.dispose(),
      boraApi.dispose(),
    ]);
  }
});
