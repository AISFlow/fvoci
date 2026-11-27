/** What the tests of tables in a body do: read a table's cells, choose from the slash menu, paste. */
import { expect, type Locator, type Page } from '@playwright/test';

export const slashMenu = (page: Page): Locator => page.locator('[data-suggestions="slash"]');

/** The texts of the first table of a body, row by row. */
export const cellsOf = (body: Locator): Promise<string[][]> =>
  body
    .locator('table')
    .first()
    .evaluate((table) =>
      [...table.querySelectorAll('tr')].map((row) =>
        [...row.querySelectorAll('th, td')].map((cell) => cell.textContent),
      ),
    );

/** Chooses what the slash menu offers under a word, with the keyboard: arrows, then Enter. */
export async function slash(page: Page, word: string, chosen: string): Promise<void> {
  await page.keyboard.type(`/${word}`);
  const options = slashMenu(page).getByRole('option');
  await expect(options.filter({ hasText: chosen })).toHaveCount(1);
  const labels = await options.allTextContents();
  for (let at = 0; at < labels.indexOf(chosen); at += 1) await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(slashMenu(page)).toHaveCount(0);
}

/** Pastes what the clipboard holds, by type, into the body where its caret is. */
export async function paste(body: Locator, data: Record<string, string>): Promise<void> {
  await body.evaluate((element, given) => {
    const transfer = new DataTransfer();
    for (const [type, value] of Object.entries(given)) transfer.setData(type, value);
    element.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }),
    );
  }, data);
}
