/**
 * A wiki on the real server copied whole: from its menu in the sidebar, the
 * copy has the same tree and bodies, and its picture is a file of its own
 * with the same bytes; saved from its menu as an archive and imported from
 * the settings into another workspace, the wiki made there is the same.
 */
import { createHash } from 'node:crypto';

import { expect, test, type APIRequestContext } from '@playwright/test';

import { sidebar } from '../e2e/shell.support.ts';
import { ON_A_WIKI, wikiPlaceOf } from '../e2e/wiki.support.ts';

import {
  addWorkspace,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

/** A picture of one pixel. */
const PICTURE = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64',
);

/** A wiki made through the API: where it is, and its pages by title. */
export interface MadeWiki {
  workspaceId: string;
  wikiId: string;
  pages: Record<string, string>;
  fileId: string;
}

async function answered<T>(answer: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
  expect(answer.ok(), `${answer.url()}: ${answer.status()} ${await answer.text()}`).toBe(true);
  return (await answer.json()) as T;
}

/** Uploads the picture to the wiki: the file's identifier. */
async function uploadPicture(api: APIRequestContext, ws: string, wikiId: string): Promise<string> {
  const opened = await answered<Schemas['UploadOpened']>(
    await api.post(`/api/v1/w/${ws}/containers/${wikiId}/uploads`, {
      data: { name: 'pic.png', size: PICTURE.length, contentType: 'image/png' },
    }),
  );
  const digest = `sha-256=:${createHash('sha256').update(PICTURE).digest('base64')}:`;
  const sent = await api.put(`/api/v1/w/${ws}/uploads/${opened.uploadId}/parts/1`, {
    headers: { 'Content-Type': 'application/octet-stream', 'Content-Digest': digest },
    data: PICTURE,
  });
  expect(sent.status()).toBe(204);
  const done = await answered<Schemas['UploadedFile']>(
    await api.post(`/api/v1/w/${ws}/uploads/${opened.uploadId}/complete`),
  );
  return done.fileId;
}

/**
 * A wiki of "Welcome" with "Getting started" below it, whose body shows a
 * picture, and "Policies" after it; "Welcome" names "Getting started".
 */
export async function makeWiki(api: APIRequestContext, title: string): Promise<MadeWiki> {
  const [home] = await answered<Schemas['UserWorkspace'][]>(
    await api.get('/api/v1/users/me/workspaces'),
  );
  const ws = home?.id ?? '';
  const wiki = await answered<Schemas['Container']>(
    await api.post(`/api/v1/w/${ws}/wikis`, { data: { title, visibility: 'private' } }),
  );
  const at = `/api/v1/w/${ws}/wikis/${wiki.id}/pages`;
  const make = async (title: string, parentId = '') =>
    answered<Schemas['Page']>(await api.post(at, { data: { title, parentId } }));
  const welcome = await make('Welcome');
  const started = await make('Getting started', welcome.id);
  await make('Policies');
  const fileId = await uploadPicture(api, ws, wiki.id);
  const write = async (page: Schemas['Page'], body: unknown) => {
    await answered(await api.patch(`${at}/${page.id}`, { data: { readAt: page.updatedAt, body } }));
  };
  await write(started, {
    type: 'doc',
    content: [{ type: 'image', attrs: { fileId } }],
  });
  await write(welcome, {
    type: 'doc',
    content: [
      {
        type: 'paragraph',
        content: [
          { type: 'text', text: 'Start with ' },
          { type: 'reference', attrs: { id: started.id } },
        ],
      },
    ],
  });
  const pages = { Welcome: welcome.id, 'Getting started': started.id };
  return { workspaceId: ws, wikiId: wiki.id, pages, fileId };
}

/** The wiki's tree as each page's title and its parent's, in the order listed. */
export async function treeOf(api: APIRequestContext, ws: string, wikiId: string) {
  const nodes = await answered<Schemas['PageNode'][]>(
    await api.get(`/api/v1/w/${ws}/wikis/${wikiId}/pages`),
  );
  const titles = new Map(nodes.map((node) => [node.id, node.title]));
  return nodes.map((node) => [node.title, titles.get(node.parentId) ?? ''] as const);
}

