/**
 * What the page asks the real server once and no more: the reader's
 * workspaces as the app starts (the organization they administer is found
 * among those, not by reading them again), and the visit of a wiki that the
 * reader goes to without a page in the address, told with the page it opens
 * on rather than first without it.
 */
import { expect, test } from '@playwright/test';

import { boardPath, makeBoard, person, register, signedIn, watchErrors } from './support.ts';

test.use({ locale: 'en-US' });

interface Visit {
  containerId: string;
  cardId?: string;
}

test('the workspaces are read once, and a wiki is told as visited once, with its page', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('reads-once');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, { title: 'Plans', cards: [{ title: 'Kick-off' }] });
  const inWorkspace = `/api/v1/w/${board.workspaceId}`;
  const made = await api.post(`${inWorkspace}/wikis`, {
    data: { title: 'Handbook', visibility: 'private' },
  });
  expect(made.ok(), await made.text()).toBe(true);
  const wiki = (await made.json()) as { id: string };
  const written = await api.post(`${inWorkspace}/wikis/${wiki.id}/pages`, {
    data: { title: 'Welcome' },
  });
  expect(written.ok(), await written.text()).toBe(true);
  const firstPage = (await written.json()) as { id: string };

  const context = await browser.newContext({
    baseURL: base,
    storageState: await api.storageState(),
    locale: 'en-US',
  });
  const page = await context.newPage();
  const errors = watchErrors(page);
  const lists: string[] = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname === '/api/v1/users/me/workspaces')
      lists.push(request.url());
  });
  const visits: Visit[] = [];
  /** Settles once the server has told the reader as being on `pageId`. */
  let toldOn: (pageId: string) => Promise<void> = () => Promise.reject(new Error('no socket'));
  page.on('websocket', (socket) => {
    socket.on('framesent', ({ payload }) => {
      if (typeof payload !== 'string') return;
      const sent = JSON.parse(payload) as { type?: string } & Visit;
      if (sent.type === 'setPresence') visits.push(sent);
    });
    toldOn = (pageId) =>
      socket
        .waitForEvent('framereceived', {
          predicate: ({ payload }) => {
            if (typeof payload !== 'string') return false;
            const told = JSON.parse(payload) as {
              type?: string;
              people?: { cardIds: string[] }[];
            };
            return (
              told.type === 'presenceChanged' &&
              (told.people ?? []).some((one) => one.cardIds.includes(pageId))
            );
          },
        })
        .then(() => undefined);
  });

  // The organization the reader administers is found in a single membership read: the sidebar leads to it.
  const administered = page.waitForResponse((response) =>
    new URL(response.url()).pathname.endsWith('/api/v1/users/me/organizations'),
  );
  await page.goto(boardPath(board, board.viewId));
  await expect(page.getByText('Kick-off')).toBeVisible();
  await administered;
  expect(lists).toHaveLength(1);

  // To the wiki by its address alone: it opens on its first page.
  const onPage = toldOn(firstPage.id);
  await page.getByTestId('sidebar').locator(`li[data-board="${wiki.id}"] > a`).click();
  await expect(page).toHaveURL(`/w/${board.workspaceId}/wikis/${wiki.id}/${firstPage.id}`);
  await onPage;
  const ofWiki = visits.filter((visit) => visit.containerId === wiki.id);
  expect(ofWiki).toEqual([expect.objectContaining({ cardId: firstPage.id })]);

  expect(lists).toHaveLength(1);
  expect(errors).toEqual([]);
  await context.close();
});
