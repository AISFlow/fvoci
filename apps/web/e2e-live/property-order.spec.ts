/**
 * The order of a board's card properties on the real server: moved from a
 * card's property menu or the view's properties panel, stored by its own
 * request (not a patch of the board), followed by another member's open
 * card, and put back by undo.
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

test.use({ locale: 'en-US', viewport: { width: 1400, height: 900 } });

/** The names of the properties a page's open card shows, in order. */
const shown = (page: Page) =>
  page
    .getByTestId('card-dialog')
    .locator('[data-property] [data-testid="property-name"]')
    .allInnerTexts();

test('a property moved on a card is the board’s order, seen by another member, and undone', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('arranger');
  const helper = person('onlooker');
  await register(base, owner);
  await register(base, helper);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'In order',
    cards: [{ title: 'Ticket', status: 'To do' }],
    members: [{ who: helper, role: 'editor' }],
  });
  const path = boardPath(board, board.viewId, board.cards.Ticket ?? '');
  const stored = async () =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`)
      ).json()) as Schemas['Container']
    ).cardProperties.map((template) => template.name);

  const pages: Page[] = [];
  const watched: string[][] = [];
  for (const who of [owner, helper]) {
    const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
    pages.push(page);
    watched.push(watchErrors(page));
    await page.goto(path);
    await signInOnPage(page, who);
    await expect(page.getByTestId('card-dialog')).toBeVisible();
  }
  const [ownerPage, helperPage] = pages as [Page, Page];
  const sent: string[] = [];
  ownerPage.on('request', (request) => {
    if (request.method() !== 'GET')
      sent.push(`${request.method()} ${new URL(request.url()).pathname}`);
  });
  for (const page of pages) await expect.poll(() => shown(page)).toEqual(['Status', 'Note', 'Due']);

  // "Due" a place up, from its menu on the card.
  await ownerPage
    .getByTestId('card-dialog')
    .locator('[data-property]')
    .filter({ hasText: 'Due' })
    .getByTestId('property-name')
    .click();
  await ownerPage.getByTestId('property-menu').locator('[data-action="move-up"]').click();
  await expect.poll(stored).toEqual(['Status', 'Due', 'Note']);
  expect(sent).toEqual([
    `PUT /api/v1/w/${board.workspaceId}/boards/${board.boardId}/property-order`,
  ]);
  await expect.poll(() => shown(helperPage)).toEqual(['Status', 'Due', 'Note']);
  await ownerPage.keyboard.press('Escape');
  await expect(ownerPage.getByTestId('property-menu')).toHaveCount(0);

  // Undo (the card's menu has it) puts it back, there and here.
  await ownerPage.getByTestId('card-menu').click();
  await ownerPage.locator('[data-action="undo"]').click();
  await expect.poll(stored).toEqual(['Status', 'Note', 'Due']);
  await expect.poll(() => shown(helperPage)).toEqual(['Status', 'Note', 'Due']);
  await expect.poll(() => shown(ownerPage)).toEqual(['Status', 'Note', 'Due']);

  for (const page of pages) await page.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('a property moved in the view’s properties panel is the board’s order, which the cards follow', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('panelist');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Panel',
    cards: [{ title: 'One', status: 'To do', note: 'remember' }],
  });
  const stored = async () =>
    (
      (await (
        await api.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`)
      ).json()) as Schemas['Container']
    ).cardProperties.map((template) => template.name);
  const face = page.locator(`[data-card="${board.cards.One ?? ''}"] [data-property]`);
  const faceOrder = () =>
    face.evaluateAll((all) => all.map((one) => one.getAttribute('data-property')));
  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await page.locator('[data-tool="properties"]').click();
  const panel = page.getByTestId('properties-panel');
  await panel.getByRole('switch', { name: 'Status' }).click();
  await panel.getByRole('switch', { name: 'Note' }).click();
  await expect.poll(faceOrder).toEqual(['status', 'note']);
  await expect(panel.getByRole('switch', { name: 'Status' })).toHaveAttribute(
    'aria-checked',
    'true',
  );

  await expect(panel.getByRole('button', { name: 'Move Status up' })).toBeDisabled();
  await panel.getByRole('button', { name: 'Move Status down' }).click();
  await expect.poll(stored).toEqual(['Note', 'Status', 'Due']);
  await expect
    .poll(() =>
      panel
        .getByRole('switch')
        .evaluateAll((all) => all.map((one) => one.getAttribute('aria-label'))),
    )
    .toEqual(['Note', 'Status', 'Due']);
  // The cards of the view follow; the switch stays as it was, and the focus with the property.
  await expect.poll(faceOrder).toEqual(['note', 'status']);
  await expect(panel.getByRole('switch', { name: 'Status' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  await expect(panel.getByRole('button', { name: 'Move Status down' })).toBeFocused();
  await expect(page.getByText('Status is now 2 of 3')).toBeAttached();
  await api.dispose();
  expect(errors).toEqual([]);
});
