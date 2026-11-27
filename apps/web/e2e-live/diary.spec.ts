/**
 * A diary on the real server: what is written on today and on the same day
 * a year before stays on that day's page over a load, the years stacked, the
 * latest on top; a day's mood, weather and place, the calendar's marks and
 * the days written in a row stay over a load too, each diary's own; and two
 * devices of the same reader write the same day together, in its room.
 *
 * The days are the server's real ones: the page's today is the device's, in
 * Seoul here, and the server takes no day more than two days past its own.
 */
import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test';

import { primaryKey } from '../e2e/keyboard.support.ts';
import { person, register, signedIn, signInOnPage, watchErrors, type Schemas } from './support.ts';

test.use({ locale: 'en-US', timezoneId: 'Asia/Seoul' });

/** Today in Seoul, as the page has it. */
function todayInSeoul(): string {
  return new Intl.DateTimeFormat('en-CA', {
    timeZone: 'Asia/Seoul',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(new Date());
}

const year = (page: Page, date: string) =>
  page.locator(`[data-testid="diary-year"][data-date="${date}"]`);
const editorOf = (page: Page, date: string) =>
  year(page, date).locator('[contenteditable="true"]').first();

/** The day before a day `YYYY-MM-DD`. */
function dayBefore(date: string): string {
  const day = new Date(`${date}T00:00:00Z`);
  day.setUTCDate(day.getUTCDate() - 1);
  return day.toISOString().slice(0, 10);
}

/** The reader's own space, and their first diary in it, made through the API. */
async function aDiary(api: APIRequestContext): Promise<{ ownId: string; diaryId: string }> {
  const workspaces = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const own = workspaces.find((workspace) => workspace.kind === 'personal');
  if (!own) throw new Error('no own space');
  const made = await api.post(`/api/v1/w/${own.id}/diaries`, { data: { ifNone: true } });
  expect(made.ok()).toBe(true);
  const diary = (await made.json()) as Schemas['Container'];
  return { ownId: own.id, diaryId: diary.id };
}

interface Node {
  text?: string;
  content?: Node[];
}

const textOf = (node: Node | null | undefined): string =>
  node ? (node.text ?? '') + (node.content ?? []).map(textOf).join('') : '';

/** The text of each entry on a day of the year, by its date, as the server holds them. */
async function stored(
  api: APIRequestContext,
  ownId: string,
  diaryId: string,
  monthDay: string,
): Promise<Record<string, string>> {
  const answer = await api.get(`/api/v1/w/${ownId}/diaries/${diaryId}/days/${monthDay}`);
  expect(answer.ok()).toBe(true);
  const entries = (await answer.json()) as Schemas['DiaryEntry'][];
  return Object.fromEntries(
    entries.map((entry) => [entry.date, textOf(entry.body as Node | null)]),
  );
}

async function pageOf(browser: Browser, address: string, who: Parameters<typeof signInOnPage>[1]) {
  const page = await (
    await browser.newContext({ locale: 'en-US', timezoneId: 'Asia/Seoul' })
  ).newPage();
  await page.goto(address);
  await signInOnPage(page, who);
  await expect(page.getByTestId('diary-page')).toBeVisible();
  return page;
}

test('today and the same day a year before stay on the page over a load, the latest on top', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('diarist');
  await register(base, who);
  const api = await signedIn(base, who);
  const { ownId, diaryId } = await aDiary(api);
  const today = todayInSeoul();
  const monthDay = today.slice(5);
  const lastYear = `${String(Number(today.slice(0, 4)) - 1)}-${monthDay}`;
  const errors = watchErrors(page);

  // A diary's address without a day opens today's.
  await page.goto(`/w/${ownId}/diaries/${diaryId}`);
  await signInOnPage(page, who);
  await expect(page).toHaveURL(new RegExp(`/diaries/${diaryId}/${monthDay}$`, 'u'));
  await editorOf(page, today).click();
  await page.keyboard.type('Written today.');
  await expect(year(page, today)).toHaveAttribute('data-written', '');
  await expect(year(page, today).getByTestId('entry-body')).toHaveAttribute('data-editor', 'ready');

  await page.getByTestId('diary-earlier').click();
  await year(page, lastYear).getByTestId('entry-blank').click();
  await page.keyboard.type('Written a year later.');
  await expect(year(page, lastYear)).toHaveAttribute('data-written', '');

  await expect
    .poll(() => stored(api, ownId, diaryId, monthDay), { timeout: 15_000 })
    .toEqual({ [today]: 'Written today.', [lastYear]: 'Written a year later.' });

  await page.reload();
  await expect(page.getByTestId('diary-page')).toBeVisible();
  const dates = await page
    .getByTestId('diary-year')
    .evaluateAll((cells) => cells.map((cell) => cell.getAttribute('data-date')));
  expect(dates).toEqual([today, lastYear]);
  await expect(year(page, today)).toHaveAttribute('data-open', '');
  await expect(editorOf(page, today)).toHaveText('Written today.');
  await expect(year(page, lastYear).getByTestId('entry-preview')).toContainText(
    'Written a year later.',
  );
  expect(errors).toEqual([]);
});

test('two devices write the same day together', async ({ browser, baseURL }) => {
  const base = baseURL ?? '';
  const who = person('diarist');
  await register(base, who);
  const api = await signedIn(base, who);
  const { ownId, diaryId } = await aDiary(api);
  const today = todayInSeoul();
  const address = `/w/${ownId}/diaries/${diaryId}/${today.slice(5)}`;
  const [first, second] = [
    await pageOf(browser, address, who),
    await pageOf(browser, address, who),
  ];

  await editorOf(first, today).click();
  await first.keyboard.type('From the phone.');
  await expect(year(first, today).getByTestId('entry-body')).toHaveAttribute(
    'data-editor',
    'ready',
  );
  // The other device hears the entry made, joins its room, and has what was written.
  await expect(editorOf(second, today)).toHaveText('From the phone.');
  await expect(year(second, today).getByTestId('entry-body')).toHaveAttribute(
    'data-editor',
    'ready',
  );
  await editorOf(second, today).click();
  await second.keyboard.press('End');
  await second.keyboard.type(' From the laptop.');
  await expect(editorOf(first, today)).toHaveText('From the phone. From the laptop.');
  await expect
    .poll(async () => (await stored(api, ownId, diaryId, today.slice(5)))[today], {
      timeout: 15_000,
    })
    .toBe('From the phone. From the laptop.');
});

test('an entry emptied and left as the page goes is a day not written', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('diarist');
  await register(base, who);
  const api = await signedIn(base, who);
  const { ownId, diaryId } = await aDiary(api);
  const today = todayInSeoul();
  const monthDay = today.slice(5);
  await page.goto(`/w/${ownId}/diaries/${diaryId}/${monthDay}`);
  await signInOnPage(page, who);
  await editorOf(page, today).click();
  await page.keyboard.type('x');
  await expect(year(page, today).getByTestId('entry-body')).toHaveAttribute('data-editor', 'ready');
  await expect
    .poll(async () => Object.keys(await stored(api, ownId, diaryId, monthDay)))
    .toEqual([today]);
  await page.keyboard.press('Backspace');
  await expect(editorOf(page, today)).toHaveText('');
  // Loaded again: the page that went sent the deletion as it went.
  await page.reload();
  await expect(page.getByTestId('diary-page')).toBeVisible();
  await expect.poll(() => stored(api, ownId, diaryId, monthDay), { timeout: 15_000 }).toEqual({});
  await expect(year(page, today)).not.toHaveAttribute('data-written');
});

