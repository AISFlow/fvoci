/**
 * The tree of a wiki on the real server: pages moved with the keyboard, from
 * "Move…" and dragged, there again where they were put after a reload, as
 * the server keeps them (`movePage`); and a page moved that somebody else
 * deleted before the page heard of it.
 */
import { expect, test, type Page } from '@playwright/test';

import { nextFrame, pointOn } from '../e2e/kanban.support.ts';
import {
  nameThePage,
  newWiki,
  pageAction,
  pageTree,
  showTree,
  treeItem,
  treeShape,
} from '../e2e/wiki.support.ts';

import {
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const row = (page: Page, name: string) =>
  pageTree(page).locator(`[data-page][aria-label="${name}"]`);

test('pages moved with the keys, from "Move…" and dragged are where they were put after a reload', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-moves');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Roadmap' });

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, owner);
  // Private: the open wikis of the workspace are other tests' to count.
  const wikiId = await newWiki(page, 'Tree moves', false);
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Welcome');
  await pageAction(page, 'Welcome', 'add-child');
  await nameThePage(page, 'Getting started');
  await pageAction(page, 'Welcome', 'add-after');
  await nameThePage(page, 'Policies');
  await pageAction(page, 'Policies', 'add-after');
  await nameThePage(page, 'Travel');
  await showTree(page);

  // Up among its siblings with the keyboard, into "Policies" from "Move…", and
  // "Getting started" dragged before "Welcome".
  await treeItem(page, 'Travel').focus();
  await page.keyboard.press('Alt+ArrowUp');
  await expect
    .poll(() => treeShape(page))
    .toEqual(['Welcome', '  Getting started', 'Travel', 'Policies']);
  await pageAction(page, 'Welcome', 'move');
  const dialog = page.getByTestId('move-page');
  await dialog.getByRole('radio', { name: 'Policies, level 1' }).check();
  await dialog.getByRole('button', { name: 'Move', exact: true }).click();
  // Gone with its backdrop, which would take the press of the drag.
  await expect(dialog).toBeHidden();
  await expect(page.locator('[data-slot="dialog-overlay"]')).toHaveCount(0);
  await showTree(page);
  await expect
    .poll(() => treeShape(page))
    .toEqual(['Travel', 'Policies', '  Welcome', '    Getting started']);
  const from = await pointOn(row(page, 'Getting started'));
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + 4, from.y - 6, { steps: 3 });
  const above = await pointOn(row(page, 'Travel'), 0.1);
  await page.mouse.move(above.x, above.y, { steps: 6 });
  await nextFrame(page);
  await expect(row(page, 'Travel')).toHaveAttribute('data-drop', 'before');
  await page.mouse.up();
  const moved = ['Getting started', 'Travel', 'Policies', '  Welcome'];
  await expect.poll(() => treeShape(page)).toEqual(moved);

  // The server keeps them so, and the page read anew shows them so. A move shows before
  // it is written: the server is read once the tree is no longer settling.
  await expect(pageTree(page)).not.toHaveAttribute('data-settling');
  const pages = (await (
    await api.get(`/api/v1/w/${workspaceId}/wikis/${wikiId}/pages`)
  ).json()) as Schemas['PageNode'][];
  const top = pages.filter((one) => one.parentId === '').map((one) => one.title);
  expect(top).toEqual(['Getting started', 'Travel', 'Policies']);
  await page.reload();
  await showTree(page);
  await expect.poll(() => treeShape(page)).toEqual(moved);
  expect(errors).toEqual([]);
  await api.dispose();
});

