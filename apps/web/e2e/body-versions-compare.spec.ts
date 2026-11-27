/**
 * A version of a body read as what changed, end to end on the in-memory
 * backend: from the version before it by default, from the body now or from
 * any other version as the reader chooses, or as it is; the choice of the
 * first two kept on the device, another version only while the dialog is
 * open; words added and removed, the way from change to change, and what is
 * the same folded away. For a card and for a wiki's page.
 *
 * `FVOCI_SCREENS=1` also takes the pictures of a comparison, wide and 390 px
 * wide, light and dark, into `target/save-version/screens/`.
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { focusAtEnd } from './embeds.support.ts';
import { primaryKey } from './keyboard.support.ts';
import { SCREENS_ROOT } from './screens.ts';
import { openApp } from './shell.support.ts';
import {
  closeVersions,
  editor,
  narrow,
  openTaskCard,
  openVersions,
  rows,
  shown,
  writtenWholeElsewhere,
} from './versions.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });
test.use({ locale: 'en-US' });

const diff = (page: Page): Locator => shown(page).getByTestId('body-diff');
const choice = (page: Page): Locator => shown(page).getByTestId('body-version-compare');
const whole = (page: Page): Locator => shown(page).getByTestId('body-version-whole');
const direction = (page: Page): Locator => shown(page).getByTestId('body-version-direction');

/** Reads the version of the list at `index` (the newest first), from the list when narrow. */
async function read(page: Page, index: number): Promise<void> {
  if (narrow(page) && (await page.getByTestId('body-versions-back').isVisible()))
    await page.getByTestId('body-versions-back').click();
  await rows(page).nth(index).click();
}

test('a version shows what it changed from the one before, from the body now, or from another, or as it is', async ({
  page,
}) => {
  const workers: string[] = [];
  page.on('worker', (worker) => workers.push(worker.url()));
  const place = await openTaskCard(page);
  for (const text of ['alpha beta gamma', 'alpha delta gamma', 'alpha beta gamma']) {
    await writtenWholeElsewhere(page, place, text);
    await expect(editor(page)).toHaveText(text);
  }
  await openVersions(page);
  // The body before the first, and each written after it.
  await expect(rows(page)).toHaveCount(4);

  // The newest, from the one before it: a word removed and a word added.
  await read(page, 1);
  await expect(direction(page)).toContainText('from the previous version');
  await expect(diff(page).locator('del')).toHaveText(['beta']);
  await expect(diff(page).locator('ins')).toHaveText(['delta']);
  await expect(diff(page).getByTestId('body-diff-counts')).toHaveText('1 added · 1 removed');
  await expect(choice(page)).toHaveValue('previous');
  // Compared in a worker, which the page's content security policy lets start.
  expect(workers.some((url) => url.includes('diff-worker'))).toBe(true);

  // The three views, by the keys: the body now, as it is, and back.
  await choice(page).focus();
  await page.keyboard.press('ArrowDown');
  await expect(choice(page)).toHaveValue('current');
  await expect(direction(page)).toContainText('From the current body');
  // Going back to it would put "delta" back for "beta".
  await expect(diff(page).locator('del')).toHaveText(['beta']);
  await expect(diff(page).locator('ins')).toHaveText(['delta']);
  await page.keyboard.press('Tab');
  await expect(whole(page)).toBeFocused();
  await page.keyboard.press('Space');
  await expect(whole(page)).toHaveAttribute('aria-pressed', 'true');
  await expect(diff(page)).toHaveCount(0);
  await expect(shown(page).locator('.doc-body')).toHaveText('alpha delta gamma');
  await page.keyboard.press('Space');
  await expect(whole(page)).toHaveAttribute('aria-pressed', 'false');
  await expect(diff(page).locator('ins')).toHaveText(['delta']);

  // The newest is the body now: nothing has changed.
  await read(page, 0);
  await expect(choice(page)).toHaveValue('current');
  await expect(diff(page).getByTestId('body-diff-none')).toHaveText('Nothing has changed.');

  // Two versions of the same body: nothing has changed either.
  const second = await rows(page).nth(2).getAttribute('data-version');
  await choice(page).selectOption(second ?? '');
  await expect(direction(page)).toContainText('from an older version');
  await expect(diff(page).getByTestId('body-diff-none')).toBeVisible();

  // Another version stays the one compared with, the way round said: from the older to this one…
  await read(page, 1);
  await expect(choice(page)).toHaveValue(second ?? '');
  await expect(direction(page)).toContainText('from an older version');
  await expect(diff(page).locator('ins')).toHaveText(['delta']);
  // …and from the newer to the oldest, which then is compared too.
  await read(page, 3);
  await expect(choice(page)).toHaveValue(second ?? '');
  await expect(direction(page)).toContainText('From a newer version');
  await expect(diff(page).getByTestId('body-diff-counts')).toBeVisible();
  // Read itself, it gives way to the kind chosen for every version: the body now, still.
  await read(page, 2);
  await expect(choice(page)).toHaveValue('current');
  await expect(direction(page)).toContainText('From the current body');
  // The oldest has none before it: as it is.
  await read(page, 3);
  await choice(page).selectOption('previous');
  await expect(direction(page)).toContainText('oldest version');
  await expect(diff(page)).toHaveCount(0);

  // Another version is let go when the dialog closes; the kind stays on the device.
  await choice(page).selectOption(second ?? '');
  await closeVersions(page);
  await openVersions(page);
  await read(page, 1);
  await expect(choice(page)).toHaveValue('previous');
  await choice(page).selectOption('current');
  await closeVersions(page);
  // A new reading of the versions takes it from the device (the backend in memory is not kept over a reload).
  expect(await page.evaluate(() => localStorage.getItem('fvoci.versions.compare'))).toBe('current');
  await openVersions(page);
  await read(page, 1);
  await expect(choice(page)).toHaveValue('current');
});

