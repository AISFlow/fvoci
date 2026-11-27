/**
 * The search of what cards say on a server without a search server (the
 * organizations server, on PostgreSQL): the database finds a card by the
 * letters of a word of its body, the word with a particle after it, at once,
 * and the card opens from the dialog; a card template and another
 * organization's card are not found.
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import { primaryKey } from '../e2e/keyboard.support.ts';

import { apiClient, makeBoard, person, signInOnPage, watchErrors, type Person } from './support.ts';
import type { Schemas } from './support.ts';

test.use({ locale: 'en-US' });

const body = (text: string): object => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

/** Starts an organization of its own for `who`, and a client signed in as them. */
async function founder(base: string, who: Person, name: string): Promise<APIRequestContext> {
  const api = await apiClient(base);
  const registered = await api.post('/api/v1/register', {
    data: { ...who, organizationName: name },
  });
  expect(registered.status(), await registered.text()).toBe(201);
  const signedIn = await api.post('/api/v1/login', {
    data: { username: who.username, password: who.password },
  });
  expect(signedIn.ok(), await signedIn.text()).toBe(true);
  return api;
}

async function cardWithBody(
  api: APIRequestContext,
  board: { workspaceId: string; boardId: string },
  title: string,
  text: string,
  isTemplate = false,
): Promise<string> {
  const answer = await api.post(`/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards`, {
    data: { title, properties: {}, body: body(text), isTemplate },
  });
  expect(answer.ok(), await answer.text()).toBe(true);
  return ((await answer.json()) as Schemas['Card']).id;
}

test('without a search server a card is found by the letters of its body and opened', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const errors = watchErrors(page);
  const owner = person('db-searcher');
  const other = person('db-other');
  const api = await founder(base, owner, 'Searching organization');
  const theirs = await founder(base, other, 'Other organization');
  const word = `산호${Date.now().toString(36)}`;
  const board = await makeBoard(api, { title: 'Reef notes' });
  await cardWithBody(api, board, 'Survey template', `${word} 양식`, true);
  const card = await cardWithBody(api, board, 'Reef survey', `오늘 ${word}을 세었다`);
  const elsewhere = await makeBoard(theirs, { title: 'Their reef' });
  await cardWithBody(theirs, elsewhere, 'Their survey', `${word}이 많다`);

  // Found at once: nothing is sent anywhere first.
  const answer = await api.get(`/api/v1/w/${board.workspaceId}/search`, { params: { q: word } });
  expect(answer.ok(), await answer.text()).toBe(true);
  const hits = (await answer.json()) as Schemas['SearchHit'][];
  expect(hits.map((hit) => hit.id)).toEqual([card]);

  await page.goto('/');
  await signInOnPage(page, owner);
  await expect(page.getByTestId('open-search')).toBeVisible();
  await page.keyboard.press(await primaryKey(page, 'k'));
  await page.getByTestId('search-field').fill(word);
  const dialog = page.getByTestId('search-dialog');
  await expect(dialog).toHaveAttribute('data-status', 'found');
  const options = dialog.getByRole('option');
  await expect(options).toHaveCount(1);
  await expect(options.first().locator('[data-hit-title]')).toHaveText('Reef survey');
  await expect(options.first().locator('mark')).toHaveText(word);
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('card-dialog')).toContainText('Reef survey');
  await expect(page).toHaveURL(new RegExp(`/boards/${board.boardId}/${board.viewId}/${card}$`));
  await api.dispose();
  await theirs.dispose();
  expect(errors).toEqual([]);
});