test('a page deleted and brought back, from the trash and by undo, is there after a reload', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-trash');
  await register(base, owner);
  const api = await signedIn(base, owner);
  await makeBoard(api, { title: 'Roadmap' });

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, owner);
  await newWiki(page, 'Tree trash', false);
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Welcome');
  await pageAction(page, 'Welcome', 'add-child');
  await nameThePage(page, 'Getting started');
  await pageAction(page, 'Welcome', 'add-after');
  await nameThePage(page, 'Policies');
  const whole = ['Welcome', '  Getting started', 'Policies'];
  const remove = async (): Promise<void> => {
    await pageAction(page, 'Welcome', 'delete');
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Delete' }).click();
    await showTree(page);
    await expect.poll(() => treeShape(page)).toEqual(['Policies']);
  };

  // From the trash of the wiki, which says one went with it.
  await remove();
  await page.getByTestId('open-page-trash').click();
  const trash = page.getByTestId('page-trash');
  const item = trash.getByTestId('trash-items').locator('li', { hasText: 'Welcome' });
  await expect(item.getByTestId('trash-below')).toHaveText('Pages below: 1');
  await item.getByRole('button', { name: 'Restore Welcome' }).click();
  await expect(trash.getByTestId('trash-empty')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(trash).toBeHidden();
  await expect.poll(() => treeShape(page)).toEqual(whole);
  await page.reload();
  await showTree(page);
  await expect.poll(() => treeShape(page)).toEqual(whole);

  // By undo.
  await remove();
  await page.getByTestId('undo').click();
  await expect.poll(() => treeShape(page)).toEqual(whole);
  await page.reload();
  await showTree(page);
  await expect.poll(() => treeShape(page)).toEqual(whole);
  expect(errors).toEqual([]);
  await api.dispose();
});

test('a page moved after somebody else deleted it is said not to move, and leaves the tree with its pages', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-gone');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Roadmap' });
  // What the server sends the page waits here while there is a list to hold it: the
  // deleting is not heard before the move is answered.
  let held: (() => void)[] | undefined;
  await page.routeWebSocket(/\/ws$/, (socket) => {
    const server = socket.connectToServer();
    server.onMessage((message) => {
      if (held)
        held.push(() => {
          socket.send(message);
        });
      else socket.send(message);
    });
  });

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, owner);
  const wikiId = await newWiki(page, 'Tree gone', false);
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Welcome');
  await pageAction(page, 'Welcome', 'add-child');
  await nameThePage(page, 'Getting started');
  await pageAction(page, 'Welcome', 'add-after');
  await nameThePage(page, 'Policies');
  await showTree(page);
  const whole = ['Welcome', '  Getting started', 'Policies'];
  await expect.poll(() => treeShape(page)).toEqual(whole);
  await expect(pageTree(page)).not.toHaveAttribute('data-settling');

  // "Welcome" deleted elsewhere (here, by the API as the same person), unheard of here.
  held = [];
  const inWiki = `/api/v1/w/${workspaceId}/wikis/${wikiId}/pages`;
  const pages = (await (await api.get(inWiki)).json()) as Schemas['PageNode'][];
  const welcome = pages.find((one) => one.title === 'Welcome')?.id ?? '';
  const deleted = await api.delete(`${inWiki}/${welcome}`);
  expect(deleted.ok(), await deleted.text()).toBe(true);
  expect(await treeShape(page)).toEqual(whole);

  await treeItem(page, 'Welcome').focus();
  await page.keyboard.press('Alt+ArrowDown');
  await expect(page.getByTestId('problems')).toContainText(
    'Somebody else deleted that page, so it was not moved.',
  );
  await expect.poll(() => treeShape(page)).toEqual(['Policies']);
  await expect(treeItem(page, 'Policies')).toBeFocused();

  // What was held arrives, the deleting among it: the tree stays as it is.
  const late = held;
  held = undefined;
  for (const send of late) send();
  await expect(pageTree(page)).not.toHaveAttribute('data-settling');
  expect(await treeShape(page)).toEqual(['Policies']);
  await page.reload();
  await showTree(page);
  await expect.poll(() => treeShape(page)).toEqual(['Policies']);
  expect(errors).toEqual([]);
  await api.dispose();
});
