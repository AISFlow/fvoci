/**
 * A diary, end to end, on the in-memory backend: the reader's own space has
 * one way in (made as it is pressed), the shared workspace none; its page
 * shows one day of the year in every year, this year's open and the years
 * before it folded; an entry is made by what is written first, not before;
 * a year nobody wrote on is written on afterwards; the days step through
 * `02-29`; midnight moves today and leaves what is being written where it
 * is; `[[` puts a reference to a page of the own space; and all of it goes
 * by the keyboard alone and fits a phone.
 *
 * The world in memory starts anew on a load: going about inside the page is
 * done as the app's links do (`goInside`). The same diary over a load and
 * from two devices is tested against the server (`e2e-live/diary.spec.ts`).
 */
import { expect, test, type Page } from '@playwright/test';

import {
  ON_A_DAY,
  dayOf,
  diaryPage,
  editorOf,
  editors,
  entriesOn,
  goToDay,
  openDiary,
  switchTo,
  year,
} from './diary.support.ts';
import { primaryKey } from './keyboard.support.ts';
import { screensDir } from './screens.ts';
import { openApp, showSidebar, sidebar } from './shell.support.ts';
import { ON_A_WIKI, goInside } from './wiki.support.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

test('the shared workspace has no diary; the own space has one way in, made as it is pressed', async ({
  page,
}) => {
  await openApp(page);
  await showSidebar(page);
  await expect(sidebar(page).getByTestId('open-diary')).toHaveCount(0);
  await sidebar(page).getByTestId('add-board').click();
  await expect(page.getByTestId('template-picker')).toBeVisible();
  await expect(page.getByTestId('template-picker').getByTestId('new-diary')).toHaveCount(0);
  await page.keyboard.press('Escape');

  await switchTo(page, 'My space');
  await showSidebar(page);
  const way = sidebar(page).getByTestId('open-diary');
  await expect(way).toHaveText('Diary');
  await way.click();
  await expect(page).toHaveURL(ON_A_DAY);
  await showSidebar(page);
  // Now it is in the list beside the boards and wikis, with its own mark, and the way in is gone.
  const link = sidebar(page).getByRole('link', { name: 'Diary, diary', exact: true });
  await expect(link).toBeVisible();
  await expect(link.locator('[data-board-icon]')).toHaveAttribute('data-kind', 'diary');
  await expect(way).toHaveCount(0);
  await expect(link).toHaveAttribute('aria-current', 'page');
});

test('today’s page: the day at the head, this year open, an entry made by what is written first', async ({
  page,
}) => {
  await openDiary(page);
  expect(dayOf(page).monthDay).toBe('11-21');
  await expect(page.getByTestId('diary-day')).toHaveText('November 21');
  await expect(page.getByTestId('diary-today')).toHaveAttribute('aria-current', 'date');
  const today = year(page, '2026-11-21');
  await expect(today).toHaveAttribute('data-open', '');
  await expect(today).not.toHaveAttribute('data-written');
  // Nothing is made for a day before the reader writes.
  expect(await entriesOn(page, '11-21')).toBe(0);

  await editorOf(today).click();
  await page.keyboard.type('Rain all day');
  await expect(today).toHaveAttribute('data-written', '');
  await expect(today.getByTestId('entry-body')).toHaveAttribute('data-editor', 'ready');
  await page.keyboard.type(', then sun.');
  await expect(editorOf(today)).toHaveText('Rain all day, then sun.');
  await expect(editors(page)).toHaveCount(1);
  expect(await entriesOn(page, '11-21')).toBe(1);

  // Away and back: what was written is there.
  await page.getByTestId('diary-next').click();
  await expect(page).toHaveURL(/\/11-22$/u);
  await expect(page.getByTestId('diary-today')).not.toHaveAttribute('aria-current');
  await expect(year(page, '2026-11-22').getByTestId('entry-to-come')).toHaveText('Not here yet');
  await page.goBack();
  await expect(page).toHaveURL(/\/11-21$/u);
  await expect(year(page, '2026-11-21')).toContainText('Rain all day, then sun.');
});

