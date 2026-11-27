/**
 * The outline of a wiki's page on the real server: it follows the headings
 * another browser writes, writes nothing into the body itself, is the same
 * for whoever may only read the page and for a reader by the public link
 * (beside a page that gives up some of its width for it where it must;
 * none for a page with fewer than two headings), and stays folded away
 * after a reload once the reader folded it. Pictures with `FVOCI_SCREENS=1`
 * go to `target/wiki-toc/screens/`.
 */
import path from 'node:path';

import { expect, test, type Browser, type Page } from '@playwright/test';

import {
  bodyHeading,
  currentItem,
  outline,
  outlineBeside,
  outlineItems,
  SECTION_WORDS,
} from '../e2e/outline.support.ts';
import { pageBody } from '../e2e/wiki.support.ts';

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

const SCREENS = path.resolve(import.meta.dirname, '../../../target/wiki-toc/screens');
const WIDE = { width: 1600, height: 900 };

/** A body of sections, each a heading of the level given and a paragraph. */
function sections(...headings: [number, string][]): object {
  return {
    type: 'doc',
    content: headings.flatMap(([level, title]) => [
      { type: 'heading', attrs: { level }, content: [{ type: 'text', text: title }] },
      { type: 'paragraph', content: [{ type: 'text', text: SECTION_WORDS }] },
    ]),
  };
}

const TITLES = Array.from({ length: 8 }, (_, at) => `Part ${at + 1}`);
const LEVELS = [1, 2, 3, 2, 2, 3, 3, 1];
const GUIDE = sections(...TITLES.map((title, at): [number, string] => [LEVELS[at] ?? 2, title]));

async function opened(
  browser: Browser,
  path: string,
  who?: Parameters<typeof signInOnPage>[1],
  before?: (page: Page) => Promise<void>,
) {
  const page = await (
    await browser.newContext({ locale: 'en-US', viewport: WIDE, reducedMotion: 'reduce' })
  ).newPage();
  await before?.(page);
  await page.goto(path);
  if (who) await signInOnPage(page, who);
  return page;
}

/**
 * Whether a binary frame of a body's room is a change of its Y document: a sync step 2
 * or an update, not a step 1 or a cursor (`contracts/websocket.md` 6, "Binary 프레임").
 */
function isChange(frame: Buffer): boolean {
  let at = 0;
  const next = (): number => {
    let value = 0;
    for (let shift = 0; ; shift += 7) {
      const byte = frame[at] ?? 0;
      at += 1;
      value += (byte & 0x7f) * 2 ** shift;
      if (byte < 0x80) return value;
    }
  };
  next(); // the room
  if (next() !== 0) return false; // a cursor
  next(); // the generation
  return next() !== 0;
}

/** Counts the changes of a body's document that `page` sends to the server. */
async function countChanges(page: Page): Promise<{ sent: number }> {
  const changes = { sent: 0 };
  await page.routeWebSocket(/\/ws$/, (socket) => {
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      if (typeof message !== 'string' && isChange(message)) changes.sent += 1;
      server.send(message);
    });
  });
  return changes;
}

const item = (page: Page, words: string) =>
  outline(page).getByRole('button', { name: words, exact: true });

/** Takes a picture of the page as it is, light and dark, when pictures are asked for. */
async function pictured(page: Page, name: string): Promise<void> {
  if (!process.env.FVOCI_SCREENS) return;
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: `${SCREENS}/${name}-${scheme}.png`, animations: 'disabled' });
  }
  await page.emulateMedia({ colorScheme: 'light' });
}

