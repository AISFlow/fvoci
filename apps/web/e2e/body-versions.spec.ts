/**
 * The versions of a card's body, end to end, on the in-memory backend, in
 * the three stages of the page: listed from the card's menu, read, and one
 * restored for everyone; what who only reads is offered; and what is said
 * when a version cannot be restored. ⌘S or Ctrl+S keeps one now, in the
 * body of a card and of a wiki's page and in the dialog, and the browser's
 * dialog to save the page never opens, also where none is kept.
 *
 * `FVOCI_SCREENS=1` also takes the pictures of the dialog, wide and 390 px
 * wide, into `target/screens/versions/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { focusAtEnd } from './embeds.support.ts';
import { primaryKey } from './keyboard.support.ts';
import { screensDir } from './screens.ts';
import { openApp } from './shell.support.ts';
import {
  closeVersions,
  editor,
  narrow,
  openTaskCard,
  openVersions,
  rows,
  shown,
  versions,
  writtenWholeElsewhere,
  type Dev,
} from './versions.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

test('the versions of a body are listed and read, and one is restored: not an undo step of the editor', async ({
  page,
}) => {
  await openTaskCard(page);
  const before = await editor(page).locator('> *').allTextContents();

  // A body nobody changed yet has no version.
  await openVersions(page);
  await expect(page.getByTestId('body-versions-empty')).toBeVisible();
  await closeVersions(page);

  await focusAtEnd(page.getByTestId('card-body'));
  // The editor takes the focus a frame later, and a key before that goes nowhere.
  await expect(editor(page)).toBeFocused();
  await page.keyboard.press('Enter');
  await page.keyboard.type('Typed now');
  await expect(editor(page)).toContainText('Typed now');

  // The body before the reader began: by nobody known.
  await openVersions(page);
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first().getByTestId('body-version-editors')).toHaveCount(0);
  await rows(page).first().click();
  await expect(shown(page)).toContainText(before[0] ?? '');
  await expect(shown(page)).not.toContainText('Typed now');
  if (narrow(page)) {
    // The list gave way: the focus is on the way back, and comes back to the version read.
    await expect(rows(page).first()).toBeHidden();
    await expect(page.getByTestId('body-versions-back')).toBeFocused();
    await page.getByTestId('body-versions-back').click();
    await expect(rows(page).first()).toBeFocused();
    // The back button goes back to the list too, and leaves the dialog open.
    await rows(page).first().click();
    await expect(shown(page)).toBeVisible();
    await page.goBack();
    await expect(rows(page).first()).toBeVisible();
    await expect(versions(page)).toBeVisible();
    await rows(page).first().click();
  } else {
    await expect(rows(page).first()).toHaveAttribute('aria-current', 'true');
  }

  // Asked first; "Cancel" restores nothing.
  await page.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Cancel' }).click();
  await expect(versions(page)).toBeVisible();
  await page.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(versions(page)).toHaveCount(0);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  await expect(editor(page).locator('> *')).toHaveText(before);
  // What was typed had reached the room: nothing is said lost.
  await expect(page.getByTestId('body-lost')).toHaveCount(0);

  // The restore is no step of the editor's undo: undoing brings nothing typed back.
  await editor(page).click();
  const undo = await primaryKey(page, 'z');
  await page.keyboard.press(undo);
  await page.keyboard.press(undo);
  await expect(editor(page).locator('> *')).toHaveText(before);

  // What was typed is a version, and the body restored the newest: the card has it now.
  await openVersions(page);
  await expect(rows(page)).toHaveCount(3);
  const me = await page.evaluate(
    async () => (await (window as unknown as { fvociDev: Dev }).fvociDev.backend.me()).username,
  );
  await expect(rows(page).first().getByTestId('body-version-current')).toBeVisible();
  await expect(rows(page).first().getByTestId('body-version-editors')).toContainText(me);
  await rows(page).first().click();
  await expect(page.getByTestId('body-version-is-current')).toBeVisible();
  await expect(page.getByTestId('body-version-restore')).toHaveCount(0);
  if (narrow(page)) await page.getByTestId('body-versions-back').click();
  await rows(page).nth(1).click();
  await expect(shown(page)).toContainText('Typed now');
  await expect(page.getByTestId('body-version-restore')).toBeVisible();

  // And can be gone back to.
  await page.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(versions(page)).toHaveCount(0);
  await expect(editor(page)).toContainText('Typed now');
});

test('who only reads the board reads the versions and is not offered to restore; a refusal is said', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const before = (await editor(page).locator('> *').first().textContent()) ?? '';
  expect(before).not.toBe('');
  await writtenWholeElsewhere(page, place, 'Written whole by somebody else');
  await expect(editor(page)).toHaveText('Written whole by somebody else');

  await openVersions(page);
  await expect(rows(page)).toHaveCount(2);
  await rows(page).nth(1).click();
  await expect(page.getByTestId('body-version-restore')).toBeVisible();

  // The server refuses: the version cannot be stored any more, then is kept no more.
  for (const [status, refusal, said] of [
    [409, 'unrestorable', 'cannot be restored'],
    [404, 'gone', 'not kept any more'],
  ] as const) {
    await page.evaluate(async (refusedWith) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const thrown = await backend.card('nowhere', 'nothing').catch((error: unknown) => error);
      const BackendError = (
        thrown as { constructor: new (status: number, body: object) => unknown }
      ).constructor;
      backend.failNext('bodyVersions', new BackendError(refusedWith, { detail: 'refused' }));
    }, status);
    await page.getByTestId('body-version-restore').click();
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
    await expect(shown(page).locator(`[data-refusal="${refusal}"]`)).toContainText(said);
    await expect(versions(page)).toBeVisible();
  }
  await expect(editor(page)).toHaveText('Written whole by somebody else');

  // Made a viewer: the version stays to read, and nothing offers to restore it.
  await page.evaluate(async ({ boardId }) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    await dev.someoneElse().setMemberRole(boardId, (await dev.backend.me()).id, 'viewer');
  }, place);
  await expect(page.getByTestId('body-version-restore')).toHaveCount(0);
  await expect(shown(page)).toContainText(before);
  await closeVersions(page);
  await openVersions(page);
  await expect(rows(page)).toHaveCount(2);
  await rows(page).first().click();
  await expect(shown(page)).toContainText('Written whole by somebody else');
  await expect(page.getByTestId('body-version-restore')).toHaveCount(0);
  await expect(page.getByTestId('body-version-is-current')).toHaveCount(0);
});

/** The line under a body that says what came of the keys. */
const keptSaid = (page: Page, testId = 'card-body'): Locator =>
  page.getByTestId(testId).getByTestId('body-kept');