test('a year before: written afterwards, one open at a time, the others folded', async ({
  page,
}) => {
  await openDiary(page);
  const today = year(page, '2026-11-21');
  await editorOf(today).click();
  await page.keyboard.type('This year.');
  await expect(today).toHaveAttribute('data-written', '');
  // The room writes the body a while after the last key, while it is open.
  await page.clock.runFor(3000);

  await page.getByTestId('diary-earlier').click();
  const last = year(page, '2025-11-21');
  await expect(last.getByTestId('entry-blank')).toHaveText('Not written');
  await last.getByTestId('entry-blank').click();
  await expect(last).toHaveAttribute('data-open', '');
  await expect(editors(page)).toHaveCount(1);
  // The caret is in the day opened.
  await page.keyboard.type('A year ago.');
  await expect(last).toHaveAttribute('data-written', '');
  // This year's is folded: its first lines, read only.
  await expect(today.getByTestId('entry-preview')).toContainText('This year.');

  await today.getByTestId('entry-open').click();
  await expect(today).toHaveAttribute('data-open', '');
  await expect(last.getByTestId('entry-preview')).toContainText('A year ago.');
  await expect(editors(page)).toHaveCount(1);
  // Years after the diary was made, and before, in that order.
  await expect(diaryPage(page).getByTestId('diary-year')).toHaveCount(2);
  const dates = await diaryPage(page)
    .getByTestId('diary-year')
    .evaluateAll((cells) => cells.map((cell) => cell.getAttribute('data-date')));
  expect(dates).toEqual(['2026-11-21', '2025-11-21']);
});

test('keys typed at once on a year just pressed all go into it, however late a frame comes', async ({
  page,
}) => {
  await openDiary(page);
  await page.getByTestId('diary-earlier').click();
  const last = year(page, '2025-11-21');
  // A machine short of time draws its frames late: the caret does not wait for one.
  await page.evaluate(() => {
    window.requestAnimationFrame = (callback) =>
      window.setTimeout(() => {
        callback(performance.now());
      }, 1000);
    window.cancelAnimationFrame = (handle) => {
      window.clearTimeout(handle);
    };
  });
  await last.getByTestId('entry-blank').click();
  await page.keyboard.type('Typed at once.');
  await expect(last).toHaveAttribute('data-written', '');
  await expect(editorOf(last)).toHaveText('Typed at once.');
});

test('an emptied entry is a day not written again', async ({ page }) => {
  await openDiary(page);
  const today = year(page, '2026-11-21');
  await editorOf(today).click();
  await page.keyboard.type('Oops');
  await expect(today.getByTestId('entry-body')).toHaveAttribute('data-editor', 'ready');
  for (let left = 4; left > 0; left -= 1) await page.keyboard.press('Backspace');
  await expect(editorOf(today)).toHaveText('');
  await page.getByTestId('diary-next').click();
  await page.getByTestId('diary-previous').click();
  await expect(page).toHaveURL(/\/11-21$/u);
  await expect(year(page, '2026-11-21')).not.toHaveAttribute('data-written');
});

test('the days step through 02-29, which holds the leap years alone', async ({ page }) => {
  await openDiary(page);
  await goToDay(page, '02-28');
  await expect(page.getByTestId('diary-day')).toHaveText('February 28');
  await page.getByTestId('diary-next').click();
  await expect(page).toHaveURL(/\/02-29$/u);
  await expect(page.getByTestId('diary-day')).toHaveText('February 29');
  await expect(diaryPage(page)).toContainText('February 29 comes in leap years only.');
  // 2026 has none: the way to the last leap year is there all the same.
  await expect(diaryPage(page).getByTestId('diary-year')).toHaveCount(0);
  await expect(page.getByTestId('diary-earlier')).toHaveText('Show 2024');
  await page.getByTestId('diary-earlier').click();
  await expect(year(page, '2024-02-29').getByTestId('entry-blank')).toBeVisible();
  await page.getByTestId('diary-next').click();
  await expect(page).toHaveURL(/\/03-01$/u);
  await page.getByTestId('diary-previous').click();
  await expect(page).toHaveURL(/\/02-29$/u);
  // A day no year has is not found.
  await goInside(page, new URL(page.url()).pathname.replace(/02-29$/u, '02-30'));
  await expect(page.getByTestId('not-found')).toBeVisible();
});

