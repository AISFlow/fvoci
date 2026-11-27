/**
 * What the diary specs share: the app at a time in Seoul, in the own space,
 * its first diary open on today; the cells of the years of its page; and the
 * entries as the in-memory backend has them.
 */
import { expect, type Locator, type Page } from '@playwright/test';

import { openApp, showSidebar, sidebar } from './shell.support.ts';
import { goInside } from './wiki.support.ts';

/** A Saturday morning in Seoul. */
export const MORNING = new Date('2026-11-21T10:00:00+09:00');
export const ID = '[0-9a-f-]{36}';
export const ON_A_DAY = new RegExp(`/w/${ID}/diaries/(${ID})/(\\d{2}-\\d{2})$`, 'u');

export const diaryPage = (page: Page): Locator => page.getByTestId('diary-page');
export const year = (page: Page, date: string): Locator =>
  diaryPage(page).locator(`[data-testid="diary-year"][data-date="${date}"]`);
export const editors = (page: Page): Locator =>
  diaryPage(page).locator('[data-testid="diary-year"] [contenteditable="true"]');
export const editorOf = (cell: Locator): Locator =>
  cell.locator('[contenteditable="true"]').first();

export function dayOf(page: Page): { diaryId: string; monthDay: string } {
  const [, diaryId = '', monthDay = ''] = ON_A_DAY.exec(page.url()) ?? [];
  return { diaryId, monthDay };
}

/** How many entries the diary shown has on a day of the year, as the backend has them. */
export async function entriesOn(page: Page, monthDay: string): Promise<number> {
  return page.evaluate(
    async ([diaryId, day]) => {
      const dev = (
        window as unknown as {
          fvociDev: {
            backend: { diary: { entriesOn(id: string, md: string): Promise<unknown[]> } };
          };
        }
      ).fvociDev;
      return (await dev.backend.diary.entriesOn(diaryId, day)).length;
    },
    [dayOf(page).diaryId, monthDay] as const,
  );
}

export async function switchTo(page: Page, name: string): Promise<void> {
  await showSidebar(page);
  await sidebar(page).getByTestId('workspace-switcher').click();
  await page
    .getByRole('menu', { name: 'Switch workspace', exact: true })
    .getByRole('menuitem', { name, exact: true })
    .click();
}

/** The app at `time`, in the own space, its first diary made and open on today. */
export async function openDiary(page: Page, time = MORNING): Promise<void> {
  await page.clock.install({ time });
  await openApp(page);
  await switchTo(page, 'My space');
  await showSidebar(page);
  await sidebar(page).getByTestId('open-diary').click();
  await expect(page).toHaveURL(ON_A_DAY);
  await expect(diaryPage(page)).toBeVisible();
}

/** Goes to another day of the year of the diary shown, as a link would. */
export async function goToDay(page: Page, monthDay: string): Promise<void> {
  await goInside(page, new URL(page.url()).pathname.replace(/\d{2}-\d{2}$/u, monthDay));
  await expect(diaryPage(page)).toHaveAttribute('data-month-day', monthDay);
}
