/**
 * A board write the server stored, whose answer the page never got (the
 * connection was reset after the server answered): the page reads the
 * board again, finds the change stored, keeps it on the screen, and undo
 * takes it back.
 */
import { expect, test } from '@playwright/test';

import { boardPath, makeBoard, person, register, signedIn, signInOnPage } from './support.ts';

test.use({ locale: 'en-US' });

test('a change stored without an answer stays and is undone', async ({ page, baseURL }) => {
  const base = baseURL ?? '';
  const owner = person('lost-answer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Alpha' });
  const path = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;
  const savedTitle = async (): Promise<string> => {
    const response = await api.get(path);
    expect(response.ok()).toBe(true);
    return ((await response.json()) as { title: string }).title;
  };
  try {
    await page.goto('/');
    await signInOnPage(page, owner);
    await page.goto(boardPath(board, board.viewId));
    const title = page.getByTestId('board-title');
    const undo = page.getByTestId('undo');
    await expect(title).toHaveValue('Alpha');

    let lost = false;
    await page.route(`**${path}`, async (route) => {
      if (!lost && route.request().method() === 'PATCH') {
        lost = true;
        // The server stores the change; its answer does not reach the page.
        const answer = await route.fetch();
        expect(answer.ok()).toBe(true);
        await route.abort('connectionreset');
        return;
      }
      await route.continue();
    });
    const reread = page.waitForResponse(
      (response) => response.url().endsWith(path) && response.request().method() === 'GET',
    );
    await title.fill('Kept');
    await title.press('Enter');
    expect((await reread).ok()).toBe(true);
    expect(await savedTitle()).toBe('Kept');
    await expect(title).toHaveValue('Kept');

    const written = page.waitForResponse(
      (response) => response.url().endsWith(path) && response.request().method() === 'PATCH',
    );
    await undo.click();
    expect((await written).ok()).toBe(true);
    await expect(title).toHaveValue('Alpha');
    expect(await savedTitle()).toBe('Alpha');
  } finally {
    await api.dispose();
  }
});