test('the outline follows the headings another browser writes, writes nothing itself, is a viewer’s too, and stays folded after a reload', async ({
  browser,
  baseURL,
}) => {
  test.setTimeout(180_000);
  const base = baseURL ?? '';
  const [owner, colleague, viewer] = [person('toc-ada'), person('toc-bora'), person('toc-chul')];
  for (const who of [owner, colleague, viewer]) await register(base, who);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Roadmap' });
  const at = `/api/v1/w/${workspaceId}`;
  const wiki = (await (
    await api.post(`${at}/wikis`, { data: { title: 'Handbook', visibility: 'private' } })
  ).json()) as Schemas['Container'];
  for (const [who, role] of [
    [colleague, 'editor'],
    [viewer, 'viewer'],
  ] as const) {
    const [found] = (await (
      await api.get(`${at}/users`, { params: { search: who.username } })
    ).json()) as Schemas['User'][];
    await api.post(`${at}/containers/${wiki.id}/members`, {
      data: { userId: found?.id, role },
    });
  }
  const made = (await (
    await api.post(`${at}/wikis/${wiki.id}/pages`, { data: { title: 'Guide' } })
  ).json()) as Schemas['Page'];
  const pageAt = `${at}/wikis/${wiki.id}/pages/${made.id}`;
  expect((await api.patch(pageAt, { data: { readAt: made.updatedAt, body: GUIDE } })).ok()).toBe(
    true,
  );
  const stored = async (): Promise<string> =>
    JSON.stringify(((await (await api.get(pageAt)).json()) as Schemas['Page']).body);
  const address = `/w/${workspaceId}/wikis/${wiki.id}/${made.id}`;
  // The body as it is before anybody has it open, with an outline or without.
  const before = await stored();

  let changes = { sent: 0 };
  const [first, second] = await Promise.all([
    opened(browser, address, owner, async (page) => {
      changes = await countChanges(page);
    }),
    opened(browser, address, colleague),
  ]);
  const watched = [watchErrors(first), watchErrors(second)];
  for (const page of [first, second]) {
    await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
    await expect(outlineItems(page)).toHaveText(TITLES);
  }
  expect(await stored()).toBe(before);

  // The first goes about the page by its outline; the second then writes one letter.
  for (const words of ['Part 6', 'Part 8', 'Part 2']) {
    await item(first, words).click();
    await expect(currentItem(first)).toHaveText(words);
    await expect(bodyHeading(first, words)).toBeInViewport();
  }
  await second.getByTestId('page-body').getByText(SECTION_WORDS).last().click();
  // The end of the body: the end of its last paragraph.
  await second.keyboard.press('ControlOrMeta+End');
  await second.keyboard.type('Z');
  const expected = JSON.parse(before) as { content: { content?: { text: string }[] }[] };
  const last = expected.content.at(-1)?.content?.[0];
  if (last) last.text += 'Z';
  await expect.poll(stored).toContain(`${SECTION_WORDS}Z`);
  // The body is what it was and that letter: the outline put nothing in it, neither in
  // the JSON nor in the Y document (the first's browser has sent no change of it).
  expect(await stored()).toBe(JSON.stringify(expected));
  expect(changes.sent).toBe(0);

  // A heading the second writes is in the first's outline, and goes from it with its words.
  await second.keyboard.press('Enter');
  await second.keyboard.type('## Part 9');
  await expect(outlineItems(first)).toHaveText([...TITLES, 'Part 9']);
  for (let left = 'Part 9'.length; left > 0; left -= 1) await second.keyboard.press('Backspace');
  await expect(outlineItems(first)).toHaveText(TITLES);
  await pictured(first, 'editing-wide');

  // Whoever may only read the page has the same outline, and is taken to a heading chosen.
  const reading = await opened(browser, address, viewer);
  await expect(reading.getByTestId('page-body')).toHaveAttribute('data-editor', 'reading');
  await expect(outlineItems(reading)).toHaveText(TITLES);
  await item(reading, 'Part 7').click();
  await expect(bodyHeading(reading, 'Part 7')).toBeInViewport();
  await expect
    .poll(() => reading.evaluate(() => document.activeElement?.hasAttribute('data-body-editor')))
    .toBe(true);
  await pictured(reading, 'read-only-wide');
  await reading.context().close();

  // The count of changes counts: a letter the first writes is one.
  await first.getByTestId('page-body').getByText(SECTION_WORDS).first().click();
  await first.keyboard.type('Y');
  await expect.poll(() => changes.sent).toBeGreaterThan(0);

  // Folded away, it stays so after a reload, and the button puts it back.
  await first.getByTestId('fold-page-outline').click();
  await expect(outlineBeside(first)).toHaveCount(0);
  await first.reload();
  await expect(pageBody(first)).toBeVisible();
  await expect(
    first.getByRole('button', { name: 'Show the contents beside the page' }),
  ).toBeVisible();
  await expect(outlineBeside(first)).toHaveCount(0);
  await first.getByRole('button', { name: 'Show the contents beside the page' }).click();
  await expect(outlineBeside(first)).toBeVisible();

  for (const errors of watched) expect(errors).toEqual([]);
  await first.context().close();
  await second.context().close();
  await api.dispose();
});