const cause = (row: Locator): Locator => row.getByTestId('body-version-cause');

/**
 * Puts down, for each press of S with Ctrl or ⌘ from here on, whether the
 * page kept the browser from doing its own with it (saving the page).
 */
async function watchSaveKeys(page: Page): Promise<() => Promise<boolean[]>> {
  await page.evaluate(() => {
    const seen: boolean[] = [];
    (window as unknown as { saveKeys: boolean[] }).saveKeys = seen;
    // On the window: after everything in the page had the key.
    window.addEventListener('keydown', (event) => {
      if ((event.ctrlKey || event.metaKey) && event.code === 'KeyS') {
        seen.push(event.defaultPrevented);
      }
    });
  });
  return () => page.evaluate(() => (window as unknown as { saveKeys: boolean[] }).saveKeys);
}

/** Writes `text` at the end of the card's body, on a line of its own. */
async function writeOnANewLine(page: Page, text: string): Promise<void> {
  await focusAtEnd(page.getByTestId('card-body'));
  await expect(editor(page)).toBeFocused();
  await page.keyboard.press('Enter');
  await page.keyboard.type(text);
  await expect(editor(page)).toContainText(text);
}

test.describe('a version kept by hand', () => {
  test('the keys keep the body of a card as a version now, and say what came of it', async ({
    page,
  }) => {
    await openTaskCard(page);
    const saveKeys = await watchSaveKeys(page);
    const save = await primaryKey(page, 's');
    await writeOnANewLine(page, 'Typed now');
    await page.keyboard.press(save);
    await expect(keptSaid(page)).toHaveText('Version kept.');
    // Pressed again on the same body: nothing is kept, and that is said.
    await page.keyboard.press(save);
    await expect(keptSaid(page)).toHaveText('Nothing has changed since the newest version.');
    // Within a minute of the one kept by hand, none is.
    await page.keyboard.type(' and more');
    await page.keyboard.press(save);
    await expect(keptSaid(page)).toContainText('a moment ago');
    expect(await saveKeys()).toEqual([true, true, true]);
    // The editor is as it was: the caret stays, and writing goes on there.
    await page.keyboard.type('!');
    await expect(editor(page)).toContainText('Typed now and more!');

    // The body before the reader began, kept on its own, and what was typed, by hand.
    await openVersions(page);
    await expect(rows(page)).toHaveCount(2);
    await expect(cause(rows(page).first())).toHaveText('Kept by hand');
    await expect(cause(rows(page).nth(1))).toHaveText('Kept automatically');
    await rows(page).first().click();
    await expect(shown(page)).toContainText('Typed now');
    await expect(shown(page)).not.toContainText('and more');
  });

  test('the dialog of the versions keeps one now, from its button and the keys, and lists it at once', async ({
    page,
  }) => {
    await openTaskCard(page);
    const saveKeys = await watchSaveKeys(page);
    await writeOnANewLine(page, 'Typed now');
    await openVersions(page);
    await expect(rows(page)).toHaveCount(1);
    const keep = versions(page).getByTestId('body-versions-keep');
    await expect(keep).toHaveAttribute('aria-keyshortcuts', /^(Meta|Control)\+S$/);
    await keep.click();
    await expect(rows(page)).toHaveCount(2);
    await expect(cause(rows(page).first())).toHaveText('Kept by hand');
    await expect(rows(page).first().getByTestId('body-version-current')).toBeVisible();
    const said = versions(page).getByTestId('body-versions-kept');
    await expect(said).toHaveText('Version kept.');
    await page.keyboard.press(await primaryKey(page, 's'));
    await expect(said).toHaveText('Nothing has changed since the newest version.');
    await expect(rows(page)).toHaveCount(2);
    expect(await saveKeys()).toEqual([true]);
  });

  test('narrow, a version kept while one is read leaves the history be: back still goes to the list', async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 800 });
    await openTaskCard(page);
    await writeOnANewLine(page, 'Typed now');
    await openVersions(page);
    await rows(page).first().click();
    await expect(page.getByTestId('body-versions-back')).toBeVisible();
    // Every step the page takes in the history from here on.
    await page.evaluate(() => {
      const steps: string[] = [];
      (window as unknown as { steps: string[] }).steps = steps;
      const push = history.pushState.bind(history);
      history.pushState = (...args) => {
        steps.push('push');
        push(...args);
      };
      const back = history.back.bind(history);
      history.back = () => {
        steps.push('back');
        back();
      };
    });
    await page.keyboard.press(await primaryKey(page, 's'));
    await expect(versions(page).getByTestId('body-versions-kept')).toHaveText('Version kept.');
    await expect(rows(page)).toHaveCount(2);
    expect(await page.evaluate(() => (window as unknown as { steps: string[] }).steps)).toEqual([]);
    await page.goBack();
    await expect(page.getByTestId('body-versions-back')).toHaveCount(0);
    await expect(rows(page).first()).toBeVisible();
    await expect(versions(page)).toBeVisible();
  });

  test('a syllable being composed as the keys are pressed is kept whole, and left as it was', async ({
    page,
    browserName,
  }) => {
    test.skip(browserName !== 'chromium', "the input method is driven through Chromium's own");
    await openTaskCard(page);
    const saveKeys = await watchSaveKeys(page);
    await writeOnANewLine(page, 'Typed ');
    const ime = await page.context().newCDPSession(page);
    await ime.send('Input.imeSetComposition', { text: '하', selectionStart: 1, selectionEnd: 1 });
    await page.keyboard.press(await primaryKey(page, 's'));
    await ime.send('Input.insertText', { text: '한' });
    await expect(keptSaid(page)).toHaveText('Version kept.');
    await expect(editor(page)).toContainText('Typed 한');
    await expect(editor(page)).not.toContainText('하한');
    expect(await saveKeys()).toEqual([true]);
    await openVersions(page);
    await rows(page).first().click();
    await expect(shown(page)).toContainText('Typed 한');
  });

  test('who only reads, and the fields of comments, keep no version, and the browser saves no page', async ({
    page,
  }) => {
    const place = await openTaskCard(page);
    const saveKeys = await watchSaveKeys(page);
    const save = await primaryKey(page, 's');
    await page.getByTestId('comment-field').fill('A comment');
    await page.getByTestId('comment-field').press(save);
    // Made a viewer by another admin, and selecting in the body to read it.
    await page.evaluate(async ({ boardId }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const other = dev.someoneElse();
      const otherId = (await other.me()).id;
      await dev.backend.addMember(boardId, otherId, 'admin');
      await dev.backend.setMemberRole(boardId, otherId, 'admin');
      await other.setMemberRole(boardId, (await dev.backend.me()).id, 'viewer');
    }, place);
    await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'reading');
    await editor(page).locator('> *').first().click();
    await page.keyboard.press(save);
    expect(await saveKeys()).toEqual([true, true]);
    await expect(keptSaid(page)).toHaveText('');
    await openVersions(page);
    await expect(page.getByTestId('body-versions-empty')).toBeVisible();
    await expect(versions(page).getByTestId('body-versions-keep')).toHaveCount(0);
  });

  test('anywhere else in an open card the keys save no page and keep nothing, saying nothing', async ({
    page,
  }) => {
    await openTaskCard(page);
    const saveKeys = await watchSaveKeys(page);
    const save = await primaryKey(page, 's');
    const title = page.getByTestId('card-heading').locator('textarea');
    await title.focus();
    await page.keyboard.press(save);
    // On the card itself, nothing in it focused nor selected.
    await page.getByTestId('card-dialog').focus();
    await page.evaluate(() => document.getSelection()?.removeAllRanges());
    await page.keyboard.press(save);
    expect(await saveKeys()).toEqual([true, true]);
    await expect(keptSaid(page)).toHaveText('');
    await expect(title).toBeVisible();
    await openVersions(page);
    await expect(page.getByTestId('body-versions-empty')).toBeVisible();
  });

  test('the keys keep the body of a wiki page as a version too', async ({ page }) => {
    await openApp(page);
    await newWiki(page, 'Handbook');
    await page.getByTestId('wiki-first-page').click();
    await nameThePage(page, 'Notes');
    const saveKeys = await watchSaveKeys(page);
    await focusAtEnd(page.getByTestId('page-body'));
    await expect(pageBody(page)).toBeFocused();
    await page.keyboard.type('Typed on the page');
    await page.keyboard.press(await primaryKey(page, 's'));
    await expect(keptSaid(page, 'page-body')).toHaveText('Version kept.');
    expect(await saveKeys()).toEqual([true]);
    await page.getByTestId('page-menu').click();
    await page.locator('[data-action="versions"]').click();
    // A page that had no body has no version before it: the one kept by hand alone.
    await expect(rows(page)).toHaveCount(1);
    await expect(cause(rows(page).first())).toHaveText('Kept by hand');
    await page.keyboard.press('Escape');
    await expect(versions(page)).toHaveCount(0);
    // The title of the page: the browser saves no page there either, and nothing is kept.
    await page.getByTestId('page-title').focus();
    await page.keyboard.press(await primaryKey(page, 's'));
    expect(await saveKeys()).toEqual([true, true]);
    await page.getByTestId('page-menu').click();
    await page.locator('[data-action="versions"]').click();
    await expect(rows(page)).toHaveCount(1);
  });
});

