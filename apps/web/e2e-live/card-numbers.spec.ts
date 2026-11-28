/**
 * The numbers of cards on the real server: the server numbers a board's
 * cards from 1 as they are made, the board shows each card's number and an
 * open card shows it at its head; `#` and a number in a body put that card
 * of the board as a chip, the body stored with its identifier and never the
 * number; and the search finds the card by its number on the board shown.
 */
import { expect, test } from '@playwright/test';

import { primaryKey } from '../e2e/keyboard.support.ts';
import { newWiki } from '../e2e/wiki.support.ts';

import {
  boardPath,
  makeBoard,
  person,
  putAwayWelcomeBoard,
  register,
  signedIn,
  signInOnPage,
  watchErrors,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

interface Stored {
  type: string;
  attrs?: Record<string, unknown>;
  content?: Stored[];
}

/** The references of a stored body, whole. */
function references(node: Stored | null): Stored[] {
  if (!node) return [];
  return [
    ...(node.type === 'reference' ? [node] : []),
    ...(node.content ?? []).flatMap(references),
  ];
}

const TITLES = ['Plan the week', 'Fix the login', 'Write the notes'] as const;

test('the board numbers its cards, `#3` in a body puts card 3, and the search finds it by `#3`', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('numberer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Numbered',
    cards: TITLES.map((title) => ({ title, status: 'To do' })),
  });
  const ids = TITLES.map((title) => board.cards[title] ?? '');
  const [first, , third] = ids;
  // The server numbers them in the order they were made.
  for (const [at, id] of ids.entries()) {
    const card = await api.get(`/api/v1/w/${board.workspaceId}/cards/${id}`);
    expect(((await card.json()) as { number: number | null }).number).toBe(at + 1);
  }

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  for (const [at, id] of ids.entries()) {
    const face = page.locator(`[data-card="${id}"]`);
    await expect(face.locator('[data-slot="card-number"]')).toHaveAttribute(
      'data-number',
      String(at + 1),
    );
    await expect(face).toHaveAccessibleName(new RegExp(`^number ${String(at + 1)}\\s`));
  }

  await page.goto(boardPath(board, board.viewId, first));
  await expect(page.getByTestId('card-location')).toContainText('#1');
  const body = page.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await body.locator('[contenteditable]').first().click();
  await page.keyboard.type('Next: #3');
  const option = page.locator('[data-suggestions="card-number"]').getByRole('option').first();
  await expect(option).toHaveText(/^#3 .*Write the notes$/);
  await page.keyboard.press('Enter');
  const chip = body.locator('[data-reference]');
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await expect(chip).toHaveText(/Write the notes$/);
  await expect
    .poll(async () => {
      const card = await api.get(`/api/v1/w/${board.workspaceId}/cards/${first ?? ''}`);
      return references(((await card.json()) as { body: Stored | null }).body);
    })
    .toEqual([{ type: 'reference', attrs: { id: third } }]);
  const stored = await api.get(`/api/v1/w/${board.workspaceId}/cards/${first ?? ''}`);
  expect(JSON.stringify(((await stored.json()) as { body: unknown }).body)).not.toContain('#3');

  // The search, from the board: `#3` is its card 3, first.
  await page.getByTestId('card-close').click();
  await expect(page.getByTestId('open-search')).toBeVisible();
  await page.keyboard.press(await primaryKey(page, 'k'));
  const search = page.getByTestId('search-dialog');
  await page.getByTestId('search-field').fill('#3');
  await expect(search).toHaveAttribute('data-status', 'found');
  const hit = search.getByRole('option').first();
  await expect(hit).toHaveAttribute('data-search-hit', third ?? '');
  await expect(hit.locator('[data-hit-title]')).toHaveText('Write the notes');
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('card-title')).toHaveValue('Write the notes');
  await expect(page.getByTestId('card-location')).toContainText('#3');
  expect(errors).toEqual([]);
  await api.dispose();
});

