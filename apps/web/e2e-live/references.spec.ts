/**
 * References on the real server: a card named with `[[` from a card of
 * another board, the chip that opens it, the card named listing what names
 * it, and a second person, who is on the board of the card that names and
 * not on the other, shown no title at all.
 */
import { expect, test } from '@playwright/test';

import {
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  uniqueName,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

interface Stored {
  type: string;
  attrs?: Record<string, unknown>;
  content?: Stored[];
}

/** The references of a stored body, whole: a title would be an attribute beside the identifier. */
function references(node: Stored | null): Stored[] {
  if (!node) return [];
  return [
    ...(node.type === 'reference' ? [node] : []),
    ...(node.content ?? []).flatMap(references),
  ];
}

test('a reference put with `[[` opens the card it names, shows on that card, and tells who may not see it no title', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('linker');
  const onlooker = person('onlooker');
  await register(base, owner);
  await register(base, onlooker);
  const api = await signedIn(base, owner);
  // A title nothing else on the server has: the search finds this card alone.
  const title = uniqueName('Kept apart');
  const apart = await makeBoard(api, { title: 'Not for the onlooker', cards: [{ title }] });
  const shared = await makeBoard(api, {
    title: 'Read by both',
    cards: [{ title: 'Names the other' }],
    members: [{ who: onlooker, role: 'editor' }],
  });
  const target = apart.cards[title] ?? '';
  const source = shared.cards['Names the other'] ?? '';
  const workspace = shared.workspaceId;

  const errors = watchErrors(page);
  await page.goto(boardPath(shared, shared.viewId, source));
  await signInOnPage(page, owner);
  const body = page.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await body.locator('[contenteditable]').first().click();
  await page.keyboard.type(`See [[${title}`);
  // A card of another board is offered with the name of its board.
  const option = page
    .locator('[data-suggestions="reference"]')
    .getByRole('option', { name: `${title} · Not for the onlooker` });
  await expect(option).toHaveCount(1);
  await page.keyboard.press('Enter');
  const chip = body.locator('[data-reference]');
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await expect(chip).toHaveText(title);
  // The body is stored with the identifier, and no title.
  await expect
    .poll(async () => {
      const card = await api.get(`/api/v1/w/${workspace}/cards/${source}`);
      return references(((await card.json()) as { body: Stored | null }).body);
    })
    .toEqual([{ type: 'reference', attrs: { id: target } }]);

  // The chip opens the card on its own board.
  await chip.getByRole('link').click();
  await expect(page).toHaveURL(
    new RegExp(`/w/${workspace}/boards/${apart.boardId}/${apart.viewId}/${target}$`),
  );
  await expect(page.getByTestId('card-title')).toHaveValue(title);
  // Which lists what names it, and leads back.
  const backlinks = page.getByTestId('card-backlinks');
  await expect(backlinks.getByRole('link')).toHaveText(['Names the other']);
  await backlinks.getByRole('link').click();
  await expect(page).toHaveURL(new RegExp(`/boards/${shared.boardId}/[^/]+/${source}$`));
  await expect(page.getByTestId('card-title')).toHaveValue('Names the other');
  await expect(page.getByTestId('card-backlinks')).toHaveCount(0);
  expect(errors).toEqual([]);

  // The onlooker reads the card that names, and is not on the board of what it names.
  const context = await browser.newContext({ locale: 'en-US' });
  const theirs = await context.newPage();
  const theirErrors = watchErrors(theirs);
  const resolved = theirs.waitForResponse(
    (answer) =>
      answer.url().endsWith('/references/resolve') && answer.request().method() === 'POST',
  );
  await theirs.goto(boardPath(shared, shared.viewId, source));
  await signInOnPage(theirs, onlooker);
  // The server answers with nothing for it; only then is the chip what it stays.
  expect(await (await resolved).json()).toEqual([]);
  const unseen = theirs.getByTestId('card-body').locator('[data-reference]');
  await expect(unseen).toHaveAttribute('data-state', 'unseen');
  await expect(unseen).toHaveText("Link you can't see");
  await expect(unseen.getByRole('link')).toHaveCount(0);
  await expect(theirs.locator('body')).not.toContainText(title);
  expect(theirErrors).toEqual([]);
  await context.close();
  await api.dispose();
});