test('a reader by the public link has the same outline, wide, beside a narrowed page and at 390 pixels; a page with one heading has none', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('toc-publisher');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Beside the wiki' });
  const at = `/api/v1/w/${workspaceId}`;
  const wiki = (await (
    await api.post(`${at}/wikis`, { data: { title: 'Field notes', visibility: 'private' } })
  ).json()) as Schemas['Container'];
  const pageWith = async (title: string, body: object): Promise<Schemas['Page']> => {
    const page = (await (
      await api.post(`${at}/wikis/${wiki.id}/pages`, { data: { title } })
    ).json()) as Schemas['Page'];
    const written = await api.patch(`${at}/wikis/${wiki.id}/pages/${page.id}`, {
      data: { readAt: page.updatedAt, body },
    });
    expect(written.ok()).toBe(true);
    return page;
  };
  const guide = await pageWith('Guide', GUIDE);
  await pageWith('Short', sections([2, 'Only one']));
  const shared = (await (
    await api.post(`${at}/containers/${wiki.id}/public-link`, { data: { enabled: true } })
  ).json()) as Schemas['PublicLink'];
  const link = `/shared/${wiki.id}/${guide.id}?r=${encodeURIComponent(shared.token ?? '')}`;

  const page = await opened(browser, link);
  const errors = watchErrors(page);
  const reader = page.getByTestId('shared-wiki');
  await expect(reader.getByTestId('page-title')).toHaveText('Guide');
  await expect(outlineBeside(page)).toBeVisible();
  await expect(outlineItems(page)).toHaveText(TITLES);
  await item(page, 'Part 5').click();
  await expect(bodyHeading(page, 'Part 5')).toBeInViewport();
  await expect(bodyHeading(page, 'Part 5')).toBeFocused();
  await expect(currentItem(page)).toHaveText('Part 5');
  await pictured(page, 'public-wide');

  // Narrower, beside the page tree: the page gives up some of its width (from 48rem, not
  // below 40rem) for the outline at its right, and a heading chosen there is gone to.
  await page.setViewportSize({ width: 1200, height: 900 });
  await expect(outlineBeside(page)).toBeVisible();
  await expect
    .poll(async () => (await reader.getByTestId('page-title').boundingBox())?.width ?? 0)
    .toBeLessThan(768);
  const [title, side] = await Promise.all([
    reader.getByTestId('page-title').boundingBox(),
    outlineBeside(page).boundingBox(),
  ]);
  expect(title?.width).toBeGreaterThanOrEqual(640);
  expect(side?.x).toBeGreaterThanOrEqual((title?.x ?? 0) + (title?.width ?? 0) - 1);
  await item(page, 'Part 2').click();
  await expect(bodyHeading(page, 'Part 2')).toBeInViewport();
  await expect(bodyHeading(page, 'Part 2')).toBeFocused();
  await expect(currentItem(page)).toHaveText('Part 2');
  await pictured(page, 'public-1200');

  // The page with one heading: no outline, and no empty box in its place.
  await reader.getByTestId('shared-page-tree').getByRole('link', { name: 'Short' }).click();
  await expect(reader.getByTestId('page-title')).toHaveText('Short');
  await expect(outline(page)).toHaveCount(0);
  await expect(page.getByTestId('open-page-outline')).toHaveCount(0);

  // At 390 pixels: a button beside the one of the pages, the list below it closing on a choice.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(link);
  await expect(reader.getByTestId('page-title')).toHaveText('Guide');
  const button = page.getByTestId('open-page-outline');
  await expect(button).toBeVisible();
  await expect(outlineBeside(page)).toHaveCount(0);
  await button.click();
  await expect(outlineItems(page)).toHaveText(TITLES);
  await pictured(page, 'public-390-open');
  await item(page, 'Part 6').click();
  await expect(outline(page)).toHaveCount(0);
  await expect(bodyHeading(page, 'Part 6')).toBeInViewport();
  await expect(bodyHeading(page, 'Part 6')).toBeFocused();
  await pictured(page, 'public-390');
  expect(errors).toEqual([]);
  await page.context().close();
  await api.dispose();
});
