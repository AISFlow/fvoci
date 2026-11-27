/**
 * Real documents in the editor, as a writer edits and saves them: run by
 * hand against a folder of samples, in the desktop project only,
 *
 *   FVOCI_HWP_SAMPLES=<folder> FVOCI_HWP_SAMPLE_FILES=<list> pnpm exec playwright test e2e/hwp-editor-samples.spec.ts --project desktop
 *
 * where the list names files of the folder, one a line. Each is attached and
 * opened in the editor. By default it is typed into at its start and saved:
 * what the save asks (the protection, what is left out, what reads back with
 * other counts) and how long it took to ask or to save are written, a line a
 * file, to `FVOCI_HWP_SAMPLE_REPORT`; `FVOCI_HWP_SAMPLE_WAIT` (milliseconds)
 * waits that long before saving (the frame counts the pages of the file as it
 * opens it, editor-frame/recounts.ts). What it asks is what
 * tests/logic/hwp-save-samples.test.ts finds in Node.
 */
import { appendFileSync, readFileSync } from 'node:fs';
import path from 'node:path';

import { expect, test } from '@playwright/test';

import { attach, openCard, viewer } from './file-viewer.support.ts';
import { editor, HWP_TYPE, HWPX_TYPE, said } from './hwp-editor.support.ts';

const FOLDER = process.env.FVOCI_HWP_SAMPLES ?? '';
const FILES = (process.env.FVOCI_HWP_SAMPLE_FILES ?? '')
  .split('\n')
  .map((line) => line.trim())
  .filter(Boolean);
const REPORT = process.env.FVOCI_HWP_SAMPLE_REPORT;
/** How long to wait after typing before saving, in milliseconds: the frame counts the pages meanwhile. */
const WAIT = Number(process.env.FVOCI_HWP_SAMPLE_WAIT ?? '0');
/** How long a large document may take to open in the viewer and in the editor. */
const OPENING = 120_000;

test.describe.configure({ timeout: 300_000 });

for (const file of FILES) {
  test(`a sample in the editor: ${file}`, async ({ page }, info) => {
    test.skip(info.project.name !== 'desktop', 'once, in the desktop project');
    const data = readFileSync(path.join(FOLDER, file));
    const zipped = data.subarray(0, 2).toString('latin1') === 'PK';
    const name = path.basename(file);
    await openCard(page);
    const row = await attach(page, name, data, zipped ? HWPX_TYPE : HWP_TYPE);
    await row.getByRole('link', { name }).click();
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown', { timeout: OPENING });
    await page.getByTestId('file-viewer-edit').click();
    await expect(editor(page)).toHaveAttribute('data-status', 'ready', { timeout: OPENING });
    await expect(page.getByTestId('file-editor-frame')).toBeFocused();

    await page.keyboard.type('1');
    await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
    if (WAIT > 0) await page.waitForTimeout(WAIT);
    const started = Date.now();
    await page.getByTestId('file-editor-save').click();
    const asked = page.getByTestId('file-editor-losses');
    const saved = said(page).filter({ hasText: /^Saved as version 2\.$/ });
    const failed = said(page).filter({ hasText: /not saved|could not|did not/ });
    await expect(asked.or(saved).or(failed)).toBeVisible({ timeout: 240_000 });
    const outcome = {
      file,
      bytes: data.byteLength,
      waited: WAIT,
      ms: Date.now() - started,
      asked: await asked.isVisible(),
      title: await asked.locator('[data-slot="alert-dialog-title"]').allInnerTexts(),
      losses: await asked.locator('ul:not([data-testid]) li').allInnerTexts(),
      recounts: await page.getByTestId('file-editor-recounts').locator('li').allInnerTexts(),
      said: await said(page).innerText(),
    };
    if (REPORT) appendFileSync(REPORT, `${JSON.stringify(outcome)}\n`);
    expect(await failed.count(), outcome.said).toBe(0);
  });
}
