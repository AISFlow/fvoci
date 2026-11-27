/**
 * Code blocks on the real server: a language chosen from the keys is the
 * stored body's, and nothing else of it changes; it opens again coloured.
 * A reader by the public link, with no setting of their own, reads the block
 * coloured with its language's name and a copy button, and no numbers.
 * Pictures of the reader's card, wide and at 390 pixels, light and dark, go
 * to `target/screens/code-blocks/` when `FVOCI_SCREENS` is set.
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import { screensDir } from '../e2e/screens.ts';

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

const SOURCE = 'def double(values):\n    return [value * 2 for value in values]  # twice';

const codeBody = (language: string): object => ({
  type: 'doc',
  content: [
    { type: 'paragraph', content: [{ type: 'text', text: 'The code:' }] },
    { type: 'codeBlock', attrs: { language }, content: [{ type: 'text', text: SOURCE }] },
  ],
});

async function storedBody(
  api: APIRequestContext,
  workspaceId: string,
  cardId: string,
): Promise<unknown> {
  const card = (await (
    await api.get(`/api/v1/w/${workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  return card.body;
}

test('a language chosen from the keys is stored, alone, and the block opens again coloured; a reader by the link reads it', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('coder');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Snippets',
    cards: [{ title: 'Doubling', status: 'To do' }],
  });
  const cardId = board.cards.Doubling ?? '';
  const cardAt = `/api/v1/w/${board.workspaceId}/cards/${cardId}`;
  const card = (await (await api.get(cardAt)).json()) as Schemas['Card'];
  // Stored as plain text in a language it is not: the reader corrects it.
  const written = await api.patch(cardAt, {
    data: { readAt: card.updatedAt, body: codeBody('text') },
  });
  expect(written.ok()).toBe(true);

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  const body = page.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  const block = body.locator('.doc-code');
  await expect(block.locator('button.doc-code-language')).toHaveAccessibleName(/text/);
  await block.locator('button.doc-code-language').focus();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.type('python');
  await page.keyboard.press('Enter');
  await expect(block.locator('button.doc-code-language')).toHaveAccessibleName(/Python/);
  await expect(block.locator('.doc-code-keyword').first()).toHaveText('def', { timeout: 30_000 });

  // The server has the language, and the body is otherwise what it was.
  await expect
    .poll(() => storedBody(api, board.workspaceId, cardId), { timeout: 30_000 })
    .toEqual(codeBody('python'));

  await page.reload();
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await expect(block.locator('button.doc-code-language')).toHaveAccessibleName(/Python/);
  await expect(block.locator('.doc-code-keyword').first()).toHaveText('def', { timeout: 30_000 });
  expect(errors).toEqual([]);

  // By the public link, signed in as nobody, with nothing chosen on this device.
  const shared = (await (
    await api.post(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}/public-link`, {
      data: { enabled: true },
    })
  ).json()) as Schemas['PublicLink'];
  const token = shared.token ?? '';
  const context = await browser.newContext({ locale: 'en-US', baseURL: base });
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  const reader = await context.newPage();
  const readerErrors = watchErrors(reader);
  const address = `/shared/${board.boardId}/${board.viewId}/${cardId}?r=${encodeURIComponent(token)}`;
  await reader.goto(address);
  const dialog = reader.getByTestId('card-dialog');
  await expect(dialog.getByText('The code:')).toBeVisible();
  const shown = dialog.locator('.doc-code');
  await expect(shown.locator('.doc-code-keyword').first()).toHaveText('def', { timeout: 30_000 });
  await expect(shown.locator('.doc-code-name')).toHaveText('Python');
  await expect(shown.locator('button.doc-code-language')).toHaveCount(0);
  await expect(shown.locator('.doc-code-line')).toHaveCount(0);
  await shown.locator('button.doc-code-copy').click();
  await expect(shown.getByRole('status')).not.toBeEmpty();
  expect(await reader.evaluate(() => navigator.clipboard.readText())).toBe(SOURCE);
  expect(readerErrors).toEqual([]);

  if (process.env.FVOCI_SCREENS) {
    const dir = screensDir('code-blocks');
    for (const width of [1280, 390]) {
      for (const dark of [false, true]) {
        await reader.emulateMedia({ colorScheme: dark ? 'dark' : 'light' });
        await reader.setViewportSize({ width, height: 844 });
        await reader.goto(address);
        await expect(shown.locator('.doc-code-keyword').first()).toBeVisible({ timeout: 30_000 });
        await shown.screenshot({
          path: `${dir}/shared-${width}-${dark ? 'dark' : 'light'}.png`,
          animations: 'disabled',
        });
      }
    }
  }
  await context.close();
});