test('`#N` search finds cards across boards with the current board first, and navigates from a wiki', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('cross-number');
  const stranger = person('private-number');
  await register(base, owner);
  await register(base, stranger);
  const api = await signedIn(base, owner);
  const theirs = await signedIn(base, stranger);
  await putAwayWelcomeBoard(api);

  const boardA = await makeBoard(api, {
    title: 'Alpha Board',
    cards: [
      { title: 'Alpha One', status: 'To do' },
      { title: 'Alpha Two', status: 'To do' },
    ],
  });
  const boardB = await makeBoard(api, {
    title: 'Beta Board',
    workspaceId: boardA.workspaceId,
    cards: [
      { title: 'Beta One', status: 'To do' },
      { title: 'Beta Two', status: 'To do' },
    ],
  });
  // Another member's private board also has a #2 card: must stay isolated.
  await makeBoard(theirs, {
    title: 'Private Board',
    workspaceId: boardA.workspaceId,
    cards: [
      { title: 'Secret One', status: 'To do' },
      { title: 'Secret Two', status: 'To do' },
    ],
  });
  const alphaTwo = boardA.cards['Alpha Two'] ?? '';
  const betaTwo = boardB.cards['Beta Two'] ?? '';

  // Wait until the search index has both readable #2 cards (and never the private one).
  await expect
    .poll(async () => {
      const answer = await api.get(`/api/v1/w/${boardA.workspaceId}/search`, {
        params: { q: '#2' },
      });
      expect(answer.ok(), await answer.text()).toBe(true);
      return ((await answer.json()) as Schemas['SearchHit'][]).map((hit) => hit.id).sort();
    })
    .toEqual([alphaTwo, betaTwo].sort());

  const errors = watchErrors(page);
  await page.goto(boardPath(boardB, boardB.viewId));
  await signInOnPage(page, owner);
  await expect(page.getByTestId('open-search')).toBeVisible();

  // On Beta Board: `#2` lists Beta Two first, then Alpha Two with its board name.
  await page.keyboard.press(await primaryKey(page, 'k'));
  const search = page.getByTestId('search-dialog');
  await page.getByTestId('search-field').fill('#2');
  await expect(search).toHaveAttribute('data-status', 'found');
  const hits = search.getByRole('option');
  await expect(hits).toHaveCount(2);
  await expect(hits.nth(0)).toHaveAttribute('data-search-hit', betaTwo);
  await expect(hits.nth(0).locator('[data-hit-container]')).toHaveText('Beta Board');
  await expect(hits.nth(0).locator('[data-slot="card-number"]')).toContainText('#2');
  await expect(hits.nth(1)).toHaveAttribute('data-search-hit', alphaTwo);
  await expect(hits.nth(1).locator('[data-hit-container]')).toHaveText('Alpha Board');
  await expect(hits.nth(1).locator('[data-slot="card-number"]')).toContainText('#2');
  await page.keyboard.press('Escape');
  await expect(search).toHaveCount(0);

  // From a wiki (off any board), `#0002` still finds both #2 cards and navigates directly.
  await newWiki(page, 'Project Wiki');
  await page.keyboard.press(await primaryKey(page, 'k'));
  await page.getByTestId('search-field').fill('#0002');
  await expect(search).toHaveAttribute('data-status', 'found');
  const alphaHit = search.locator(`[data-search-hit="${alphaTwo}"]`);
  await expect(alphaHit).toBeVisible();
  await expect(alphaHit.locator('[data-hit-container]')).toHaveText('Alpha Board');
  await expect(alphaHit.locator('[data-slot="card-number"]')).toContainText('#2');
  await alphaHit.click();
  await expect(search).toHaveCount(0);
  await expect(page.getByTestId('card-title')).toHaveValue('Alpha Two');
  await expect(page.getByTestId('card-location')).toContainText('#2');

  expect(errors).toEqual([]);
  await api.dispose();
  await theirs.dispose();
});