test('what is the same is folded, and the way through the changes goes by the buttons and the keys', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const lines = Array.from({ length: 8 }, (_, index) => `Line ${String(index + 1)} stays`);
  const body = (last: string, heading: number): object => ({
    type: 'doc',
    content: [
      { type: 'heading', attrs: { level: heading }, content: [{ type: 'text', text: 'Plan' }] },
      ...lines.map((text) => ({ type: 'paragraph', content: [{ type: 'text', text }] })),
      { type: 'paragraph', content: [{ type: 'text', text: last }] },
    ],
  });
  await writtenWholeElsewhere(page, place, body('The end is near', 2));
  await expect(editor(page)).toContainText('The end is near');
  await writtenWholeElsewhere(page, place, body('The end is here', 1));
  await expect(editor(page)).toContainText('The end is here');
  await openVersions(page);
  await read(page, 0);

  // The heading says what it was; the lines between the two changes are folded.
  await expect(diff(page).locator('.doc-diff-note')).toHaveText('Heading 2 → Heading 1');
  const fold = diff(page).getByTestId('body-diff-fold');
  await expect(fold).toHaveText('Show 6 unchanged blocks');
  await expect(diff(page)).not.toContainText('Line 4 stays');
  await fold.click();
  await expect(diff(page)).toContainText('Line 4 stays');

  // Next, next, and round to the first; Alt+↑ goes back from where the focus is.
  const stops = diff(page).locator('[data-diff-stop]');
  await expect(stops).toHaveCount(2);
  await diff(page).getByTestId('body-diff-next').click();
  await expect(stops.first()).toBeFocused();
  await page.keyboard.press('Alt+ArrowDown');
  await expect(stops.last()).toBeFocused();
  await expect(stops.last().locator('del')).toHaveText('near');
  await page.keyboard.press('Alt+ArrowDown');
  await expect(stops.first()).toBeFocused();
  await page.keyboard.press('Alt+ArrowUp');
  await expect(stops.last()).toBeFocused();
});

test('the lines of code are numbered as the version has them: a line removed counts none', async ({
  page,
}) => {
  const place = await openTaskCard(page);
  const code = (value: string): object => ({
    type: 'doc',
    content: [
      { type: 'codeBlock', attrs: { language: 'ts' }, content: [{ type: 'text', text: value }] },
    ],
  });
  await writtenWholeElsewhere(page, place, code('const a = 1;\nconst b = 2;\nconst c = 3;'));
  await expect(editor(page)).toContainText('const b = 2;');
  await writtenWholeElsewhere(page, place, code('const a = 1;\nconst c = 3;'));
  await expect(editor(page)).not.toContainText('const b = 2;');
  await page.evaluate(() => {
    document.documentElement.setAttribute('data-code-lines', '');
  });
  await openVersions(page);
  await read(page, 0);
  const numbers = diff(page).locator('.doc-code-line');
  await expect(numbers).toHaveCount(3);
  const counted = await numbers.evaluateAll((all) =>
    all.map((one) => getComputedStyle(one).counterIncrement),
  );
  expect(counted).toEqual(['doc-code-line 1', 'none', 'doc-code-line 1']);
  await expect(diff(page).locator('del .doc-code-line')).toHaveCount(1);
});

