/**
 * The gallery on the real server: a card shows its body's first picture and
 * the beginning of its text, asked of the server only while a gallery is
 * shown, and follows a body changed elsewhere.
 */
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

/** A picture of one red pixel. */
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==',
  'base64',
);

const paragraph = (text: string) => ({ type: 'paragraph', content: [{ type: 'text', text }] });

test('a gallery shows each card’s first picture and the start of its text, read only for the gallery', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('curator');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Pictures',
    cards: [
      { title: 'Sunset', status: 'To do' },
      { title: 'Plain', status: 'To do' },
    ],
  });
  const sunset = board.cards.Sunset ?? '';

  const uploaded = await api.post(
    `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/files`,
    {
      multipart: { file: { name: 'pixel.png', mimeType: 'image/png', buffer: PIXEL } },
    },
  );
  expect(uploaded.ok()).toBe(true);
  const { fileId } = (await uploaded.json()) as Schemas['UploadedFile'];
  const writeBody = async (content: unknown[]) => {
    const now = (await (
      await api.get(`/api/v1/w/${board.workspaceId}/cards/${sunset}`)
    ).json()) as Schemas['Card'];
    const written = await api.patch(`/api/v1/w/${board.workspaceId}/cards/${sunset}`, {
      data: { readAt: now.updatedAt, body: { type: 'doc', content } },
    });
    expect(written.ok()).toBe(true);
  };
  await writeBody([
    paragraph('Taken from the pier'),
    { type: 'image', attrs: { fileId } },
    paragraph('Second line'),
  ]);
  const [gallery] = (await (
    await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/blocks`, {
      data: [
        {
          containerId: board.boardId,
          type: 'view',
          title: 'Gallery',
          createdAt: 1,
          updatedAt: 1,
          fields: {
            viewType: 'gallery',
            sort: [],
            shownProperties: [],
            cardOrder: [],
            filter: { operation: 'and', filters: [] },
          },
        },
      ],
    })
  ).json()) as Schemas['Block'][];

  const errors = watchErrors(page);
  const asked: string[] = [];
  page.on('request', (request) => {
    if (request.url().includes('/cards?')) asked.push(request.url());
  });

  // The board's other view asks for no previews.
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await expect(page.locator(`[data-card="${sunset}"]`)).toBeVisible();
  expect(asked.some((url) => url.includes('preview=true'))).toBe(false);

  await page.goto(boardPath(board, gallery?.id ?? ''));
  const card = page.getByTestId('gallery').locator(`[data-card="${sunset}"]`);
  const preview = card.locator('[data-slot="card-preview"]');
  await expect(preview).toContainText('Taken from the pier');
  await expect(preview).toContainText('Second line');
  const picture = preview.locator('img');
  await expect(picture).toHaveAttribute('src', new RegExp(`/files/${fileId}$`));
  await expect
    .poll(() => picture.evaluate((image: HTMLImageElement) => image.naturalWidth))
    .toBe(1);
  expect(asked.filter((url) => url.includes('preview=true'))).toHaveLength(1);
  // A card without a body shows none.
  await expect(
    page.locator(`[data-card="${board.cards.Plain ?? ''}"] [data-slot="card-preview"]`),
  ).toHaveCount(0);

  // Changed elsewhere: the gallery follows, without the picture now.
  await writeBody([paragraph('Rewritten somewhere else')]);
  await expect(preview).toHaveText('Rewritten somewhere else');
  await expect(preview.locator('img')).toHaveCount(0);

  await api.dispose();
  expect(errors).toEqual([]);
});