test('search result ?year=YYYY expands that year; invalid or absent years keep the default', async ({
  page,
}) => {
  await openDiary(page);
  await page.getByTestId('diary-earlier').click();
  const last = year(page, '2025-11-21');
  await last.getByTestId('entry-blank').click();
  await page.keyboard.type('Autumn lantern festival.');
  await expect(last).toHaveAttribute('data-written', '');
  await page.clock.runFor(3000);

  // Move to another day so search navigates back to 11-21 with ?year=2025.
  await page.getByTestId('diary-next').click();
  await expect(page).toHaveURL(/\/11-22$/u);

  await page.getByTestId('open-search').click();
  await page.getByTestId('search-field').fill('lantern');
  // Search debounces input; advance the fake clock installed by openDiary.
  await page.clock.runFor(500);
  const hit = page.locator('[data-search-hit]').first();
  await expect(hit).toBeVisible();
  await expect(hit).toHaveAttribute('href', /\/11-21\?year=2025$/u);
  await hit.click();

  await expect(page).toHaveURL(/\/11-21\?year=2025$/u);
  await expect(year(page, '2025-11-21')).toHaveAttribute('data-open', '');
  await expect(year(page, '2026-11-21')).not.toHaveAttribute('data-open');

  // Unknown/non-existent or malformed ?year= falls back to this year's default.
  const basePath = new URL(page.url()).pathname;
  await goInside(page, `${basePath}?year=1999`);
  await expect(year(page, '2026-11-21')).toHaveAttribute('data-open', '');
  await goInside(page, `${basePath}?year=bad`);
  await expect(year(page, '2026-11-21')).toHaveAttribute('data-open', '');
  // Plain address without ?year= stays backward compatible.
  await goInside(page, basePath);
  await expect(year(page, '2026-11-21')).toHaveAttribute('data-open', '');
});

test('midnight moves today; what is being written stays on its day', async ({ page }) => {
  await openDiary(page, new Date('2026-11-21T23:58:00+09:00'));
  const late = year(page, '2026-11-21');
  await editorOf(late).click();
  await page.keyboard.type('Late');
  await expect(late).toHaveAttribute('data-written', '');
  await page.clock.fastForward('05:00');
  await expect(page.getByTestId('diary-today')).not.toHaveAttribute('aria-current');
  await expect(page).toHaveURL(/\/11-21$/u);
  await expect(late).toHaveAttribute('data-open', '');
  await page.keyboard.type(' night.');
  await expect(editorOf(late)).toHaveText('Late night.');

  await page.getByTestId('diary-today').click();
  await expect(page).toHaveURL(/\/11-22$/u);
  await expect(page.getByTestId('diary-today')).toHaveAttribute('aria-current', 'date');
  await expect(year(page, '2026-11-22')).toHaveAttribute('data-open', '');
});

test('by the keyboard alone: from day to day, from year to year, a year opened', async ({
  page,
}) => {
  await openDiary(page);
  await page.getByTestId('diary-earlier').click();
  await page.getByTestId('diary-previous').focus();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/11-20$/u);
  await page.keyboard.press('Tab');
  await expect(page.getByTestId('diary-today')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/11-21$/u);
  // From the head down to the years.
  const blank = year(page, '2025-11-21').getByTestId('entry-blank');
  await page.getByTestId('diary-pick').focus();
  const focused = (): Promise<boolean> => blank.evaluate((it) => it === document.activeElement);
  // Through the open day: its mood, weather, place and body.
  for (let step = 0; step < 12 && !(await focused()); step += 1) await page.keyboard.press('Tab');
  await expect(blank).toBeFocused();
  await expect(blank).toHaveAccessibleName('Write the entry of Friday, November 21, 2025');
  await page.keyboard.press('Enter');
  await page.keyboard.type('Typed by keys.');
  await expect(year(page, '2025-11-21')).toHaveAttribute('data-written', '');
});