test('a version of a wiki page shows what it changed', async ({ page }) => {
  await openApp(page);
  await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Notes');
  await focusAtEnd(page.getByTestId('page-body'));
  await expect(pageBody(page)).toBeFocused();
  await page.keyboard.type('First words');
  await page.keyboard.press(await primaryKey(page, 's'));
  await expect(page.getByTestId('page-body').getByTestId('body-kept')).toHaveText('Version kept.');
  await page.keyboard.type(' and more');
  const openPageVersions = async (): Promise<void> => {
    await page.getByTestId('page-menu').click();
    await page.locator('[data-action="versions"]').click();
  };
  await openPageVersions();
  // Going back to the one kept keeps what was typed after it as a version.
  await read(page, 0);
  await page.getByTestId('body-version-restore').click();
  await page.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(pageBody(page)).toHaveText('First words');

  await openPageVersions();
  await expect(rows(page)).toHaveCount(3);
  // What was typed, from the version kept by hand before it.
  await read(page, 1);
  await expect(diff(page).locator('ins')).toHaveText([' and more']);
  await expect(diff(page).locator('del')).toHaveCount(0);
  // The version gone back to, from what was typed: the words removed.
  await read(page, 0);
  await expect(diff(page).locator('del')).toHaveText([' and more']);
});

test.describe('pictures', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'set FVOCI_SCREENS=1 to take the pictures');
  test.use({ locale: 'ko-KR' });

  const SCREENS = path.join(SCREENS_ROOT, '..', 'save-version', 'screens');
  const SIZES = [
    { name: 'wide', width: 1280, height: 800 },
    { name: 'narrow', width: 390, height: 844 },
  ] as const;
  const text = (value: string): object => ({ type: 'text', text: value });
  const paragraph = (value: string): object => ({ type: 'paragraph', content: [text(value)] });
  const item = (value: string, checked: boolean): object => ({
    type: 'taskItem',
    attrs: { checked },
    content: [paragraph(value)],
  });
  const body = (draft: string, review: string, done: boolean, code: string): object => ({
    type: 'doc',
    content: [
      { type: 'heading', attrs: { level: 2 }, content: [text('출시 계획')] },
      paragraph(draft),
      ...[
        '일정은 팀 회의에서 정합니다.',
        '디자인 검토는 따로 합니다.',
        '문서는 위키에 둡니다.',
      ].map(paragraph),
      { type: 'taskList', content: [item('초안 쓰기', done), item(review, false)] },
      { type: 'codeBlock', attrs: { language: 'ts' }, content: [text(code)] },
    ],
  });

  for (const size of SIZES) {
    for (const scheme of ['light', 'dark'] as const) {
      test(`a version compared, in Korean (${size.name}, ${scheme})`, async ({
        page,
      }, testInfo) => {
        test.skip(testInfo.project.name !== 'desktop', 'the sizes are set here');
        await page.setViewportSize(size);
        await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
        const place = await openTaskCard(page);
        await writtenWholeElsewhere(
          page,
          place,
          body('다음 주 화요일까지 초안을 올립니다.', '검토 받기', false, 'const due = "화요일";'),
        );
        await expect(editor(page)).toContainText('화요일까지');
        await writtenWholeElsewhere(
          page,
          place,
          body(
            '다음 주 금요일까지 초안을 올렸습니다.',
            '팀장 검토 받기',
            true,
            'const due = "금요일";',
          ),
        );
        await expect(editor(page)).toContainText('금요일까지');
        await openVersions(page);
        await read(page, 0);
        await expect(diff(page).locator('ins').first()).toBeVisible();
        await page.evaluate(() => document.fonts.ready);
        await page.screenshot({ path: path.join(SCREENS, `compare-${size.name}-${scheme}.png`) });
      });
    }
  }
});
