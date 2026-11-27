/**
 * The versions of a card's body on the real server: two people in the
 * card's room, one of them goes back to a version and both have its text,
 * what was typed stays a version to go back to, and undo in the editor
 * does not take a restore back; a viewer reads the versions and restores
 * none; a reader by the public link is offered none. ⌘S or Ctrl+S keeps a
 * version now, of a card or a wiki's page, with what was typed just before,
 * the other in the room going on as they were; the browser saves no page.
 */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

import { focusAtEnd } from '../e2e/embeds.support.ts';
import { primaryKey } from '../e2e/keyboard.support.ts';

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
import {
  cardAt,
  editor,
  kept,
  openVersions,
  pageOf,
  paragraph,
  rows,
  shown,
  versions,
  writeWhole,
} from './versions.support.ts';

test.use({ locale: 'en-US' });

/**
 * Types at the end of the body, once the room's document is all there: the
 * caret is put through the editor, which a click and a key can race while
 * the document is still being received. The editor takes the focus a frame
 * later, and a key before that goes nowhere.
 */
async function typeAtEnd(page: Page, text: string, newLine = false): Promise<void> {
  await focusAtEnd(page.getByTestId('card-body'));
  await expect(editor(page)).toBeFocused();
  if (newLine) await page.keyboard.press('Enter');
  await page.keyboard.type(text);
}

const keptSaid = (page: Page, body = 'card-body') =>
  page.getByTestId(body).getByTestId('body-kept');

/** The text of each paragraph of a body as stored. */
function paragraphs(body: unknown): string[] {
  const doc = body as { content?: { content?: { text?: string }[] }[] } | null;
  return (doc?.content ?? []).map((block) =>
    (block.content ?? []).map((inline) => inline.text ?? '').join(''),
  );
}

async function keptBody(api: APIRequestContext, at: string, version: number): Promise<string[]> {
  const answer = await api.get(`${at}/body-versions/${String(version)}`);
  expect(answer.ok()).toBe(true);
  return paragraphs(((await answer.json()) as Schemas['BodyVersionContent']).body);
}

/** Puts down, for each press of S with Ctrl or ⌘ from here on, whether the page kept the browser from saving it. */
async function watchSaveKeys(page: Page): Promise<() => Promise<boolean[]>> {
  await page.evaluate(() => {
    const seen: boolean[] = [];
    (window as unknown as { saveKeys: boolean[] }).saveKeys = seen;
    window.addEventListener('keydown', (event) => {
      if ((event.ctrlKey || event.metaKey) && event.code === 'KeyS') {
        seen.push(event.defaultPrevented);
      }
    });
  });
  return () => page.evaluate(() => (window as unknown as { saveKeys: boolean[] }).saveKeys);
}

/** Goes back to the version shown, after the question. */
async function restoreShown(page: Page): Promise<void> {
  await page.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(versions(page)).toHaveCount(0);
}

