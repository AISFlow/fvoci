/**
 * What a diary's day carries besides its body, and the diary's calendar, end
 * to end on the in-memory backend: the mood is one of five named faces at the
 * head of the open day, and makes the entry of a day nobody wrote on; the
 * weather and place are typed by hand; a folded day shows all three; the
 * calendar marks the written days by their mood, counts the days written in
 * a row (up to yesterday while today is not written yet) and the most ever,
 * and takes the reader to a day pressed; and it all goes by the keyboard and
 * fits a phone, light and dark.
 *
 * Over a load and on the server: `e2e-live/diary.spec.ts`.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { diaryPage, editorOf, entriesOn, openDiary, year } from './diary.support.ts';
import { screensDir } from './screens.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

const calendar = (page: Page): Locator => page.getByTestId('diary-calendar');
const dayIn = (page: Page, date: string): Locator =>
  calendar(page).locator(`[data-bits-day][data-value="${date}"]`);

/** An entry of the diary shown made on `date` by another device of the reader. */
async function writtenElsewhere(page: Page, date: string): Promise<void> {
  await page.evaluate(async (day) => {
    const diaryId = /diaries\/([^/]+)\//u.exec(location.pathname)?.[1] ?? '';
    const dev = (
      window as unknown as {
        fvociDev: {
          backend: { diary: { createEntry(id: string, date: string): Promise<unknown> } };
        };
      }
    ).fvociDev;
    await dev.backend.diary.createEntry(diaryId, day);
  }, date);
}

/** The notes of an entry of the diary shown, as the backend has them. */
async function notesOn(page: Page, monthDay: string): Promise<Record<string, unknown>[]> {
  return page.evaluate(async (day) => {
    const diaryId = /diaries\/([^/]+)\//u.exec(location.pathname)?.[1] ?? '';
    const dev = (
      window as unknown as {
        fvociDev: {
          backend: {
            diary: { entriesOn(id: string, md: string): Promise<Record<string, unknown>[]> };
          };
        };
      }
    ).fvociDev;
    const entries = await dev.backend.diary.entriesOn(diaryId, day);
    return entries.map(({ date, mood, weather, place }) => ({ date, mood, weather, place }));
  }, monthDay);
}

