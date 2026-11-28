/**
 * One editor of a file at a time, on the real server and between two people
 * in two browsers: while one edits an HWPX attached to a card, the other sees
 * who and cannot start; once the first closes the editor, the second can.
 */
import { expect, test, type Page } from '@playwright/test';

import { koreanHwpx } from '../tests/support/hwp-files.ts';

import { boardPath, makeBoard, person, register, signedIn, watchErrors } from './support.ts';

test.use({ locale: 'en-US' });

const NAME = '회의록.hwpx';

const viewer = (page: Page) => page.getByTestId('file-viewer');
const editor = (page: Page) => page.getByTestId('file-editor');
const editing = (page: Page) => page.getByTestId('file-viewer-editing');
const editButton = (page: Page) => page.getByTestId('file-viewer-edit');

async function view(page: Page): Promise<void> {
  await page.getByTestId('card-attachments').getByRole('link', { name: NAME, exact: true }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
}

test('while one person edits a file the other sees who and cannot start, until the editor closes', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const writer = person('lease-writer');
  const other = person('lease-other');
  await register(base, writer);
  await register(base, other);
  const api = await signedIn(base, writer);
  const otherApi = await signedIn(base, other);
  const board = await makeBoard(api, {
    title: 'One at a time',
    cards: [{ title: 'Write the minutes', status: 'To do' }],
    members: [{ who: other, role: 'editor' }],
  });
  const cardId = board.cards['Write the minutes'] ?? '';
  const contexts = await Promise.all(
    [api, otherApi].map(async (signed) =>
      browser.newContext({
        baseURL: base,
        storageState: await signed.storageState(),
        locale: 'en-US',
      }),
    ),
  );
  const [first, second] = await Promise.all(contexts.map((context) => context.newPage()));
  if (!first || !second) throw new Error('no pages');
  const errors = [watchErrors(first), watchErrors(second)];
  try {
    await first.goto(boardPath(board, board.viewId, cardId));
    await first
      .getByTestId('attach-input')
      .setInputFiles({ name: NAME, mimeType: 'application/hwp+zip', buffer: koreanHwpx() });
    await expect(first.getByTestId('card-attachments').locator('[data-attachment]')).toHaveCount(1);
    await view(first);
    await editButton(first).click();
    await expect(editor(first)).toHaveAttribute('data-status', 'ready');

    await second.goto(boardPath(board, board.viewId, cardId));
    await view(second);
    await expect(editing(second)).toHaveAttribute('data-editing', 'true');
    await expect(editing(second)).toHaveText(`${writer.username} is editing this file.`);
    await expect(editButton(second)).toBeDisabled();

    // The server refuses the lease to the second as well, whatever their page shows.
    const attached = (await (
      await otherApi.get(
        `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/attachments`,
      )
    ).json()) as { fields: { fileId?: string } }[];
    const fileId = attached[0]?.fields.fileId ?? '';
    expect(fileId).not.toBe('');
    const lease = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/files/${fileId}/lease`;
    const refused = await otherApi.put(lease, { data: { leaseId: '' } });
    expect(refused.status()).toBe(409);
    expect(((await refused.json()) as { reason?: string }).reason).toBe('file_being_edited');

    await first.getByTestId('file-editor-close').click();
    await expect(editor(first)).toHaveCount(0);
    // The second's viewer asks again within its while: nobody edits, and "Edit" is back.
    await expect(editing(second)).toHaveAttribute('data-editing', 'false', { timeout: 20_000 });
    await editButton(second).click();
    await expect(editor(second)).toHaveAttribute('data-status', 'ready');
    // And now the first, who views it again, is the one told.
    await expect(viewer(first)).toHaveAttribute('data-status', 'shown');
    await first.getByTestId('file-viewer-close').click();
    await view(first);
    await expect(editing(first)).toHaveText(`${other.username} is editing this file.`);
    await expect(editButton(first)).toBeDisabled();
    await second.getByTestId('file-editor-close').click();
    await expect(editor(second)).toHaveCount(0);
    expect(errors.flat()).toEqual([]);
  } finally {
    await Promise.all(contexts.map((context) => context.close()));
    await api.dispose();
    await otherApi.dispose();
  }
});
