/**
 * Attachments on the real server: a file goes up in parts through an
 * upload session, is attached to the card, comes down again as the same
 * bytes, and is deleted with its attachment. The server runs with the
 * smallest part it allows (5 MiB, `tools/live-server.ts`), so the file here
 * goes in two, and the stack of uploads at the bottom right says how far it
 * is after the first. A file over the limit is told there too, with the limit.
 */
import { createHash } from 'node:crypto';
import { closeSync, ftruncateSync, openSync, readFileSync } from 'node:fs';

import { expect, test } from '@playwright/test';

import {
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const sha = (bytes: Buffer): string => createHash('sha256').update(bytes).digest('hex');

test('a file of two parts is attached on the server and downloads as the same bytes', async ({
  page,
  baseURL,
}, info) => {
  const base = baseURL ?? '';
  const owner = person('filer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Files',
    cards: [{ title: 'Keep the scans', status: 'To do' }],
  });
  const cardId = board.cards['Keep the scans'] ?? '';
  const attachmentsOnServer = async (): Promise<Schemas['Block'][]> =>
    (await (
      await api.get(
        `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/attachments`,
      )
    ).json()) as Schemas['Block'][];

  // 6 MiB of bytes that differ from part to part.
  const bytes = Buffer.alloc(6 * 1024 * 1024);
  for (let at = 0; at < bytes.length; at += 1) bytes[at] = (at * 31 + (at >> 13)) % 251;
  const parts: string[] = [];
  page.on('request', (request) => {
    if (
      request.method() === 'PUT' &&
      /\/api\/v1\/w\/[^/]+\/uploads\/[^/]+\/parts\/\d+$/.test(request.url())
    ) {
      parts.push(request.url().replace(/.*\/parts\//, ''));
    }
  });

  // The second part waits until the stack has been seen to say how far the first went.
  let letSecondGo = (): void => undefined;
  const secondMayGo = new Promise<void>((resolve) => {
    letSecondGo = resolve;
  });
  await page.route(/\/api\/v1\/w\/[^/]+\/uploads\/[^/]+\/parts\/2$/, async (route) => {
    await secondMayGo;
    await route.continue();
  });

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  const section = page.getByTestId('card-attachments');
  await expect(section).toBeVisible();
  await page
    .getByTestId('attach-input')
    .setInputFiles({ name: 'scan.bin', mimeType: 'application/octet-stream', buffer: bytes });
  const told = page
    .getByTestId('transfers')
    .locator('[data-transfer="upload"][data-name="scan.bin"]');
  await expect(told).toContainText('Uploading scan.bin');
  await expect(told).toContainText('5.0 MiB of 6.0 MiB');
  letSecondGo();
  await expect(told).toHaveAttribute('data-finished', '');
  await expect(told).toContainText('scan.bin is uploaded.');
  const row = section.locator('[data-attachment]');
  await expect(row).toHaveCount(1);
  await expect(row).toContainText('scan.bin');
  await expect(row).toContainText('6.0 MiB');
  expect([...new Set(parts)].sort()).toEqual(['1', '2']);

  const [stored] = await attachmentsOnServer();
  expect(stored).toMatchObject({ title: 'scan.bin', fields: { size: bytes.length } });
  const fileId = String(stored?.fields.fileId);
  const fetched = await api.get(
    `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/files/${fileId}`,
  );
  expect(sha(await fetched.body())).toBe(sha(bytes));

  // The button beside it downloads it, after a reload too.
  await page.reload();
  const downloading = page.waitForEvent('download');
  await section.getByRole('button', { name: 'Download scan.bin' }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toBe('scan.bin');
  // Saved, not read where the browser put it: a browser reached over the network (Linux's
  // WebKit in its image) keeps the file on its own side.
  const saved = info.outputPath('scan.bin');
  await download.saveAs(saved);
  expect(sha(readFileSync(saved))).toBe(sha(bytes));

  await section.getByRole('button', { name: 'Delete scan.bin' }).click();
  await expect(row).toHaveCount(0);
  await expect.poll(attachmentsOnServer).toEqual([]);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('a file over the server’s limit is refused before it is sent, and says the limit', async ({
  page,
  baseURL,
}, testInfo) => {
  const base = baseURL ?? '';
  const owner = person('heavy');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const config = (await (await api.get('/api/v1/client-config')).json()) as Schemas['ClientConfig'];
  const board = await makeBoard(api, {
    title: 'Too big',
    cards: [{ title: 'The video', status: 'To do' }],
  });
  const cardId = board.cards['The video'] ?? '';
  let opened = 0;
  page.on('request', (request) => {
    if (request.method() === 'POST' && request.url().endsWith('/uploads')) opened += 1;
  });

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  // A file one byte over the limit, with no bytes written (a sparse file).
  const video = testInfo.outputPath('video.bin');
  const handle = openSync(video, 'w');
  ftruncateSync(handle, config.maxFileBytes + 1);
  closeSync(handle);
  await page.getByTestId('attach-input').setInputFiles(video);
  // Told where every upload is, with the limit, and not again where problems are.
  const told = page
    .getByTestId('transfers')
    .locator('[data-transfer="upload"][data-name="video.bin"]');
  await expect(told).toHaveAttribute('data-failed', 'too-large');
  await expect(told).toContainText(
    `It is larger than ${(config.maxFileBytes / 1024 / 1024).toFixed(1)} MiB.`,
  );
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  expect(opened).toBe(0);
  await api.dispose();
  expect(errors).toEqual([]);
});
