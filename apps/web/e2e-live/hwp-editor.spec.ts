/**
 * The editor of Hangul documents on the real server: a member edits an HWPX
 * attached to a card and saves it as the file's new version, which the
 * server hands back with the edit; an edit of a version that somebody wrote
 * over meanwhile is refused by the server and kept as a file of its own. The
 * editor's frame is served sandboxed, as the viewer's is.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { HWP_TEXT, koreanHwpx } from '../tests/support/hwp-files.ts';

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

const HWPX_TYPE = 'application/hwp+zip';
const TYPED = 'EDITED IN FVOCI';

const viewer = (page: Page): Locator => page.getByTestId('file-viewer');
const editor = (page: Page): Locator => page.getByTestId('file-editor');
const said = (page: Page): Locator => page.getByTestId('file-editor-said');
const shownText = (page: Page): Locator =>
  page.frameLocator('[data-testid="file-viewer-frame"]').locator('.page[data-page="1"] .hwp-text');

async function view(page: Page, name: string): Promise<void> {
  await page.getByTestId('card-attachments').getByRole('link', { name, exact: true }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
}

async function type(page: Page, text: string): Promise<void> {
  await expect(page.getByTestId('file-editor-frame')).toBeFocused();
  await page.keyboard.type(text);
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
}

test('a member edits an HWPX and saves it as a version; over a newer version, as a file of its own', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('hwpedit');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Hangul papers',
    cards: [{ title: 'Write the minutes', status: 'To do' }],
  });
  const cardId = board.cards['Write the minutes'] ?? '';
  const at = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;

  const errors = watchErrors(page);
  const frames: { status: number; policy: string | undefined }[] = [];
  page.on('response', (response) => {
    if (!new URL(response.url()).pathname.endsWith('/static/editor-frame.html')) return;
    frames.push({
      status: response.status(),
      policy: response.headers()['content-security-policy'],
    });
  });
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  await page
    .getByTestId('attach-input')
    .setInputFiles({ name: '회의록.hwpx', mimeType: HWPX_TYPE, buffer: koreanHwpx() });
  const attachments = page.getByTestId('card-attachments').locator('[data-attachment]');
  await expect(attachments).toHaveCount(1);

  await view(page, '회의록.hwpx');
  await page.getByTestId('file-viewer-edit').click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  // The frame's document came from the server sandboxed: without an origin, scripts only.
  expect(frames).toEqual([{ status: 200, policy: 'sandbox allow-scripts' }]);

  await type(page, TYPED);
  await page.getByTestId('file-editor-save').click();
  await expect(said(page)).toHaveText('Saved as version 2.');

  // The server has the edit as the file's second version, by who saved it.
  const attached = (await (
    await api.get(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`, {
      params: { parentId: cardId, type: 'attachment' },
    })
  ).json()) as { fields: { fileId?: string } }[];
  const fileId = attached[0]?.fields.fileId ?? '';
  expect(fileId).not.toBe('');
  const listed = await api.get(`${at}/files/${fileId}/versions`);
  expect(listed.status()).toBe(200);
  expect(JSON.stringify(await listed.json())).toContain('"version":2');

  // Somebody writes the third version elsewhere while the second is still being edited here.
  const uploaded = (await (
    await api.post(`${at}/files`, {
      multipart: { file: { name: '회의록.hwpx', mimeType: HWPX_TYPE, buffer: koreanHwpx() } },
    })
  ).json()) as { fileId: string };
  const written = await api.post(`${at}/files/${fileId}/versions`, {
    data: { baseVersion: 2, fileId: uploaded.fileId },
  });
  expect(written.status()).toBe(201);

  await type(page, '!');
  await page.keyboard.press('Control+s');
  await page.getByTestId('file-editor-confirm-version').click();
  const stale = page.getByTestId('file-editor-stale');
  await expect(stale).toContainText('The file got a newer version meanwhile');
  await page.getByTestId('file-editor-stale-apart').click();
  await expect(said(page)).toHaveText('Saved as a file of its own: 회의록 (edited).hwpx');
  await page.getByTestId('file-editor-close').click();
  await expect(editor(page)).toHaveCount(0);

  // The file is as they wrote it, in three versions; the edit is beside it on the card.
  const after = await api.get(`${at}/files/${fileId}/versions`);
  expect(JSON.stringify(await after.json())).not.toContain('"version":4');
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(shownText(page)).toContainText(HWP_TEXT.title);
  await expect(shownText(page)).not.toContainText(TYPED);
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await expect(attachments).toHaveCount(2);

  // Read again from the server, the file of its own has both edits.
  await page.reload();
  await expect(attachments).toHaveCount(2);
  await view(page, '회의록 (edited).hwpx');
  await expect(shownText(page)).toContainText(`${TYPED}!`);

  await api.dispose();
  expect(errors).toEqual([]);
});
