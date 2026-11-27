/**
 * An image in a card's body on the real server: pasted into the editor, it
 * goes up as a file of the board, the stored body names it by its file, and
 * it is drawn again from the server when the card is opened later, in the
 * editor and in the read-only view of somebody who may only look.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';
import { documentBoundary } from '../e2e/keyboard.support.ts';

import {
  boardPath,
  makeBoard,
  type Person,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

/** A picture of 3 by 2 pixels. */
const PNG =
  'iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAIAAAASFvFNAAAAEElEQVR4nGM4YaMBQQxwFgBKJAcJKumIJAAAAABJRU5ErkJggg==';

interface Node {
  type: string;
  attrs?: Record<string, unknown>;
  content?: Node[];
}

function imagesIn(node: Node | null | undefined): string[] {
  if (!node) return [];
  const own = node.type === 'image' ? [String(node.attrs?.fileId)] : [];
  return [...own, ...(node.content ?? []).flatMap((child) => imagesIn(child))];
}

/**
 * Whether every picture in `place` has arrived and been drawn. The image without a size that
 * the editor puts after somebody's caret at the end of a line is not one.
 */
async function drawn(place: Locator): Promise<number[]> {
  return place
    .locator('img:not(.ProseMirror-separator)')
    .evaluateAll((images) =>
      images.map((image) =>
        (image as HTMLImageElement).complete ? (image as HTMLImageElement).naturalWidth : -1,
      ),
    );
}

async function openCard(page: Page, path: string, who: Person): Promise<void> {
  await page.goto(path);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
}

test('an image pasted into the body is a file of the board, and is drawn from the server again', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('painter');
  const onlooker = person('onlooker');
  await register(base, owner);
  await register(base, onlooker);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Pictures',
    cards: [{ title: 'The view from the hill', status: 'To do' }],
    members: [{ who: onlooker, role: 'viewer' }],
  });
  const cardId = board.cards['The view from the hill'] ?? '';
  const path = boardPath(board, board.viewId, cardId);
  const stored = async (): Promise<Schemas['Card']> =>
    (await (
      await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
    ).json()) as Schemas['Card'];

  const errors = watchErrors(page);
  // Every reading of a file answers with it.
  const files: number[] = [];
  page.on('response', (response) => {
    if (/\/api\/v1\/w\/[^/]+\/containers\/[^/]+\/files\//.test(response.url())) {
      files.push(response.status());
    }
  });
  await openCard(page, path, owner);
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  const editor = page.getByTestId('card-body').locator('[contenteditable]').first();
  await editor.click();
  await page.keyboard.type('From the top: ');
  await editor.evaluate((element, png) => {
    const bytes = Uint8Array.from(atob(png), (letter) => letter.charCodeAt(0));
    const data = new DataTransfer();
    data.items.add(new File([bytes], 'hill.png', { type: 'image/png' }));
    element.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }),
    );
  }, PNG);

  // Stored: the body names the picture by a file the board has.
  await expect
    .poll(async () => imagesIn((await stored()).body as unknown as Node), { timeout: 15_000 })
    .toHaveLength(1);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  const [fileId] = imagesIn((await stored()).body as unknown as Node);
  const file = await api.get(
    `/api/v1/w/${board.workspaceId}/containers/${board.boardId}/files/${fileId ?? ''}`,
  );
  expect(file.status()).toBe(200);
  expect(Buffer.from(await file.body()).equals(Buffer.from(PNG, 'base64'))).toBe(true);
  await expect.poll(() => drawn(editor)).toEqual([3]);

  // Opened again: drawn from the server.
  await page.reload();
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  await expect.poll(() => drawn(page.getByTestId('card-body'))).toEqual([3]);
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  expect(files.length).toBeGreaterThan(0);
  expect(files.every((status) => status === 200)).toBe(true);

  // Somebody who may only look sees it in the read-only view.
  const look = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const lookErrors = watchErrors(look);
  await openCard(look, path, onlooker);
  await expect(look.getByTestId('card-body').locator('[contenteditable="true"]')).toHaveCount(0);
  await expect.poll(() => drawn(look.getByTestId('card-body'))).toEqual([3]);
  await look.context().close();
  await api.dispose();
  expect([...errors, ...lookErrors]).toEqual([]);
});

