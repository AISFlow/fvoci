/**
 * Taking bodies out as Markdown, in the world in memory: a page copied and
 * saved as a `.md` file from its menu, and the wiki saved as a ZIP of
 * Markdown files from its menu in the sidebar, beside its archive.
 */
import { readFile } from 'node:fs/promises';

import { expect, test } from '@playwright/test';

import { focusAtEnd } from './embeds.support.ts';
import { clipboardPermissions, engineOf } from './keyboard.support.ts';
import { openApp, showSidebar, sidebar } from './shell.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

test('a page is copied and saved as Markdown, and its wiki saved as Markdown files', async ({
  page,
}, testInfo) => {
  await page.context().grantPermissions(clipboardPermissions(engineOf(page) ?? 'chromium'));
  await openApp(page);
  await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Notes');
  await focusAtEnd(page.getByTestId('page-body'));
  await expect(pageBody(page)).toBeFocused();
  await page.keyboard.type('Written here');

  await page.getByTestId('page-menu').click();
  await page.locator('[data-action="copy-markdown"]').click();
  await expect(page.getByTestId('page-link-copied')).toHaveText('Copied as Markdown');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    '# Notes\n\nWritten here\n',
  );

  const saving = page.waitForEvent('download');
  await page.getByTestId('page-menu').click();
  await page.locator('[data-action="save-markdown"]').click();
  const saved = await saving;
  expect(saved.suggestedFilename()).toBe('Notes.md');
  const file = testInfo.outputPath('Notes.md');
  await saved.saveAs(file);
  expect(await readFile(file, 'utf8')).toBe('# Notes\n\nWritten here\n');

  await showSidebar(page);
  await sidebar(page)
    .getByRole('button', { name: 'Menu of the wiki Handbook', exact: true })
    .click();
  await expect(page.locator('[data-action="export"]')).toHaveText(/As a \.fvoci archive/);
  const zipping = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'As Markdown (.zip)' }).click();
  const zip = await zipping;
  expect(zip.suggestedFilename()).toMatch(/^markdown-\d{4}-\d{2}-\d{2}\.zip$/);
  const zipFile = testInfo.outputPath('markdown.zip');
  await zip.saveAs(zipFile);
  const bytes = await readFile(zipFile);
  expect(bytes.subarray(0, 4).toString('latin1')).toBe('PK\u0003\u0004');
  expect(bytes.toString('utf8')).toContain('Handbook/Notes.md');
});
