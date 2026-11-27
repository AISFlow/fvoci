/**
 * What the live specs of the versions of a body share (body-versions.spec.ts,
 * body-versions-compare.spec.ts): a card's body written whole through the API,
 * its versions as the server lists them, a page signed in on the card, and
 * the dialog of its versions.
 */
import { expect, type APIRequestContext, type Browser, type Page } from '@playwright/test';

import { signInOnPage, type makeBoard, type Schemas } from './support.ts';

export type Board = Awaited<ReturnType<typeof makeBoard>>;

export const paragraph = (text: string): object => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

export const cardAt = (board: Board, cardId: string): string =>
  `/api/v1/w/${board.workspaceId}/cards/${cardId}`;

/** Writes a card's body whole through the API, as whoever `api` is signed in as: a paragraph, or a document. */
export async function writeWhole(
  api: APIRequestContext,
  board: Board,
  cardId: string,
  body: string | object,
): Promise<void> {
  const card = (await (await api.get(cardAt(board, cardId))).json()) as Schemas['Card'];
  const written = await api.patch(cardAt(board, cardId), {
    data: { body: typeof body === 'string' ? paragraph(body) : body, readAt: card.updatedAt },
  });
  expect(written.ok()).toBe(true);
}

export async function kept(
  api: APIRequestContext,
  board: Board,
  cardId: string,
): Promise<Schemas['BodyVersion'][]> {
  const answer = await api.get(`${cardAt(board, cardId)}/body-versions`);
  expect(answer.ok()).toBe(true);
  return (await answer.json()) as Schemas['BodyVersion'][];
}

export async function pageOf(
  browser: Browser,
  path: string,
  who: Parameters<typeof signInOnPage>[1],
): Promise<Page> {
  const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
  await page.goto(path);
  await signInOnPage(page, who);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  return page;
}

export const editor = (page: Page) =>
  page.getByTestId('card-body').locator('[contenteditable]').first();
export const versions = (page: Page) => page.getByTestId('body-versions');
export const rows = (page: Page) => versions(page).locator('[data-version]');
export const shown = (page: Page) => page.getByTestId('body-version');

export async function openVersions(page: Page): Promise<void> {
  await page.getByTestId('card-menu').click();
  await page.locator('[data-action="versions"]').click();
  await expect(versions(page)).toBeVisible();
}