test('a card does not close at once while an image is still going up for its body', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('patient');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Slow pictures',
    cards: [{ title: 'Hold on', status: 'To do' }],
  });
  const cardId = board.cards['Hold on'] ?? '';
  const errors = watchErrors(page);
  await openCard(page, boardPath(board, board.viewId, cardId), owner);
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');

  // The upload's completion waits until released: the image is still going up.
  let release: () => void = () => undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route('**/uploads/*/complete', async (route) => {
    await released;
    await route.continue();
  });
  const editor = page.getByTestId('card-body').locator('[contenteditable]').first();
  await editor.click();
  await editor.evaluate((element, png) => {
    const bytes = Uint8Array.from(atob(png), (letter) => letter.charCodeAt(0));
    const data = new DataTransfer();
    data.items.add(new File([bytes], 'slow.png', { type: 'image/png' }));
    element.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }),
    );
  }, PNG);
  await page.waitForRequest('**/uploads/*/complete');

  // Closing once is held: the image would be given up.
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  release();
  await expect
    .poll(
      async () =>
        imagesIn(
          (
            (await (
              await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
            ).json()) as Schemas['Card']
          ).body as unknown as Node,
        ),
      { timeout: 15_000 },
    )
    .toHaveLength(1);
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  await api.dispose();
  expect(errors).toEqual([]);
});

test('an image going up lands where it was put while somebody else writes above it', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('pictures');
  const colleague = person('writes');
  await register(base, owner);
  await register(base, colleague);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Pictures together',
    cards: [{ title: 'The trip', status: 'To do' }],
    members: [{ who: colleague, role: 'editor' }],
  });
  const cardId = board.cards['The trip'] ?? '';
  const path = boardPath(board, board.viewId, cardId);
  const stored = async (): Promise<Schemas['Card']> =>
    (await (
      await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
    ).json()) as Schemas['Card'];
  const pageOf = async (who: Person): Promise<Page> => {
    const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
    await openCard(page, path, who);
    await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
    return page;
  };
  const first = await pageOf(owner);
  const second = await pageOf(colleague);
  // Read at the end: errors that come later are in them too.
  const watched = [watchErrors(first), watchErrors(second)];
  const editorOf = (page: Page): Locator =>
    page.getByTestId('card-body').locator('[contenteditable]').first();

  // The end of the upload waits until the test lets it go.
  let release = (): void => undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  let held = 0;
  await first.route('**/uploads/*/complete', async (route) => {
    held += 1;
    await released;
    await route.continue();
  });

  try {
    await editorOf(first).click();
    await first.keyboard.type('The view from the top');
    await expect(editorOf(second)).toHaveText('The view from the top');
    await editorOf(first).evaluate((element, png) => {
      const bytes = Uint8Array.from(atob(png), (letter) => letter.charCodeAt(0));
      const data = new DataTransfer();
      data.items.add(new File([bytes], 'hill.png', { type: 'image/png' }));
      element.dispatchEvent(
        new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }),
      );
    }, PNG);
    await expect.poll(() => held).toBe(1);
    await expect(first.getByTestId('card-body').locator('.doc-upload')).toHaveCount(1);

    // Meanwhile the other writes two lines above it.
    await editorOf(second).click();
    await documentBoundary(second, 'start');
    await second.keyboard.type('Day one');
    await second.keyboard.press('Enter');
    await second.keyboard.type('Day two');
    await second.keyboard.press('Enter');
    // All of it arrived before the image goes in (an image put in while the other's Enter
    // rewrites its line at the same moment may merge on either side of it).
    await expect(editorOf(first).locator('p')).toHaveText([
      'Day one',
      'Day two',
      'The view from the top',
    ]);
  } finally {
    release();
  }

  // The picture is after the line it was put behind, on both pages and on the server.
  await expect
    .poll(
      async () =>
        ((await stored()).body as unknown as Node | null)?.content?.map((node) => node.type),
      {
        timeout: 15_000,
      },
    )
    .toEqual(['paragraph', 'paragraph', 'paragraph', 'image']);
  for (const page of [first, second]) {
    const body = page.getByTestId('card-body');
    await expect(body.locator('.ProseMirror > *')).toHaveCount(4);
    await expect(body.locator('.ProseMirror > p').nth(2)).toHaveText('The view from the top');
    await expect(
      body.locator('.ProseMirror > :nth-child(4)').locator('xpath=descendant-or-self::img'),
    ).toHaveCount(1);
    await expect.poll(() => drawn(body)).toEqual([3]);
  }
  await expect(first.locator('[data-problem]')).toHaveCount(0);
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});
