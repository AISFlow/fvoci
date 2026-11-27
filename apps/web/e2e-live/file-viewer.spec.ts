/**
 * The file viewer on the real server: the server serves the frame's document
 * sandboxed and the app's files to the frame's opaque origin; a member views a
 * PDF and a Word document attached to a card, and a reader by the board's
 * public link views the same PDF, its bytes read with the link's token.
 */
import { expect, test, type Page } from '@playwright/test';

import { koreanDocx, koreanPdf } from '../tests/support/viewer-files.ts';

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
    .locator('[data-attachment]', { hasText: name })
    .getByRole('link', { name })
    .click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
}

async function close(page: Page): Promise<void> {
  await viewer(page).getByRole('button', { name: 'Close' }).click();
  await expect(viewer(page)).toHaveCount(0);
}

test('a PDF and a Word document are viewed on the card, and by the reader of the public link', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('viewer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Papers',
    cards: [{ title: 'Read the minutes', status: 'To do' }],
  });
  const cardId = board.cards['Read the minutes'] ?? '';

  // The frame's document is sandboxed by the server too, and the app's files may be read from it.
  const frameDocument = await api.get('/static/viewer-frame.html');
  expect(frameDocument.ok()).toBe(true);
  expect(frameDocument.headers()['content-security-policy']).toBe('sandbox allow-scripts');
  expect(frameDocument.headers()['access-control-allow-origin']).toBe('*');

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  await page.getByTestId('attach-input').setInputFiles([
    { name: 'minutes.pdf', mimeType: 'application/pdf', buffer: koreanPdf(2) },
    {
      name: '회의록.docx',
      mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      buffer: koreanDocx(),
    },
  ]);
  await expect(page.getByTestId('card-attachments').locator('[data-attachment]')).toHaveCount(2);

  await view(page, 'minutes.pdf');
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 2');
  const frame = page.frameLocator('[data-testid="file-viewer-frame"]');
  await expect(frame.locator('.page[data-page="1"] .textLayer')).toContainText('한국어 견본 1쪽');
  // Off the network, on the real server's origin as well.
  const inside = page.frames().find((each) => each.url().endsWith('/static/viewer-frame.html'));
  const fetched = await inside?.evaluate(() =>
    fetch('/api/v1/client-config').then(
      () => 'reached',
      () => 'refused',
    ),
  );
  expect(fetched).toBe('refused');
  await close(page);

  await view(page, '회의록.docx');
  await expect(frame.locator('section.docx table')).toContainText('설계 검토');
  await close(page);

  // The public link: a browser signed in as nobody.
  const at = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;
  const shared = (await (
    await api.post(`${at}/public-link`, { data: { enabled: true } })
  ).json()) as {
    token?: string;
  };
  const token = shared.token ?? '';
  const context = await browser.newContext({ locale: 'en-US' });
  const reader = await context.newPage();
  const readerErrors = watchErrors(reader);
  const fileRequests: string[] = [];
  reader.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.includes('/files/')) fileRequests.push(url.pathname);
  });
  await reader.goto(
    `/shared/${board.boardId}/${board.viewId}/${cardId}?r=${encodeURIComponent(token)}`,
  );
  await expect(reader.getByTestId('card-attachments')).toBeVisible();
  await view(reader, 'minutes.pdf');
  await expect(reader.getByTestId('file-viewer-page')).toContainText('1 / 2');
  // Read through the link: the shared path, nothing that names a person.
  expect(fileRequests.length).toBeGreaterThan(0);
  for (const path of fileRequests) expect(path).toMatch(/^\/api\/v1\/shared\//);
  await context.close();

  await api.dispose();
  expect(errors).toEqual([]);
  expect(readerErrors).toEqual([]);
});
