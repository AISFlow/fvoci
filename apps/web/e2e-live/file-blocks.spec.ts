/**
 * A file block on the real server: a PDF chosen through the slash menu of a
 * wiki's page goes up as a file of the wiki and its block shows in another
 * editor's browser, which downloads the same bytes by its button and opens
 * the file in the viewer by Enter; somebody who may only look downloads them
 * and opens the viewer by the file's name where the body is only shown; a
 * copy of the wiki has a file of its own with those bytes, and a reader by
 * the wiki's public link downloads them too and views them, the viewer
 * naming nobody. The server offers the file as an attachment that is never
 * sniffed. Pictures of the block, wide and 390 pixels across, go to
 * `target/files-f1/`.
 */
import path from 'node:path';

import {
  expect,
  test,
  type APIRequestContext,
  type Download,
  type Page,
  type Response,
} from '@playwright/test';

import { hideSidebar, sidebar } from '../e2e/shell.support.ts';
import { humanFileSize } from '../src/lib/logic/file-size.ts';
import { koreanPdf } from '../tests/support/viewer-files.ts';

import {
  person,
  register,
  type Person,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const SHOTS = path.resolve(import.meta.dirname, '../../../target/files-f1');

/** A PDF of one page in Korean, which the viewer shows: its bytes come back the same. */
const PDF = koreanPdf(1);
const NAME = '2026-11 회의록 (minutes) of the steering committee.pdf';

interface Node {
  type: string;
  attrs?: Record<string, unknown>;
  content?: Node[];
}

function fileBlocksIn(node: Node | null | undefined): string[] {
  if (!node) return [];
  const own = node.type === 'file' ? [String(node.attrs?.fileId)] : [];
  return [...own, ...(node.content ?? []).flatMap((child) => fileBlocksIn(child))];
}

async function answered<T>(answer: Awaited<ReturnType<APIRequestContext['get']>>): Promise<T> {
  expect(answer.ok(), `${answer.url()}: ${answer.status()} ${await answer.text()}`).toBe(true);
  return (await answer.json()) as T;
}

async function bytesOf(download: Download): Promise<Buffer> {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

/** Opens a page of a wiki, signing in on the way. */
async function openPage(page: Page, at: string, who: Person): Promise<void> {
  await page.goto(at);
  await signInOnPage(page, who);
  await expect(page.getByTestId('page-body')).toBeVisible();
}

/** Downloads the file of the block in `page` by its link: the name it is saved under and its bytes. */
async function download(page: Page): Promise<[string, Buffer]> {
  const block = page.getByTestId('page-body').locator('[data-file-block]');
  await expect(block).toHaveAttribute('data-state', 'shown');
  const [saved] = await Promise.all([
    page.waitForEvent('download'),
    block.getByRole('link', { name: `Download ${NAME}` }).click(),
  ]);
  return [saved.suggestedFilename(), await bytesOf(saved)];
}

/**
 * Opens the file of the block in `page` in the viewer by its name, pressed by the mouse or by
 * Enter once it has the focus, sees it shown and closes it; the focus goes back to the name.
 * Gives what the viewer held, and the addresses and bodies of what the page was sent meanwhile.
 */
async function view(page: Page, by: 'mouse' | 'keyboard' = 'mouse'): Promise<string[]> {
  const sent: Promise<string>[] = [];
  const onResponse = (response: Response): void => {
    const type = response.headers()['content-type'] ?? '';
    const body = /json|text|html|javascript/.test(type) ? response.text().catch(() => '') : '';
    sent.push(Promise.resolve(body).then((text) => `${response.url()}\n${text}`));
  };
  page.on('response', onResponse);
  const name = page
    .getByTestId('page-body')
    .locator('[data-file-block]')
    .getByRole('button', { name: `View ${NAME}` });
  if (by === 'mouse') await name.click();
  else {
    await name.focus();
    await page.keyboard.press('Enter');
  }
  const viewer = page.getByTestId('file-viewer');
  await expect(viewer).toHaveAttribute('data-status', 'shown');
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 1');
  const held = await viewer.evaluate((element) => element.outerHTML);
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer).toHaveCount(0);
  await expect(name).toBeFocused();
  page.off('response', onResponse);
  return [held, ...(await Promise.all(sent))];
}

test('a PDF put into a page is a file block that others, a copy and the public link download and view', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('filer');
  const colleague = person('colleague');
  const onlooker = person('onlooker');
  await register(base, owner);
  await register(base, colleague);
  await register(base, onlooker);
  const api = await signedIn(base, owner);
  const [home] = await answered<Schemas['UserWorkspace'][]>(
    await api.get('/api/v1/users/me/workspaces'),
  );
  const ws = home?.id ?? '';
  // Whom the viewer must never name to the public link's reader.
  const people = [((await (await api.get('/api/v1/users/me')).json()) as Schemas['User']).id];
  const wiki = await answered<Schemas['Container']>(
    await api.post(`/api/v1/w/${ws}/wikis`, {
      data: { title: 'Committee', visibility: 'private' },
    }),
  );
  const pages = `/api/v1/w/${ws}/wikis/${wiki.id}/pages`;
  const minutes = await answered<Schemas['Page']>(
    await api.post(pages, { data: { title: 'Minutes', parentId: '' } }),
  );
  for (const [who, editor] of [
    [colleague, true],
    [onlooker, false],
  ] as const) {
    const found = await answered<Schemas['User'][]>(
      await api.get(`/api/v1/w/${ws}/users`, { params: { search: who.username } }),
    );
    const userId = found.find((user) => user.username === who.username)?.id ?? '';
    people.push(userId);
    await answered(
      await api.post(`/api/v1/w/${ws}/containers/${wiki.id}/members`, {
        data: { userId, role: editor ? 'editor' : 'viewer' },
      }),
    );
  }
  const at = `/w/${ws}/wikis/${wiki.id}/${minutes.id}`;
  const stored = async (pageId: string, wikiId = wiki.id): Promise<Node> =>
    (
      await answered<Schemas['Page']>(
        await api.get(`/api/v1/w/${ws}/wikis/${wikiId}/pages/${pageId}`),
      )
    ).body as unknown as Node;

  // The owner chooses the PDF through the slash menu.
  const errors = watchErrors(page);
  await openPage(page, at, owner);
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  const editor = page.getByTestId('page-body').locator('[contenteditable="true"]');
  await editor.click();
  await page.keyboard.type('Agenda and minutes:');
  await page.keyboard.press('Enter');
  await page.keyboard.type('/file');
  const menu = page.locator('[data-suggestions="slash"]');
  await expect(menu.getByRole('option')).toHaveText(['File or image']);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.keyboard.press('Enter'),
  ]);
  await chooser.setFiles({ name: NAME, mimeType: 'application/pdf', buffer: PDF });
  await expect
    .poll(async () => fileBlocksIn(await stored(minutes.id)), { timeout: 15_000 })
    .toHaveLength(1);
  const [fileId] = fileBlocksIn(await stored(minutes.id));
  const block = page.getByTestId('page-body').locator('[data-file-block]');
  await expect(block).toHaveAttribute('data-state', 'shown');
  await expect(block).toHaveAttribute('aria-label', `File: ${NAME}, ${humanFileSize(PDF.length)}`);
  expect(await download(page)).toEqual([NAME, PDF]);
  // Followed by the browser itself (a press with a modifier), it is an attachment never sniffed.
  const href = (await block.getByRole('link').getAttribute('href')) ?? '';
  const served = await page.request.get(href);
  expect(served.status()).toBe(200);
  expect(served.headers()['content-disposition']).toMatch(/^attachment;/);
  expect(served.headers()['x-content-type-options']).toBe('nosniff');
  expect(Buffer.from(await served.body()).equals(PDF)).toBe(true);

  // Pictures of the block: wide, and 390 pixels across on one line.
  await page.screenshot({ path: path.join(SHOTS, 'file-block-wide.png') });
  await page.setViewportSize({ width: 390, height: 844 });
  // Narrowed, the sidebar that was open is a drawer over the page: closed, as a phone shows it.
  await hideSidebar(page);
  // A drawer slides out: the picture is taken once it is out of sight.
  await expect
    .poll(async () => {
      const drawer = await sidebar(page).boundingBox();
      return drawer === null || drawer.x + drawer.width <= 0;
    })
    .toBe(true);
  await expect(block).toBeVisible();
  // One line: the name is cut short, the size and the button stay beside it.
  const box = await block.boundingBox();
  const button = await block.getByRole('link').boundingBox();
  expect(box?.height ?? 0).toBeLessThan(60);
  expect((button?.x ?? 0) + (button?.width ?? 0)).toBeLessThanOrEqual(390);
  expect(Math.abs((button?.y ?? 0) - (box?.y ?? 0))).toBeLessThan(box?.height ?? 0);
  await page.screenshot({ path: path.join(SHOTS, 'file-block-390.png') });

  // Another editor of the wiki sees the block and downloads the same bytes.
  const other = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const otherErrors = watchErrors(other);
  await openPage(other, at, colleague);
  await expect(other.getByTestId('page-body').locator('[data-file-block]')).toHaveAttribute(
    'data-file-id',
    fileId ?? '',
  );
  expect(await download(other)).toEqual([NAME, PDF]);
  // Reached by the keyboard: selected with an arrow from the line above it, viewed with Enter.
  await other.getByTestId('page-body').getByText('Agenda and minutes:').click();
  await other.keyboard.press('End');
  await other.keyboard.press('ArrowDown');
  const selected = other.locator('[data-file-block].ProseMirror-selectednode');
  await expect(selected).toHaveCount(1);
  await other.keyboard.press('Enter');
  await expect(other.getByTestId('file-viewer')).toHaveAttribute('data-status', 'shown');
  await other.keyboard.press('Escape');
  await expect(other.getByTestId('file-viewer')).toHaveCount(0);
  // Back in the editor, still selected and still there.
  await expect(selected).toHaveCount(1);
  await other.context().close();

  // Somebody who may only look sees the body only shown, and downloads the same bytes.
  const look = await (await browser.newContext({ locale: 'en-US' })).newPage();
  const lookErrors = watchErrors(look);
  await openPage(look, at, onlooker);
  await expect(look.getByTestId('page-body').locator('[contenteditable="true"]')).toHaveCount(0);
  expect(await download(look)).toEqual([NAME, PDF]);
  await view(look, 'keyboard');
  await look.context().close();

  // A copy of the wiki has a file of its own, with the same bytes, that its block downloads.
  const made = await answered<Schemas['ContainersAndBlocks']>(
    await api.post(`/api/v1/w/${ws}/containers/${wiki.id}/duplicate`, { data: {} }),
  );
  const copy = made.containers[0]?.id ?? '';
  const copyPages = await answered<Schemas['PageNode'][]>(
    await api.get(`/api/v1/w/${ws}/wikis/${copy}/pages`),
  );
  const copyPage = copyPages.find((node) => node.title === 'Minutes')?.id ?? '';
  const [copied] = fileBlocksIn(await stored(copyPage, copy));
  expect(copied).toBeDefined();
  expect(copied).not.toBe(fileId);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`/w/${ws}/wikis/${copy}/${copyPage}`);
  expect(await download(page)).toEqual([NAME, PDF]);

  // The reader by the wiki's public link downloads it, and the file of the copy is not there.
  const shared = await answered<Schemas['PublicLink']>(
    await api.post(`/api/v1/w/${ws}/containers/${wiki.id}/public-link`, {
      data: { enabled: true },
    }),
  );
  const token = shared.token ?? '';
  const anonymous = await browser.newContext({ locale: 'en-US', baseURL: base });
  const reader = await anonymous.newPage();
  const readerErrors = watchErrors(reader);
  await reader.goto(`/shared/${wiki.id}/${minutes.id}?r=${encodeURIComponent(token)}`);
  await expect(reader.getByTestId('page-body').locator('[data-file-block]')).toBeVisible();
  expect(await download(reader)).toEqual([NAME, PDF]);
  // Nobody is named to the reader: not in the viewer, nor in anything sent to show it.
  const seen = await view(reader);
  expect(seen.length).toBeGreaterThan(1);
  for (const id of people) {
    for (const text of seen) expect(text).not.toContain(id);
  }
  const elsewhere = await anonymous.request.get(
    `/api/v1/shared/${wiki.id}/files/${copied ?? ''}?readToken=${encodeURIComponent(token)}`,
  );
  expect(elsewhere.status()).toBe(404);
  await reader.screenshot({ path: path.join(SHOTS, 'file-block-shared.png') });
  await anonymous.close();
  await api.dispose();
  expect([...errors, ...otherErrors, ...lookErrors, ...readerErrors]).toEqual([]);
});
