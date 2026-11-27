/**
 * The people of a wiki on the real server: its admin shares it from the head
 * of its tree (a colleague added, their role changed, the wiki opened to the
 * workspace), an administrator of the workspace manages a wiki for the
 * invited only without reading it and joins it from there, and a guest comes
 * in by a link to one wiki and writes in it, with nothing else in sight. Each
 * change is checked on the server. With `FVOCI_SCREENS` set the screens are
 * pictured in Korean under `target/screens/wiki-people/`, wide and at 390
 * pixels.
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import { sidebar, unfold } from '../e2e/shell.support.ts';
import { showTree, wikiScreen } from '../e2e/wiki.support.ts';

import {
  ADMIN,
  koreanShots,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  uniqueName,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

/** A wiki of `api`'s account with one page, in its first workspace. */
async function makeWiki(api: APIRequestContext, title: string, visibility: 'open' | 'private') {
  const [workspace] = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const workspaceId = workspace?.id ?? '';
  const inWorkspace = `/api/v1/w/${workspaceId}`;
  const wiki = (await (
    await api.post(`${inWorkspace}/wikis`, { data: { title, visibility } })
  ).json()) as Schemas['Container'];
  const page = (await (
    await api.post(`${inWorkspace}/wikis/${wiki.id}/pages`, { data: { title: 'Hello' } })
  ).json()) as Schemas['Page'];
  return {
    workspaceId,
    wikiId: wiki.id,
    pageId: page.id,
    path: `/w/${workspaceId}/wikis/${wiki.id}`,
  };
}

async function members(api: APIRequestContext, wiki: { workspaceId: string; wikiId: string }) {
  return (await (
    await api.get(`/api/v1/w/${wiki.workspaceId}/containers/${wiki.wikiId}/members`)
  ).json()) as Schemas['ContainerMember'][];
}

async function idOf(api: APIRequestContext): Promise<string> {
  return ((await (await api.get('/api/v1/users/me')).json()) as Schemas['User']).id;
}

test('a wiki is shared from its head: a colleague added, their role changed, and the wiki opened', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-sharer');
  const colleague = person('wiki-colleague');
  await register(base, owner);
  await register(base, colleague);
  const api = await signedIn(base, owner);
  const colleagueApi = await signedIn(base, colleague);
  const colleagueId = await idOf(colleagueApi);
  const title = uniqueName('Shared handbook');
  const wiki = await makeWiki(api, title, 'private');

  const page = await (await browser.newContext({ locale: 'en-US', baseURL: base })).newPage();
  const errors = watchErrors(page);
  await page.goto(wiki.path);
  await signInOnPage(page, owner);
  await showTree(page);
  await wikiScreen(page).getByTestId('share-wiki').click();
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog.getByRole('heading', { name: 'Share wiki' })).toBeVisible();
  await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(1);

  await dialog.getByTestId('share-search').fill(colleague.username);
  await dialog.getByRole('button', { name: `Add ${colleague.username}` }).click();
  await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(2);
  await expect
    .poll(async () => (await members(api, wiki)).find((one) => one.userId === colleagueId))
    .toMatchObject({ role: 'editor' });

  await dialog
    .getByRole('combobox', { name: `Role of ${colleague.username}` })
    .selectOption('viewer');
  await expect
    .poll(async () => (await members(api, wiki)).find((one) => one.userId === colleagueId))
    .toMatchObject({ role: 'viewer' });

  const open = dialog.getByTestId('share-open');
  await expect(open).not.toBeChecked();
  await open.click();
  await expect(open).toBeChecked();
  await expect
    .poll(async () => {
      const listed = (await (
        await colleagueApi.get(`/api/v1/w/${wiki.workspaceId}/wikis`)
      ).json()) as Schemas['Container'][];
      return listed.find((one) => one.id === wiki.wikiId)?.visibility;
    })
    .toBe('open');
  await koreanShots(browser, page, 'wiki-people', 'share-dialog', async (seen) => {
    await expect(seen.getByTestId('page-title')).toHaveValue('Hello');
    await showTree(seen);
    await wikiScreen(seen).getByTestId('share-wiki').click();
    await expect(seen.getByTestId('share-members').locator('li')).toHaveCount(2);
  });
  expect(errors).toEqual([]);
  await Promise.all([api.dispose(), colleagueApi.dispose(), page.context().close()]);
});