test.describe('pictures', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'set FVOCI_SCREENS=1 to take the pictures');
  // The language the browser asks for is the app's, until the reader chooses one.
  test.use({ locale: 'ko-KR' });

  const SCREENS = screensDir('versions');
  const SIZES = [
    { name: 'wide', width: 1280, height: 800 },
    { name: 'narrow', width: 390, height: 844 },
  ] as const;

  for (const size of SIZES) {
    for (const scheme of ['light', 'dark'] as const) {
      test(`what a comment changed, and the keys on a title, in Korean (${size.name}, ${scheme})`, async ({
        page,
      }, testInfo) => {
        test.skip(testInfo.project.name !== 'desktop', 'the sizes are set here');
        const into = screensDir('comments');
        await page.setViewportSize(size);
        await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
        await openTaskCard(page);
        const list = page.getByTestId('card-comments').locator('[data-comment]');
        const count = await list.count();
        await page.getByTestId('comment-field').fill('금요일까지 초안을 올립니다.');
        await page.getByTestId('comment-send').click();
        await expect(list).toHaveCount(count + 1);
        const mine = list.last();
        await mine.locator('[data-action="edit"]').click();
        const field = mine.getByRole('textbox');
        await field.fill('다음 주 월요일까지 초안을 올렸습니다.');
        await field.press(await primaryKey(page, 'Enter'));
        await mine.locator('[data-action="history"]').click();
        const now = mine.getByTestId('comment-history').locator('[data-version="now"]');
        await expect(now.locator('ins').first()).toBeVisible();
        await now.scrollIntoViewIfNeeded();
        await page.evaluate(() => document.fonts.ready);
        await page.screenshot({
          path: path.join(into, `comment-diff-${size.name}-${scheme}.png`),
        });
        const title = page.getByTestId('card-heading').locator('textarea');
        await title.focus();
        await title.scrollIntoViewIfNeeded();
        await page.keyboard.press(await primaryKey(page, 's'));
        await expect(title).toBeFocused();
        await page.screenshot({ path: path.join(into, `title-save-${size.name}-${scheme}.png`) });
      });
    }
  }

  for (const size of SIZES) {
    test(`the versions of a body, in Korean (${size.name})`, async ({ page }, testInfo) => {
      test.skip(testInfo.project.name !== 'desktop', 'the sizes are set here');
      const shot = async (name: string): Promise<void> => {
        await page.screenshot({ path: path.join(SCREENS, `${name}-${size.name}.png`) });
      };
      await page.setViewportSize(size);
      // Nothing fades or slides: a picture is not taken halfway through.
      await page.emulateMedia({ reducedMotion: 'reduce' });
      const place = await openTaskCard(page);
      await page.getByTestId('card-menu').click();
      await expect(page.locator('[data-action="versions"]')).toBeVisible();
      await shot('menu');
      await page.keyboard.press('Escape');

      await writtenWholeElsewhere(page, place, '다음 주 화요일까지 초안을 올립니다.');
      await focusAtEnd(page.getByTestId('card-body'));
      await expect(editor(page)).toBeFocused();
      await page.keyboard.type(' 검토는 목요일에 합니다.');
      // Taken by the room before the next body is written whole over it.
      await expect(page.getByTestId('card-save-state')).toHaveAttribute('data-state', 'saved');
      await writtenWholeElsewhere(page, place, '일정이 바뀌었습니다. 초안은 금요일에 올립니다.');
      await openVersions(page);
      await expect(rows(page)).toHaveCount(4);
      await page.evaluate(() => document.fonts.ready);
      await shot('list');
      await rows(page).nth(1).click();
      await expect(shown(page)).toContainText('검토는 목요일에 합니다.');
      await shot('version');
      await page.getByTestId('body-version-restore').click();
      await expect(page.getByTestId('confirm-dialog')).toBeVisible();
      await shot('restore');
    });
  }
});