test('the menu renames and deletes the diary, and the way in comes back', async ({ page }) => {
  await openDiary(page);
  await page.getByTestId('diary-menu').click();
  await page.locator('[data-action="rename"]').click();
  const naming = page.getByTestId('diary-name-dialog');
  await naming.getByRole('textbox').fill('Days');
  await naming.getByRole('textbox').press('Enter');
  await expect(page.getByTestId('diary-title')).toHaveText('Days');
  await showSidebar(page);
  await expect(sidebar(page).getByRole('link', { name: 'Days, diary', exact: true })).toBeVisible();

  await page.keyboard.press('Escape');
  await page.getByTestId('diary-menu').click();
  await page.locator('[data-action="delete"]').click();
  await page
    .getByTestId('confirm-dialog')
    .getByRole('button', { name: 'Delete', exact: true })
    .click();
  await expect(diaryPage(page)).toHaveCount(0);
  await showSidebar(page);
  await expect(sidebar(page).getByTestId('open-diary')).toBeVisible();
});

test('fits a phone, light and dark', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openDiary(page);
  await page.keyboard.press('Escape');
  await page.getByTestId('diary-earlier').click();
  await editorOf(year(page, '2026-11-21')).click();
  await page.keyboard.type('Small screen.');
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    const wide = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(wide).toBeLessThanOrEqual(390);
    for (const id of ['diary-previous', 'diary-today', 'diary-next', 'diary-pick', 'diary-menu']) {
      await expect(page.getByTestId(id)).toBeInViewport();
    }
    await page.screenshot({ path: `${screensDir('diary')}/phone-${scheme}.png` });
  }
});

test('keeps a manual body version with ⌘S/Ctrl+S and restores from the versions dialog', async ({
  page,
}) => {
  await openDiary(page);
  const today = year(page, '2026-11-21');
  await editorOf(today).click();
  await page.keyboard.type('First entry draft.');
  await expect(today.getByTestId('entry-body')).toHaveAttribute('data-editor', 'ready');

  const save = await primaryKey(page, 's');
  await page.keyboard.press(save);
  await expect(today.getByTestId('body-kept')).toContainText('Version kept.');

  await page.keyboard.press('Enter');
  await page.keyboard.type('Second line added.');
  await expect(editorOf(today)).toContainText('Second line added.');

  await today.getByTestId('entry-versions').click();
  const dialog = page.getByTestId('body-versions');
  await expect(dialog).toBeVisible();
  const list = dialog.getByTestId('body-versions-list').locator('button[data-version]');
  await expect(list).toHaveCount(1);
  await expect(list.first().getByTestId('body-version-cause')).toHaveText('Kept by hand');

  // Restore version 1 (before "Second line added.").
  await list.first().click();
  await dialog.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(editorOf(today)).toHaveText('First entry draft.');
});