test('the mood, weather and place of a day: marked at its head, made by the mood, shown folded', async ({
  page,
}) => {
  await openDiary(page);
  const today = year(page, '2026-11-21');
  const notes = today.getByTestId('entry-notes');
  await expect(notes).toHaveAccessibleName(
    'The mood, weather and place of Saturday, November 21, 2026',
  );
  const names = await notes
    .getByRole('button')
    .evaluateAll((buttons) => buttons.map((button) => button.getAttribute('aria-label')));
  expect(names).toEqual(['Great', 'Good', 'Okay', 'Bad', 'Awful']);
  expect(await entriesOn(page, '11-21')).toBe(0);

  // A mood alone makes the day's entry.
  await notes.getByRole('button', { name: 'Good', exact: true }).click();
  await expect(notes.getByRole('button', { name: 'Good' })).toHaveAttribute('aria-pressed', 'true');
  await expect(notes.getByTestId('mood-name')).toHaveText('Good');
  await expect(today).toHaveAttribute('data-written', '');
  await expect
    .poll(() => notesOn(page, '11-21'))
    .toEqual([{ date: '2026-11-21', mood: 'good', weather: '', place: '' }]);
  await notes.getByRole('textbox', { name: 'Weather' }).fill('Clear, then rain');
  await notes.getByRole('textbox', { name: 'Place' }).fill('Home');
  await editorOf(today).click();
  await page.keyboard.type('A good day.');
  await expect
    .poll(() => notesOn(page, '11-21'))
    .toEqual([{ date: '2026-11-21', mood: 'good', weather: 'Clear, then rain', place: 'Home' }]);

  // Folded under another year opened, it shows its notes, to read.
  await page.getByTestId('diary-earlier').click();
  await year(page, '2025-11-21').getByTestId('entry-blank').click();
  const shown = today.getByTestId('entry-notes-shown');
  await expect(shown).toContainText('Good');
  await expect(shown).toContainText('Clear, then rain');
  await expect(shown).toContainText('Home');
  await expect(today.getByTestId('entry-notes')).toHaveCount(0);
  // Pressed again, the mood is taken away.
  await today.getByTestId('entry-open').click();
  await today.getByRole('button', { name: 'Good', exact: true }).click();
  await expect(today.getByRole('button', { name: 'Good' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  await expect.poll(async () => (await notesOn(page, '11-21'))[0]?.mood).toBeNull();
});

test('the calendar marks the written days by mood, counts the days in a row, and goes to a day', async ({
  page,
}) => {
  await openDiary(page);
  // Yesterday and the day before are written; today is not, yet.
  await page.getByTestId('diary-previous').click();
  await expect(page).toHaveURL(/\/11-20$/u);
  await page.getByTestId('diary-previous').click();
  await expect(page).toHaveURL(/\/11-19$/u);
  await editorOf(year(page, '2026-11-19')).click();
  await page.keyboard.type('Thursday.');
  await expect(year(page, '2026-11-19')).toHaveAttribute('data-written', '');
  await page.getByTestId('diary-next').click();
  await year(page, '2026-11-20').getByRole('button', { name: 'Bad', exact: true }).click();
  await expect(year(page, '2026-11-20')).toHaveAttribute('data-written', '');
  await page.getByTestId('diary-today').click();
  await expect(page).toHaveURL(/\/11-21$/u);
  await expect(page.getByTestId('diary-streak-badge')).toHaveText('Days in a row: 2');

  await page.getByTestId('diary-pick').click();
  await expect(calendar(page).getByTestId('streak-current')).toHaveText('Days in a row: 2');
  await expect(calendar(page).getByTestId('streak-longest')).toHaveText('Longest: 2');
  await expect(dayIn(page, '2026-11-19')).toHaveAttribute('data-written', '');
  await expect(dayIn(page, '2026-11-19')).not.toHaveAttribute('data-mood');
  await expect(dayIn(page, '2026-11-19')).toHaveAccessibleName('Thursday, November 19, 2026');
  await expect(dayIn(page, '2026-11-19')).toHaveAccessibleDescription('Written');
  await expect(dayIn(page, '2026-11-20')).toHaveAttribute('data-mood', 'bad');
  await expect(dayIn(page, '2026-11-20')).toHaveAccessibleDescription('Written, Bad');
  await expect(dayIn(page, '2026-11-21')).not.toHaveAttribute('data-written');

  // Today written: the count goes on to it.
  await page.keyboard.press('Escape');
  await year(page, '2026-11-21').getByRole('button', { name: 'Great', exact: true }).click();
  await expect(page.getByTestId('diary-streak-badge')).toHaveText('Days in a row: 3');
  await page.getByTestId('diary-pick').click();
  await expect(dayIn(page, '2026-11-21')).toHaveAttribute('data-mood', 'great');
  await expect(calendar(page).getByTestId('streak-longest')).toHaveText('Longest: 3');

  // A day pressed is its page, its year open.
  await dayIn(page, '2026-11-19').click();
  await expect(page).toHaveURL(/\/11-19$/u);
  await expect(year(page, '2026-11-19')).toHaveAttribute('data-open', '');
  await expect(calendar(page)).toHaveCount(0);

  // A month before is read as it is gone to: its written day is marked.
  await writtenElsewhere(page, '2026-10-05');
  await page.getByTestId('diary-pick').click();
  await expect(dayIn(page, '2026-10-05')).toHaveCount(0);
  await calendar(page)
    .getByRole('button', { name: /previous/iu })
    .click();
  await expect(dayIn(page, '2026-10-05')).toHaveAttribute('data-written', '');
  await expect(calendar(page).locator('[data-written]')).toHaveCount(1);
});

test('a day still to come this year opens the calendar on last year’s, the day in hand', async ({
  page,
}) => {
  await openDiary(page);
  await page.getByTestId('diary-next').click();
  await expect(page).toHaveURL(/\/11-22$/u);
  await page.getByTestId('diary-pick').click();
  await expect(dayIn(page, '2025-11-22')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(year(page, '2025-11-22')).toHaveAttribute('data-open', '');
});

test('the notes and the calendar by the keyboard alone', async ({ page }) => {
  await openDiary(page);
  const today = year(page, '2026-11-21');
  await today.getByRole('button', { name: 'Great', exact: true }).focus();
  await page.keyboard.press('Tab');
  await expect(today.getByRole('button', { name: 'Good', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(today.getByRole('button', { name: 'Good' })).toHaveAttribute('aria-pressed', 'true');
  // The three faces after it, and the weather.
  for (let step = 0; step < 4; step += 1) await page.keyboard.press('Tab');
  await expect(today.getByRole('textbox', { name: 'Weather' })).toBeFocused();
  await page.keyboard.type('Snow');
  await page.keyboard.press('Tab');
  await expect(today.getByRole('textbox', { name: 'Place' })).toBeFocused();
  await expect.poll(async () => (await notesOn(page, '11-21'))[0]?.weather).toBe('Snow');

  await page.getByTestId('diary-pick').focus();
  await page.keyboard.press('Enter');
  await expect(calendar(page)).toBeVisible();
  await expect(dayIn(page, '2026-11-21')).toBeFocused();
  await page.keyboard.press('ArrowLeft');
  await expect(dayIn(page, '2026-11-20')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/11-20$/u);
  await expect(year(page, '2026-11-20')).toHaveAttribute('data-open', '');
});

test('the notes and the calendar fit a phone, light and dark', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openDiary(page);
  await page.keyboard.press('Escape');
  const today = year(page, '2026-11-21');
  await today.getByRole('button', { name: 'Okay', exact: true }).click();
  await today.getByRole('textbox', { name: 'Weather' }).fill('Cloudy');
  await today.getByRole('textbox', { name: 'Place' }).fill('A café by the river');
  await editorOf(today).click();
  await page.keyboard.type('Small screen.');
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      390,
    );
    for (const name of ['Great', 'Awful']) {
      await expect(today.getByRole('button', { name, exact: true })).toBeInViewport();
    }
    await expect(today.getByRole('textbox', { name: 'Place' })).toBeInViewport();
    await page.screenshot({ path: `${screensDir('diary')}/notes-phone-${scheme}.png` });
    await page.getByTestId('diary-pick').click();
    await expect(calendar(page)).toBeInViewport();
    const box = await calendar(page).boundingBox();
    expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(390);
    await page.screenshot({ path: `${screensDir('diary')}/calendar-phone-${scheme}.png` });
    await page.keyboard.press('Escape');
  }
  await expect(diaryPage(page)).toBeVisible();
});

test('the pictures of a wide page, light and dark: years stacked, one open, the others folded, and the calendar', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'once, at the size it sets');
  await page.setViewportSize({ width: 1280, height: 900 });
  await openDiary(page);
  const today = year(page, '2026-11-21');
  await today.getByRole('button', { name: 'Good', exact: true }).click();
  await today.getByRole('textbox', { name: 'Weather' }).fill('Clear, cold wind');
  await today.getByRole('textbox', { name: 'Place' }).fill('Home');
  await editorOf(today).click();
  await page.keyboard.type('Walked along the river after lunch.');
  await expect(today).toHaveAttribute('data-written', '');
  for (const [date, mood, text] of [
    ['2025-11-21', 'Okay', 'First snow, gone by noon.'],
    ['2024-11-21', 'Great', 'Moved into the new place.'],
  ] as const) {
    await page.getByTestId('diary-earlier').click();
    const cell = year(page, date);
    await cell.getByTestId('entry-blank').click();
    await expect(editorOf(cell)).toBeFocused();
    await page.keyboard.type(text);
    await expect(cell).toHaveAttribute('data-written', '');
    await cell.getByRole('button', { name: mood, exact: true }).click();
  }
  await today.getByTestId('entry-open').click();
  await expect(today).toHaveAttribute('data-open', '');
  await expect(year(page, '2024-11-21').getByTestId('entry-notes-shown')).toContainText('Great');
  await expect(year(page, '2024-11-21').getByTestId('entry-preview')).toContainText(
    'Moved into the new place.',
  );
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({
      path: `${screensDir('diary')}/wide-${scheme}.png`,
      animations: 'disabled',
    });
    await page.getByTestId('diary-pick').click();
    await expect(dayIn(page, '2026-11-21')).toHaveAttribute('data-mood', 'good');
    await page.screenshot({
      path: `${screensDir('diary')}/calendar-wide-${scheme}.png`,
      animations: 'disabled',
    });
    await page.keyboard.press('Escape');
  }
});