/**
 * Checks that the wiki `copyId` of the workspace `ws` is `made` again: the
 * same tree, "Welcome" naming its own "Getting started", whose picture is a
 * file of the copy's with the same bytes.
 */
export async function checkCopied(
  api: APIRequestContext,
  made: MadeWiki,
  ws: string,
  copyId: string,
) {
  expect(await treeOf(api, ws, copyId)).toEqual(await treeOf(api, made.workspaceId, made.wikiId));
  const nodes = await answered<Schemas['PageNode'][]>(
    await api.get(`/api/v1/w/${ws}/wikis/${copyId}/pages`),
  );
  const idOf = (title: string) => nodes.find((node) => node.title === title)?.id ?? '';
  const page = async (title: string) =>
    answered<Schemas['Page']>(
      await api.get(`/api/v1/w/${ws}/wikis/${copyId}/pages/${idOf(title)}`),
    );
  const welcome = JSON.stringify((await page('Welcome')).body);
  expect(welcome).toContain(idOf('Getting started'));
  const shown = JSON.stringify((await page('Getting started')).body);
  const fileId = /"fileId":"([^"]+)"/.exec(shown)?.[1] ?? '';
  expect(fileId).not.toBe('');
  expect(fileId).not.toBe(made.fileId);
  const file = await api.get(`/api/v1/w/${ws}/containers/${copyId}/files/${fileId}`);
  expect(file.status()).toBe(200);
  expect(Buffer.from(await file.body()).equals(PICTURE)).toBe(true);
}

test('a wiki is copied from its menu with its tree, its bodies and its picture', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('wiki-copies');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const made = await makeWiki(api, 'Handbook to copy');

  const errors = watchErrors(page);
  await page.goto(`/w/${made.workspaceId}/wikis/${made.wikiId}`);
  await signInOnPage(page, owner);
  await sidebar(page)
    .getByRole('button', { name: 'Menu of the wiki Handbook to copy', exact: true })
    .click();
  await page.getByRole('menuitem', { name: 'Duplicate wiki' }).click();
  await expect(page).toHaveURL(ON_A_WIKI);
  await expect.poll(() => wikiPlaceOf(page.url()).wikiId).not.toBe(made.wikiId);
  const copyId = wikiPlaceOf(page.url()).wikiId;
  await expect(sidebar(page).getByRole('link', { name: 'Handbook to copy copy' })).toBeVisible();
  await checkCopied(api, made, made.workspaceId, copyId);

  await api.dispose();
  expect(errors).toEqual([]);
});

test('a wiki saved as an archive is made again in another workspace', async ({
  page,
  baseURL,
}, testInfo) => {
  const base = baseURL ?? '';
  const owner = person('wiki-archive');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const made = await makeWiki(api, 'Handbook to archive');
  const second = addWorkspace(base, owner, 'Second');

  const errors = watchErrors(page);
  await page.goto(`/w/${made.workspaceId}/wikis/${made.wikiId}`);
  await signInOnPage(page, owner);
  const saving = page.waitForEvent('download');
  await sidebar(page)
    .getByRole('button', { name: 'Menu of the wiki Handbook to archive', exact: true })
    .click();
  await page.getByRole('menuitem', { name: 'Export the wiki' }).click();
  const archive = testInfo.outputPath('wiki.fvoci');
  await (await saving).saveAs(archive);

  await page.goto(`/w/${second}`);
  await sidebar(page).getByTestId('open-settings').click();
  await page.getByTestId('settings-dialog').getByTestId('import-archive').setInputFiles(archive);
  await expect(
    page.getByTestId('transfers').locator('[data-transfer="import"][data-finished]'),
  ).toContainText('wiki.fvoci is imported.');
  const wikis = async () =>
    answered<Schemas['Container'][]>(await api.get(`/api/v1/w/${second}/wikis`));
  await expect.poll(async () => (await wikis()).length).toBe(1);
  const [copy] = await wikis();
  expect(copy).toMatchObject({ kind: 'wiki', title: 'Handbook to archive', workspaceId: second });
  await checkCopied(api, made, second, copy?.id ?? '');
  await page.keyboard.press('Escape');
  await expect(sidebar(page).getByRole('link', { name: 'Handbook to archive' })).toBeVisible();

  await api.dispose();
  expect(errors).toEqual([]);
});
