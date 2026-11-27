/** Native caret navigation: macOS uses Command and an arrow for the document's ends. */
import { expect, type Locator, type Page } from '@playwright/test';

import { isApplePlatform } from '../src/lib/keys.ts';

/**
 * The browser's platform, as the app reads it. An Apple device emulated by Playwright's WebKit
 * on Linux keeps Linux's platform (only its User-Agent is Apple's): Control there.
 */
export async function primaryKey(page: Page, key: string): Promise<string> {
  return `${await primaryName(page)}+${key}`;
}

export async function primaryName(page: Page): Promise<'Meta' | 'Control'> {
  return isApplePlatform(await page.evaluate(() => navigator.platform)) ? 'Meta' : 'Control';
}

export async function documentBoundary(page: Page, edge: 'start' | 'end'): Promise<void> {
  const key =
    process.platform === 'darwin'
      ? `Meta+${edge === 'start' ? 'ArrowUp' : 'ArrowDown'}`
      : `Control+${edge === 'start' ? 'Home' : 'End'}`;
  await page.keyboard.press(key);
}

/** The end of the row the caret is on: Home and End on macOS scroll and leave the caret. */
export async function lineBoundary(
  page: Page,
  edge: 'start' | 'end',
  { select = false } = {},
): Promise<void> {
  const shift = select ? 'Shift+' : '';
  const key =
    process.platform === 'darwin'
      ? `${shift}Meta+${edge === 'start' ? 'ArrowLeft' : 'ArrowRight'}`
      : `${shift}${edge === 'start' ? 'Home' : 'End'}`;
  await page.keyboard.press(key);
}

/** The engine that draws the page: the one place the tests ask for it. */
export function engineOf(page: Page): string | undefined {
  return page.context().browser()?.browserType().name();
}

/**
 * What a test that reads back what the page copied is granted. Chromium refuses every
 * permission a grant leaves out, the writing too; WebKit knows none for writing and lets a
 * page write after a press.
 */
export function clipboardPermissions(engine: string): string[] {
  return engine === 'webkit' ? ['clipboard-read'] : ['clipboard-read', 'clipboard-write'];
}

/** Safari on macOS uses Option+Tab to include buttons regardless of host preferences. */
export async function tabKey(page: Page, backwards = false): Promise<string> {
  const safariOnMac = process.platform === 'darwin' && engineOf(page) === 'webkit';
  return `${safariOnMac ? 'Alt+' : ''}${backwards ? 'Shift+' : ''}Tab`;
}

/** Dropdowns use arrows; sheets use Tab after their opening focus has arrived. */
export async function menuTo(page: Page, target: Locator): Promise<void> {
  const dropdown = page.getByTestId('dropdown').filter({ has: target });
  if (await dropdown.count()) {
    await expect
      .poll(() => dropdown.evaluate((node) => node.contains(document.activeElement)))
      .toBe(true);
    for (let step = 0; step < 20; step += 1) {
      if (await target.evaluate((node) => node === document.activeElement)) return;
      await page.keyboard.press('ArrowDown');
    }
  } else {
    const dialog = page.getByRole('dialog').filter({ has: target });
    await expect
      .poll(() => dialog.evaluate((node) => node.contains(document.activeElement)))
      .toBe(true);
    for (let step = 0; step < 20; step += 1) {
      if (await target.evaluate((node) => node === document.activeElement)) return;
      await page.keyboard.press(await tabKey(page));
    }
  }
  await expect(target).toBeFocused();
}
