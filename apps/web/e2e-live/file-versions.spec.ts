/**
 * The versions of a file on the real server: a member writes a new version
 * of an attached PDF from the viewer and goes back to the first; a version
 * written elsewhere meanwhile makes the next write a conflict, which the
 * list says and recovers from; and a reader by the board's public link is
 * shown the newest version and no list.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { koreanPdf } from '../tests/support/viewer-files.ts';

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

const PDF = 'application/pdf';
const viewer = (page: Page): Locator => page.getByTestId('file-viewer');
const counter = (page: Page): Locator => page.getByTestId('file-viewer-page');
const rows = (page: Page): Locator => page.getByTestId('file-versions-list').locator('li');
const versionRow = (page: Page, version: number): Locator =>
  page.getByTestId('file-versions-list').locator(`li[data-version="${String(version)}"]`);

async function view(page: Page, name: string): Promise<void> {
  await page
    .getByTestId('card-attachments')
    .locator('[data-attachment]', { hasText: name })
    .getByRole('link', { name })
    .click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
}

/** The list of versions, opened if it is not. */
async function versions(page: Page): Promise<void> {
  if (!(await page.getByTestId('file-versions').isVisible())) {
    await page.getByTestId('file-viewer-versions').click();
  }
  await expect(page.getByTestId('file-versions-list')).toBeVisible();
}

async function goBackTo(page: Page, version: number): Promise<void> {
  await versions(page);
  await versionRow(page, version).getByTestId('file-version-restore').click();
  await page
    .getByRole('alertdialog', { name: 'Restore this version?' })
    .getByRole('button', { name: 'Restore' })
    .click();
}

test('a member writes versions of a file and goes back to one; the public link shows the newest only', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('versions');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Papers',
    cards: [{ title: 'Read the minutes', status: 'To do' }],
  });
  const cardId = board.cards['Read the minutes'] ?? '';
  const at = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  await page
    .getByTestId('attach-input')
    .setInputFiles({ name: 'minutes.pdf', mimeType: PDF, buffer: koreanPdf(2) });
  await expect(page.getByTestId('card-attachments').locator('[data-attachment]')).toHaveCount(1);
  await view(page, 'minutes.pdf');
  await expect(counter(page)).toContainText('1 / 2');

  // The upload is the file's first version, by who uploaded it.
  await versions(page);
  await expect(rows(page)).toHaveCount(1);
  await expect(versionRow(page, 1).getByTestId('file-version-by')).toContainText(owner.username);
  await expect(versionRow(page, 1).getByTestId('file-version-current')).toBeVisible();

  // A new version from a file is what the viewer shows next.
  await page
    .getByTestId('file-version-input')
    .setInputFiles({ name: 'longer.pdf', mimeType: PDF, buffer: koreanPdf(3) });
  await expect(counter(page)).toContainText('1 / 3');
  await versions(page);
  await expect(rows(page)).toHaveCount(2);

  // Elsewhere, a third version is written; the list here still has two.
  const attached = (await (
    await api.get(
      `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/attachments`,
    )
  ).json()) as { fields: { fileId?: string } }[];
  const fileId = attached[0]?.fields.fileId ?? '';
  expect(fileId).not.toBe('');
  const uploaded = (await (
    await api.post(`${at}/files`, {
      multipart: { file: { name: 'elsewhere.pdf', mimeType: PDF, buffer: koreanPdf(4) } },
    })
  ).json()) as { fileId: string };
  const written = await api.post(`${at}/files/${fileId}/versions`, {
    data: { baseVersion: 2, fileId: uploaded.fileId },
  });
  expect(written.status()).toBe(201);
  // From the same version once more: a conflict, as a problem that says so.
  const again = await api.post(`${at}/files/${fileId}/versions/1/restore`, {
    data: { baseVersion: 2 },
  });
  expect(again.status()).toBe(409);
  expect(await again.json()).toMatchObject({
    type: expect.stringContaining('/problems/file-version-stale'),
    reason: 'file_version_stale',
  });

  // Going back from the list that is behind: refused, said, and the list is read again.
  await goBackTo(page, 1);
  const refusal = page.getByTestId('file-version-refusal');
  await expect(refusal).toHaveAttribute('data-refusal', 'stale');
  await expect(rows(page)).toHaveCount(3);
  await page.getByTestId('file-version-dismiss').click();
  await expect(refusal).toHaveCount(0);

  // From the list as it is now, it is written: a fourth version with the first one's bytes.
  await goBackTo(page, 1);
  await expect(counter(page)).toContainText('1 / 2');
  await versions(page);
  await expect(rows(page)).toHaveCount(4);
  await expect(versionRow(page, 4)).toContainText('Restored from version 1');
  // The version written elsewhere is still there to look at.
  await versionRow(page, 3).getByTestId('file-version-view').click();
  await expect(counter(page)).toContainText('1 / 4');
  await expect(page.getByTestId('file-viewer-older')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);

  // The public link: the newest version, no list, and no way to a version by the link's token.
  const shared = (await (
    await api.post(`${at}/public-link`, { data: { enabled: true } })
  ).json()) as {
    token?: string;
  };
  const token = encodeURIComponent(shared.token ?? '');
  const context = await browser.newContext({ locale: 'en-US' });
  const reader = await context.newPage();
  const readerErrors = watchErrors(reader);
  await reader.goto(`/shared/${board.boardId}/${board.viewId}/${cardId}?r=${token}`);
  await expect(reader.getByTestId('card-attachments')).toBeVisible();
  await view(reader, 'minutes.pdf');
  await expect(counter(reader)).toContainText('1 / 2');
  await expect(reader.getByTestId('file-viewer-versions')).toHaveCount(0);
  const anonymous = await context.request.get(
    `/api/v1/shared/${board.boardId}/files/${fileId}/versions?readToken=${token}`,
  );
  expect(anonymous.status()).toBe(404);
  const byBoard = await context.request.get(`${at}/files/${fileId}/versions?readToken=${token}`);
  expect(byBoard.status()).toBe(401);
  await context.close();

  await api.dispose();
  expect(errors).toEqual([]);
  expect(readerErrors).toEqual([]);
});
