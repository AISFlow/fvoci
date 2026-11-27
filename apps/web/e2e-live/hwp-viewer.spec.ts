/**
 * HWP and HWPX documents in the file viewer on the real server: the server
 * hands the frame's opaque origin the renderer and its WebAssembly (a module
 * of tens of megabytes before compression), and a member views both kinds
 * attached to a card.
 */
import { expect, test, type Page } from '@playwright/test';

import { HWP_TEXT, koreanHwp, koreanHwpx } from '../tests/support/hwp-files.ts';

import {
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const viewer = (page: Page) => page.getByTestId('file-viewer');

async function view(page: Page, name: string): Promise<void> {
  await page
    .getByTestId('card-attachments')
    // Exactly: one name begins the other.
    .getByRole('link', { name, exact: true })
    .click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
}

test('an HWPX and an HWP document are viewed on the card, their renderer served to the frame', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('hangul');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Hangul papers',
    cards: [{ title: 'Read the minutes', status: 'To do' }],
  });
  const cardId = board.cards['Read the minutes'] ?? '';

  const errors = watchErrors(page);
  const served: { path: string; status: number; origin: string | undefined }[] = [];
  page.on('response', (response) => {
    const path = new URL(response.url()).pathname;
    if (!/\/static\/(hwp|rhwp-wasm)-/.test(path)) return;
    served.push({
      path,
      status: response.status(),
      origin: response.headers()['access-control-allow-origin'],
    });
  });
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  await page.getByTestId('attach-input').setInputFiles([
    { name: '회의록.hwpx', mimeType: 'application/hwp+zip', buffer: koreanHwpx() },
    { name: '회의록.hwp', mimeType: 'application/x-hwp', buffer: koreanHwp() },
  ]);
  await expect(page.getByTestId('card-attachments').locator('[data-attachment]')).toHaveCount(2);

  const frame = page.frameLocator('[data-testid="file-viewer-frame"]');
  for (const name of ['회의록.hwpx', '회의록.hwp']) {
    await view(page, name);
    await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 2');
    const text = frame.locator('.page[data-page="1"] .hwp-text');
    await expect(text).toContainText(HWP_TEXT.title);
    await expect(text).toContainText('설계 검토');
    await viewer(page).getByRole('button', { name: 'Close' }).click();
    await expect(viewer(page)).toHaveCount(0);
  }
  // The renderer and its WebAssembly came from the app's files, to the frame's opaque origin.
  expect(served.some((each) => each.path.includes('rhwp-wasm'))).toBe(true);
  for (const each of served) {
    expect(each.status, each.path).toBe(200);
    expect(each.origin, each.path).toBe('*');
  }

  await api.dispose();
  expect(errors).toEqual([]);
});