test('a wiki for the invited only is managed by the workspace’s administrator without reading it, and joined', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-keeper');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const title = uniqueName('Private notes');
  const wiki = await makeWiki(api, title, 'private');

  const errors = watchErrors(page);
  await page.goto(wiki.path);
  await signInOnPage(page, ADMIN);
  const managed = page.getByTestId('wiki-managed');
  await expect(managed).toBeVisible();
  await expect(managed.getByRole('heading')).toHaveText(title);
  await expect(managed).toContainText('Join it to read its pages');
  await expect(wikiScreen(page)).toHaveCount(0);
  await expect(page.getByText('Hello')).toHaveCount(0);

  // Its members, and nothing of its content: no openness, no public link.
  await managed.getByRole('button', { name: 'Manage members' }).click();
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog.getByTestId('share-members').locator('li')).toHaveCount(1);
  await expect(dialog.getByTestId('share-open')).toHaveCount(0);
  await expect(dialog.getByTestId('share-public')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await koreanShots(browser, page, 'wiki-people', 'managed-wiki', async (seen) => {
    await expect(seen.getByTestId('wiki-managed')).toBeVisible();
  });

  await managed.getByRole('button', { name: 'Join', exact: true }).click();
  await expect(wikiScreen(page)).toBeVisible();
  await expect(page.locator('h1[data-testid="page-title"]')).toHaveText('Hello');
  const adminId = await idOf(await signedIn(base, ADMIN));
  expect((await members(api, wiki)).find((one) => one.userId === adminId)).toMatchObject({
    role: 'viewer',
  });
  expect(errors).toEqual([]);
  await api.dispose();
});

test('a guest comes in by a link to one wiki, and writes in it with nothing else in sight', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-host');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const title = uniqueName('Guest wiki');
  const wiki = await makeWiki(api, title, 'private');
  const other = await makeWiki(api, uniqueName('Not for guests'), 'open');

  const errors = watchErrors(page);
  await page.goto(wiki.path);
  await signInOnPage(page, owner);
  await showTree(page);
  await wikiScreen(page).getByTestId('share-wiki').click();
  const share = page.getByTestId('share-dialog');
  await unfold(share.getByTestId('share-guest-toggle'));
  await expect(share).toContainText('A guest sees this wiki and nothing else');
  await share.getByTestId('share-guest-make').click();
  const field = share.getByTestId('share-guest-link');
  await expect(field).toHaveValue(/\/invite\/[\w-]{43}$/);
  const link = await field.inputValue();

  const context = await browser.newContext({ locale: 'en-US' });
  const guestPage = await context.newPage();
  const guestErrors = watchErrors(guestPage);
  await guestPage.goto(link);
  await expect(guestPage.getByTestId('invite-summary')).toContainText(`the wiki “${title}”`);
  const guest = person('wiki-guest');
  const form = guestPage.getByTestId('invite-register-form');
  await form.getByLabel(/^user name$/i).fill(guest.username);
  await form.getByLabel(/email/i).fill(guest.email);
  await form.getByLabel(/^password$/i).fill(guest.password);
  await form.getByRole('button', { name: 'Create account and join' }).click();

  // At the wiki's own address, an editor of it.
  await expect(guestPage).toHaveURL(new RegExp(`/wikis/${wiki.wikiId}(/${wiki.pageId})?$`));
  await expect(guestPage.getByTestId('page-title')).toHaveValue('Hello');
  await guestPage.getByTestId('page-title').fill('Written by the guest');
  await guestPage.keyboard.press('Enter');
  await expect
    .poll(async () => {
      const pages = (await (
        await api.get(`/api/v1/w/${wiki.workspaceId}/wikis/${wiki.wikiId}/pages`)
      ).json()) as Schemas['Page'][];
      return pages.find((one) => one.id === wiki.pageId)?.title;
    })
    .toBe('Written by the guest');
  await expect(sidebar(guestPage).getByRole('link', { name: `${title}, wiki` })).toBeVisible();
  await expect(sidebar(guestPage).getByText('Not for guests')).toHaveCount(0);
  await expect(guestPage.getByTestId('wiki-join')).toHaveCount(0);
  const guestApi = await signedIn(base, guest);
  expect(
    (await guestApi.get(`/api/v1/w/${other.workspaceId}/wikis/${other.wikiId}/pages`)).status(),
  ).toBe(404);
  expect(errors).toEqual([]);
  expect(guestErrors).toEqual([]);
  await Promise.all([api.dispose(), guestApi.dispose(), context.close()]);
});