test('`[[` in an entry puts a reference to a page of the own space, which leads there', async ({
  page,
}) => {
  await openDiary(page);
  // A wiki of the own space, with a page.
  await showSidebar(page);
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await picker.getByTestId('new-title').fill('Trips');
  await picker.getByTestId('new-title').press('Enter');
  await picker.getByTestId('new-wiki').click();
  await expect(page).toHaveURL(ON_A_WIKI);
  const wikiId = ON_A_WIKI.exec(page.url())?.[1] ?? '';
  const pageId = await page.evaluate(async (id) => {
    const dev = (
      window as unknown as {
        fvociDev: {
          backend: {
            wiki: { createPage(id: string, draft: object): Promise<{ id: string }> };
          };
        };
      }
    ).fvociDev;
    return (await dev.backend.wiki.createPage(id, { title: 'Packing list' })).id;
  }, wikiId);

  // Back to the day, as the browser's back button goes.
  await page.goBack();
  await expect(page).toHaveURL(ON_A_DAY);
  const today = year(page, '2026-11-21');
  await editorOf(today).click();
  await page.keyboard.type('Packed by [[Packing');
  const option = page
    .locator('[data-suggestions="reference"]')
    .getByRole('option', { name: 'Packing list · Page' });
  await expect(option).toHaveCount(1);
  await page.keyboard.press('Enter');
  const chip = today.locator(`[data-reference][data-block-id="${pageId}"]`);
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await expect(chip).toHaveText('Packing list');
  await chip.getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/wikis/${wikiId}/${pageId}$`, 'u'));
});

/** The mood of the entry of `monthDay` of the diary shown, as the backend has it. */
async function moodOn(page: Page, monthDay: string): Promise<unknown> {
  return page.evaluate(
    async ([diaryId, day]) => {
      const { diary } = (
        window as unknown as {
          fvociDev: {
            backend: {
              diary: { entriesOn(id: string, day: string): Promise<{ mood: unknown }[]> };
            };
          };
        }
      ).fvociDev.backend;
      return (await diary.entriesOn(diaryId, day))[0]?.mood;
    },
    [dayOf(page).diaryId, monthDay] as const,
  );
}

/** Deletes the entry of `monthDay` of the diary shown, as another device of the reader would. */
async function deletedElsewhere(page: Page, monthDay: string): Promise<void> {
  await page.evaluate(
    async ([diaryId, day]) => {
      const { diary } = (
        window as unknown as {
          fvociDev: {
            backend: {
              diary: {
                entriesOn(id: string, day: string): Promise<{ id: string }[]>;
                deleteEntry(id: string, entryId: string): Promise<void>;
              };
            };
          };
        }
      ).fvociDev.backend;
      const [entry] = await diary.entriesOn(diaryId, day);
      if (entry) await diary.deleteEntry(diaryId, entry.id);
    },
    [dayOf(page).diaryId, monthDay] as const,
  );
}

test('the menu brings a deleted entry back to its day, unless the day has one since', async ({
  page,
}) => {
  await openDiary(page);
  const today = year(page, '2026-11-21');
  // A day with a mood is no empty one, which a deletion would not keep.
  const notes = today.getByTestId('entry-notes');
  await notes.getByRole('button', { name: 'Good', exact: true }).click();
  await expect(today).toHaveAttribute('data-written', '');
  await expect.poll(() => moodOn(page, '11-21')).toBe('good');
  await deletedElsewhere(page, '11-21');
  await expect(today).not.toHaveAttribute('data-written');

  await page.getByTestId('diary-menu').click();
  await page.locator('[data-action="trash"]').click();
  const trash = page.getByTestId('entry-trash');
  const row = trash.locator('[data-trash-item="Saturday, November 21, 2026 · Good"]');
  await expect(row).toBeVisible();
  await expect(row.getByTestId('trash-days-left')).toHaveText('30 days left');
  await row.getByRole('button', { name: /Restore/u }).click();
  await expect(trash.getByTestId('trash-empty')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(today).toHaveAttribute('data-written', '');
  await expect(notes.getByRole('button', { name: 'Good' })).toHaveAttribute('aria-pressed', 'true');

  // Deleted again, and the day written anew: the one in the trash finds it taken.
  await deletedElsewhere(page, '11-21');
  await expect(today).not.toHaveAttribute('data-written');
  await editorOf(today).click();
  await page.keyboard.type('Sun after all.');
  await expect(today).toHaveAttribute('data-written', '');
  await page.getByTestId('diary-menu').click();
  await page.locator('[data-action="trash"]').click();
  await trash.getByRole('button', { name: /Restore/u }).click();
  await expect(page.getByTestId('problems')).toContainText('that day has an entry written since');
  await expect(today).toContainText('Sun after all.');
});
