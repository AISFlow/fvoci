/**
 * A card's values on the real server: each change sends only the values it
 * sets or clears, a date goes in its new shape (a calendar day as text), and
 * two people changing different values of one card at about the same time
 * both keep what they did.
 */
import { expect, test, type Page } from '@playwright/test';

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

test.use({ locale: 'en-US', timezoneId: 'UTC' });

function property(page: Page, name: string) {
  return page
    .getByTestId('card-dialog')
    .locator('[data-property]')
    .filter({ has: page.locator('dt', { hasText: new RegExp(`^\\s*${name}\\s*$`) }) });
}

async function closePicker(page: Page): Promise<void> {
  const sheet = page.locator('[data-testid="bottom-sheet"][data-state="open"]');
  if ((await sheet.count()) > 0) await sheet.last().getByRole('button', { name: 'Done' }).click();
  else await page.keyboard.press('Escape');
  await expect(sheet).toHaveCount(0);
}

test('a date is stored as a calendar day, and a value is cleared by naming it alone', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('dater');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Dates',
    cards: [{ title: 'Renew the lease', status: 'To do', note: 'call first' }],
  });
  const cardId = board.cards['Renew the lease'] ?? '';
  const values = async (): Promise<Schemas['Card']['properties']> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
      ).json()) as Schemas['Card']
    ).properties;
  const patches: unknown[] = [];
  page.on('request', (request) => {
    if (request.method() === 'PATCH' && request.url().endsWith(`/cards/${cardId}`)) {
      patches.push(request.postDataJSON());
    }
  });

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  await expect(page.getByTestId('card-dialog')).toBeVisible();

  const due = property(page, 'Due');
  await due.locator('[data-slot="property-date"]').click();
  await page.locator('[data-slot="date-picker"] [data-value$="-15"]').first().click();
  await closePicker(page);
  await expect
    .poll(async () => JSON.stringify((await values()).due))
    .toMatch(/^\{"from":"\d{4}-\d{2}-15"\}$/);
  expect(await values()).toMatchObject({ status: board.options['To do'], note: 'call first' });
  const day = ((await values()).due as { from: string }).from;

  // Cleared: the patch names the value alone, and the other values stay.
  await due.locator('[data-slot="property-date"]').click();
  await page.getByRole('button', { name: 'Clear' }).click();
  await closePicker(page).catch(() => undefined);
  await expect.poll(values).toEqual({ status: board.options['To do'], note: 'call first' });
  expect(patches).toEqual([
    { updatedProperties: { due: { from: day } } },
    { deletedProperties: ['due'] },
  ]);

  await page.reload();
  await expect(due.locator('[data-slot="property-empty"]')).toBeVisible();
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  await api.dispose();
  expect(errors).toEqual([]);
});

test('two people change two values of one card at about the same time, and both stay', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('first');
  const colleague = person('second');
  await register(base, owner);
  await register(base, colleague);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Shared card',
    cards: [{ title: 'Plan the trip', status: 'To do', note: 'draft' }],
    members: [{ who: colleague, role: 'editor' }],
  });
  const cardId = board.cards['Plan the trip'] ?? '';
  const values = async (): Promise<Schemas['Card']['properties']> =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
      ).json()) as Schemas['Card']
    ).properties;

  const first = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const second = await (await browser.newContext({ locale: 'en-US' })).newPage();
  // Read at the end: errors that come later are in them too.
  const watched = [watchErrors(first), watchErrors(second)];
  const open = async (page: Page, who: typeof owner): Promise<void> => {
    await page.goto(boardPath(board, board.viewId, cardId));
    await signInOnPage(page, who);
    await expect(page.getByTestId('card-dialog')).toBeVisible();
  };
  await open(first, owner);
  await open(second, colleague);

  // The first person's change is held on its way until the second's is stored.
  let release = (): void => undefined;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  const held: unknown[] = [];
  await first.route(`**/cards/${cardId}`, async (route) => {
    if (route.request().method() !== 'PATCH') {
      await route.continue();
      return;
    }
    held.push(route.request().postDataJSON());
    await released;
    await route.continue();
  });

  try {
    await property(first, 'Status').locator('[data-slot="property-select"]').click();
    await first.getByRole('option', { name: 'Done' }).click();
    await expect.poll(() => held.length).toBe(1);

    await property(second, 'Note').locator('input').fill('book the train');
    await second.keyboard.press('Enter');
    await expect.poll(async () => (await values()).note).toBe('book the train');
  } finally {
    // A failure above must not leave the held request hanging the test.
    release();
  }
  await expect.poll(values).toEqual({ status: board.options.Done, note: 'book the train' });
  expect(held).toEqual([{ updatedProperties: { status: board.options.Done } }]);

  // Each page shows the other's change too.
  await expect(property(first, 'Note').locator('input')).toHaveValue('book the train');
  await expect(property(second, 'Status').locator('dd')).toContainText('Done');
  await expect(first.locator('[data-problem]')).toHaveCount(0);
  await expect(second.locator('[data-problem]')).toHaveCount(0);
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});