test('a version restored is the body of everyone in the room, and what was typed stays a version', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('ada');
  const colleague = person('bora');
  await register(base, owner);
  await register(base, colleague);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Versions',
    cards: [{ title: 'Plan the launch', status: 'To do' }],
    members: [{ who: colleague, role: 'editor' }],
  });
  const cardId = board.cards['Plan the launch'] ?? '';
  // A card with a body: no body is no first version.
  await writeWhole(api, board, cardId, 'The first plan');
  const path = boardPath(board, board.viewId, cardId);
  const first = await pageOf(browser, path, owner);
  const second = await pageOf(browser, path, colleague);
  // Read at the end: errors that come later are in them too.
  const watched = [watchErrors(first), watchErrors(second)];
  for (const page of [first, second]) {
    await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
    await expect(editor(page).locator('p')).toHaveText(['The first plan']);
  }

  await typeAtEnd(second, 'Bora adds a line', true);
  await expect(editor(first).locator('p')).toHaveText(['The first plan', 'Bora adds a line']);
  // The owner writes too: each has something of their own to undo.
  await typeAtEnd(first, ', Ada another');
  const typed = ['The first plan', 'Bora adds a line, Ada another'];
  for (const page of [first, second]) await expect(editor(page).locator('p')).toHaveText(typed);

  // The owner goes back to the body from before.
  await openVersions(first);
  await expect(rows(first)).toHaveCount(1);
  await expect(rows(first).first().getByTestId('body-version-editors')).toContainText(
    owner.username,
  );
  await rows(first).first().click();
  await expect(shown(first)).toContainText('The first plan');
  await expect(shown(first)).not.toContainText('Bora adds a line');
  await expect(shown(first)).not.toContainText('Ada another');
  await restoreShown(first);
  for (const page of [first, second]) {
    await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
    await expect(editor(page).locator('p')).toHaveText(['The first plan']);
    // Nothing of theirs was waiting: nothing is said lost.
    await expect(page.getByTestId('body-lost')).toHaveCount(0);
  }
  const restored = (await (await api.get(cardAt(board, cardId))).json()) as Schemas['Card'];
  expect(restored.body).toEqual(paragraph('The first plan'));

  // The restore is no step of the editor's undo, for the one who restored or the other:
  // undoing brings back nothing either of them typed.
  for (const page of [first, second]) {
    await editor(page).click();
    await page.keyboard.press(await primaryKey(page, 'z'));
    await expect(editor(page).locator('p')).toHaveText(['The first plan']);
  }

  // What was typed is a version of its writer, and the body restored the newest, of who restored it.
  const list = await kept(api, board, cardId);
  expect(list.map((one) => [one.version, one.isCurrent])).toEqual([
    [3, true],
    [2, false],
    [1, false],
  ]);
  await openVersions(second);
  await expect(rows(second)).toHaveCount(3);
  await expect(rows(second).first().getByTestId('body-version-current')).toBeVisible();
  await expect(rows(second).first().getByTestId('body-version-editors')).toContainText(
    owner.username,
  );
  for (const who of [owner, colleague]) {
    await expect(rows(second).nth(1).getByTestId('body-version-editors')).toContainText(
      who.username,
    );
  }

  // The colleague goes back to what they typed.
  await rows(second).nth(1).click();
  await expect(shown(second)).toContainText('Bora adds a line, Ada another');
  await restoreShown(second);
  for (const page of [first, second]) await expect(editor(page).locator('p')).toHaveText(typed);
  // Both go on writing in it.
  await typeAtEnd(first, ', and both agree');
  await expect(editor(second).locator('p')).toHaveText([
    'The first plan',
    'Bora adds a line, Ada another, and both agree',
  ]);
  for (const page of [first, second]) await expect(page.locator('[data-problem]')).toHaveCount(0);
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('a viewer reads the versions and restores none; a reader by the public link is offered none', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('keeper');
  const reader = person('looker');
  await register(base, owner);
  await register(base, reader);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Versions to read',
    cards: [{ title: 'Minutes', status: 'To do' }],
    members: [{ who: reader, role: 'viewer' }],
  });
  const cardId = board.cards.Minutes ?? '';
  await writeWhole(api, board, cardId, 'Decided: ship on Monday');
  await writeWhole(api, board, cardId, 'Decided: ship on Friday');

  const looker = await pageOf(browser, boardPath(board, board.viewId, cardId), reader);
  const errors = watchErrors(looker);
  await openVersions(looker);
  await expect(rows(looker)).toHaveCount(2);
  await rows(looker).nth(1).click();
  await expect(shown(looker)).toContainText('Decided: ship on Monday');
  await expect(looker.getByTestId('body-version-restore')).toHaveCount(0);
  // The server refuses a viewer too.
  const asReader = await signedIn(base, reader);
  const refused = await asReader.post(`${cardAt(board, cardId)}/body-versions/1/restore`);
  expect(refused.status()).toBe(403);
  // Nor keep one now.
  expect((await asReader.post(`${cardAt(board, cardId)}/body-versions`)).status()).toBe(403);
  expect((await kept(asReader, board, cardId)).map((one) => one.version)).toEqual([2, 1]);
  await asReader.dispose();
  await looker.context().close();

  // By the board's public link: the card is read, and its menu has no versions.
  const at = `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`;
  const shared = (await (
    await api.post(`${at}/public-link`, { data: { enabled: true } })
  ).json()) as Schemas['PublicLink'];
  const context = await browser.newContext({ locale: 'en-US', baseURL: base });
  const page = await context.newPage();
  const sharedErrors = watchErrors(page);
  await page.goto(
    `/shared/${board.boardId}/${board.viewId}/${cardId}?r=${encodeURIComponent(shared.token ?? '')}`,
  );
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog.getByText('Decided: ship on Friday')).toBeVisible();
  // The keys in the body read by the link keep nothing, and the browser saves no page.
  const saveKeys = await watchSaveKeys(page);
  await dialog.getByText('Decided: ship on Friday').click();
  await page.keyboard.press(await primaryKey(page, 's'));
  expect(await saveKeys()).toEqual([true]);
  await expect(keptSaid(page)).toHaveText('');
  await page.getByTestId('card-menu').click();
  await expect(page.locator('[data-action="copy-link"]')).toBeVisible();
  await expect(page.locator('[data-action="versions"]')).toHaveCount(0);
  // Nor does the server give them to who is signed in as nobody.
  const anonymous = await context.request.get(`${cardAt(board, cardId)}/body-versions`);
  expect(anonymous.status()).toBe(401);
  await context.close();
  await api.dispose();
  expect([...errors, ...sharedErrors]).toEqual([]);
});