test('a day’s mood, weather and place, the calendar and the days in a row stay over a load, each diary’s own', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('diarist');
  await register(base, who);
  const api = await signedIn(base, who);
  const { ownId, diaryId } = await aDiary(api);
  const other = await api.post(`/api/v1/w/${ownId}/diaries`, { data: { title: 'Workouts' } });
  expect(other.status()).toBe(201);
  const otherId = ((await other.json()) as Schemas['Container']).id;
  const today = todayInSeoul();
  const yesterday = dayBefore(today);
  const made = await api.post(`/api/v1/w/${ownId}/diaries/${diaryId}/entries`, {
    data: { date: yesterday, mood: 'okay' },
  });
  expect(made.status()).toBe(201);
  const errors = watchErrors(page);

  await page.goto(`/w/${ownId}/diaries/${diaryId}/${today.slice(5)}`);
  await signInOnPage(page, who);
  await expect(page.getByTestId('diary-streak-badge')).toHaveText('Days in a row: 1');
  const cell = year(page, today);
  await cell.getByRole('button', { name: 'Great', exact: true }).click();
  await cell.getByRole('textbox', { name: 'Weather' }).fill('Clear');
  await cell.getByRole('textbox', { name: 'Place' }).fill('Seoul');
  await cell.getByRole('textbox', { name: 'Place' }).blur();
  await expect(page.getByTestId('diary-streak-badge')).toHaveText('Days in a row: 2');
  await expect
    .poll(
      async () => {
        const answer = await api.get(
          `/api/v1/w/${ownId}/diaries/${diaryId}/days/${today.slice(5)}`,
        );
        const [entry] = (await answer.json()) as Schemas['DiaryEntry'][];
        return entry && { mood: entry.mood, weather: entry.weather, place: entry.place };
      },
      { timeout: 15_000 },
    )
    .toEqual({ mood: 'great', weather: 'Clear', place: 'Seoul' });

  await page.reload();
  await expect(page.getByTestId('diary-page')).toBeVisible();
  await expect(cell.getByRole('button', { name: 'Great' })).toHaveAttribute('aria-pressed', 'true');
  await expect(cell.getByRole('textbox', { name: 'Weather' })).toHaveValue('Clear');
  await expect(cell.getByRole('textbox', { name: 'Place' })).toHaveValue('Seoul');
  await expect(page.getByTestId('diary-streak-badge')).toHaveText('Days in a row: 2');
  await page.getByTestId('diary-pick').click();
  const calendar = page.getByTestId('diary-calendar');
  await expect(calendar.getByTestId('streak-longest')).toHaveText('Longest: 2');
  await expect(calendar.locator(`[data-bits-day][data-value="${today}"]`)).toHaveAttribute(
    'data-mood',
    'great',
  );
  // Yesterday may be in the month before: the calendar opens on today's.
  if (yesterday.slice(0, 7) === today.slice(0, 7)) {
    await expect(calendar.locator(`[data-bits-day][data-value="${yesterday}"]`)).toHaveAttribute(
      'data-mood',
      'okay',
    );
  }

  // The other diary has its own days: none.
  await page.goto(`/w/${ownId}/diaries/${otherId}/${today.slice(5)}`);
  await expect(page.getByTestId('diary-title')).toHaveText('Workouts');
  await expect(year(page, today)).not.toHaveAttribute('data-written');
  await page.getByTestId('diary-pick').click();
  await expect(page.getByTestId('diary-calendar').getByTestId('streak-current')).toHaveText(
    'Days in a row: 0',
  );
  await expect(page.getByTestId('diary-calendar').locator('[data-written]')).toHaveCount(0);
  await expect(page.getByTestId('diary-streak-badge')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('keeps a diary entry body version with ⌘S/Ctrl+S and restores it on the real server', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('diarist');
  await register(base, who);
  const api = await signedIn(base, who);
  const { ownId, diaryId } = await aDiary(api);
  const today = todayInSeoul();
  const monthDay = today.slice(5);
  const errors = watchErrors(page);

  await page.goto(`/w/${ownId}/diaries/${diaryId}/${monthDay}`);
  await signInOnPage(page, who);
  const cell = year(page, today);
  await editorOf(page, today).click();
  await page.keyboard.type('Kept paragraph.');
  await expect(cell.getByTestId('entry-body')).toHaveAttribute('data-editor', 'ready');

  const save = await primaryKey(page, 's');
  await page.keyboard.press(save);
  await expect(cell.getByTestId('body-kept')).toContainText('Version kept.');

  await page.keyboard.press('Enter');
  await page.keyboard.type('Edited after save.');
  await expect(editorOf(page, today)).toContainText('Edited after save.');

  await cell.getByTestId('entry-versions').click();
  const dialog = page.getByTestId('body-versions');
  await expect(dialog).toBeVisible();
  const list = dialog.getByTestId('body-versions-list').locator('button[data-version]');
  await expect(list).toHaveCount(1);
  await expect(list.first().getByTestId('body-version-cause')).toHaveText('Kept by hand');

  await list.first().click();
  await dialog.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(editorOf(page, today)).toHaveText('Kept paragraph.');
  await expect
    .poll(() => stored(api, ownId, diaryId, monthDay), { timeout: 15_000 })
    .toEqual({ [today]: 'Kept paragraph.' });
  expect(errors).toEqual([]);
});