test('the keys keep what was typed just before as a version, and the other in the room goes on as they were', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('kept-ada');
  const colleague = person('kept-bora');
  await register(base, owner);
  await register(base, colleague);
  const api = await signedIn(base, owner);
  const board = await makeBoard(api, {
    title: 'Versions by hand',
    cards: [{ title: 'Plan the launch', status: 'To do' }],
    members: [{ who: colleague, role: 'editor' }],
  });
  const cardId = board.cards['Plan the launch'] ?? '';
  await writeWhole(api, board, cardId, 'The first plan');
  const path = boardPath(board, board.viewId, cardId);
  const first = await pageOf(browser, path, owner);
  const second = await pageOf(browser, path, colleague);
  const watched = [watchErrors(first), watchErrors(second)];
  for (const page of [first, second]) {
    await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  }

  await typeAtEnd(first, ', Ada too');
  // In the room before the other types: on WebKit the two otherwise interleave.
  await expect(editor(second).locator('p')).toHaveText(['The first plan, Ada too']);
  await typeAtEnd(second, 'Bora adds a line', true);
  const both = ['The first plan, Ada too', 'Bora adds a line'];
  await expect(editor(first).locator('p')).toHaveText(both);
  // The owner's caret is where they typed; what is typed just before the keys is kept.
  const saveKeys = await watchSaveKeys(first);
  await editor(first).locator('p').first().click();
  await first.keyboard.press('End');
  await first.keyboard.type(' now');
  await first.keyboard.press(await primaryKey(first, 's'));
  await expect(keptSaid(first)).toHaveText('Version kept.');
  const list = await kept(api, board, cardId);
  expect(list.map((one) => one.cause)).toEqual(['manual', 'auto']);
  const [newest] = list;
  expect(await keptBody(api, cardAt(board, cardId), newest?.version ?? 0)).toEqual([
    'The first plan, Ada too now',
    'Bora adds a line',
  ]);

  // The colleague's editor is as it was: still theirs to type in where they were.
  await expect(editor(second)).toBeFocused();
  await expect(second.getByTestId('body-lost')).toHaveCount(0);
  await second.keyboard.type(' and more');
  const after = ['The first plan, Ada too now', 'Bora adds a line and more'];
  for (const page of [first, second]) await expect(editor(page).locator('p')).toHaveText(after);
  await expect(keptSaid(second)).toHaveText('');

  // Again within a minute of it: none is kept, and that is said.
  await editor(first).locator('p').first().click();
  await first.keyboard.press(await primaryKey(first, 's'));
  await expect(keptSaid(first)).toContainText('a moment ago');
  expect(await saveKeys()).toEqual([true, true]);
  expect(await kept(api, board, cardId)).toHaveLength(2);

  // The dialog says which was kept by hand.
  await openVersions(second);
  await expect(rows(second)).toHaveCount(2);
  await expect(rows(second).first().getByTestId('body-version-cause')).toHaveText('Kept by hand');
  await expect(rows(second).nth(1).getByTestId('body-version-cause')).toHaveText(
    'Kept automatically',
  );
  await first.context().close();
  await second.context().close();
  await api.dispose();
  expect(watched.flat()).toEqual([]);
});

test('the keys keep the body of a wiki page as a version, and not again unchanged', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('kept-page');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Roadmap' });
  const inWorkspace = `/api/v1/w/${workspaceId}`;
  const wiki = (await (
    await api.post(`${inWorkspace}/wikis`, { data: { title: 'Handbook', visibility: 'private' } })
  ).json()) as Schemas['Container'];
  const pages = `${inWorkspace}/wikis/${wiki.id}/pages`;
  const made = (await (
    await api.post(pages, { data: { title: 'Notes' } })
  ).json()) as Schemas['Page'];
  const at = `${pages}/${made.id}`;
  const page = await (await browser.newContext({ locale: 'en-US' })).newPage();
  await page.goto(`/w/${workspaceId}/wikis/${wiki.id}/${made.id}`);
  await signInOnPage(page, owner);
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  const errors = watchErrors(page);
  const saveKeys = await watchSaveKeys(page);
  await focusAtEnd(page.getByTestId('page-body'));
  await expect(page.getByTestId('page-body').locator('[contenteditable]').first()).toBeFocused();
  await page.keyboard.type('Written on the page');
  await page.keyboard.press(await primaryKey(page, 's'));
  await expect(keptSaid(page, 'page-body')).toHaveText('Version kept.');
  // A page that had no body has no version before: the one kept by hand alone.
  const [newest, ...older] = (await (
    await api.get(`${at}/body-versions`)
  ).json()) as Schemas['BodyVersion'][];
  expect(newest?.cause).toBe('manual');
  expect(older).toEqual([]);
  expect(await keptBody(api, at, newest?.version ?? 0)).toEqual(['Written on the page']);
  await page.keyboard.press(await primaryKey(page, 's'));
  await expect(keptSaid(page, 'page-body')).toHaveText(
    'Nothing has changed since the newest version.',
  );
  expect(await saveKeys()).toEqual([true, true]);
  await page.getByTestId('page-menu').click();
  await page.locator('[data-action="versions"]').click();
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first().getByTestId('body-version-cause')).toHaveText('Kept by hand');
  await page.context().close();
  await api.dispose();
  expect(errors).toEqual([]);
});
