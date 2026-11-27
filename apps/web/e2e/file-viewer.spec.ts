/**
 * The file viewer, opened from a card's attachments: a PDF turned through,
 * a Word document in Korean, a file it does not know, one too large, one
 * that says it unpacks to too much and one that hides how much, Word
 * documents that would draw too much or draw slowly, and
 * the frame the renderers run in kept off the network and the app's storage.
 *
 * With `FVOCI_SCREENS=1` the tests also take pictures of each state into
 * `target/viewer-p1/` (light and dark), at every size the project runs at
 * (the phone at 390 pixels).
 */
import { closeSync, ftruncateSync, openSync, writeSync } from 'node:fs';

import { expect, test } from '@playwright/test';

import { manyLinksDocx, nestedSuperscriptDocx } from '../tests/support/hostile-docx.ts';
import {
  blankPaddedDocx,
  DOCX_LINK,
  DOCX_TEXT,
  koreanDocx,
  koreanPdf,
  repeatedHeaderDocx,
  zip,
} from '../tests/support/viewer-files.ts';

import {
  attach,
  frameOf,
  frameReaches,
  openCard,
  picture,
  timerLateness,
  viewer,
} from './file-viewer.support.ts';

const SCREENS = 'viewer-p1';
const DOCX_TYPE = 'application/vnd.openxmlformats-officedocument.wordprocessingml.document';

test('a PDF opens in the viewer, and its pages are turned with the buttons and the keys', async ({
  page,
}) => {
  // The viewer, its frame and the renderers are fetched when a file is first opened.
  const fetched: string[] = [];
  page.on('request', (request) => fetched.push(new URL(request.url()).pathname));
  const viewerFiles = (): string[] =>
    fetched.filter((file) => /\/(FileViewer|pdf|docx|viewer-frame)[-.][^/]*$/.test(file));
  await openCard(page);
  const row = await attach(page, '보고서.pdf', koreanPdf(3), 'application/pdf');
  expect(viewerFiles()).toEqual([]);
  await row.getByRole('link', { name: '보고서.pdf' }).click();

  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  // The keys that scroll a page scroll the document: the focus is in it.
  await expect(page.getByTestId('file-viewer-frame')).toBeFocused();
  expect(viewerFiles().some((file) => /\/pdf-[0-9a-f]{16}\.js$/.test(file))).toBe(true);
  expect(viewerFiles().some((file) => file.includes('/docx-'))).toBe(false);
  await expect(viewer(page).getByRole('heading', { name: '보고서.pdf' })).toBeVisible();
  const counter = page.getByTestId('file-viewer-page');
  await expect(counter).toContainText('1 / 3');
  await expect(counter).toContainText('Page 1 of 3');
  // The first page is drawn, and its text (Korean, through the bundled CMap) is there to select.
  const frame = frameOf(page);
  await expect(frame.locator('.page[data-page="1"] canvas')).toBeVisible();
  await expect(frame.locator('.page[data-page="1"] .textLayer')).toContainText('한국어 견본 1쪽');
  await picture(page, SCREENS, 'pdf');

  await viewer(page).getByRole('button', { name: 'Next page' }).click();
  await expect(counter).toContainText('2 / 3');
  await expect(frame.locator('.page[data-page="2"] canvas')).toBeVisible();

  // The keys work with the focus in the document as well as around it.
  await frame.locator('.page[data-page="2"]').click();
  await page.keyboard.press('ArrowRight');
  await expect(counter).toContainText('3 / 3');
  await expect(viewer(page).getByRole('button', { name: 'Next page' })).toBeDisabled();
  await page.keyboard.press('ArrowLeft');
  await expect(counter).toContainText('2 / 3');

  const zoom = page.getByTestId('file-viewer-zoom');
  await expect(zoom).toHaveText('100%');
  await page.keyboard.press('+');
  await expect(zoom).toHaveText('125%');
  await viewer(page).getByRole('button', { name: 'Zoom out' }).click();
  await viewer(page).getByRole('button', { name: 'Zoom out' }).click();
  await expect(zoom).toHaveText('75%');

  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  // The card is still open, and the focus is back on the file's name.
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(row.getByRole('link', { name: '보고서.pdf' })).toBeFocused();
});

test('the frame reaches neither the network nor the app', async ({ page }) => {
  await openCard(page);
  await page.evaluate(() => {
    localStorage.setItem('fvoci:viewer-probe', 'secret');
  });
  const row = await attach(page, 'probe.pdf', koreanPdf(1), 'application/pdf');
  await row.getByRole('link', { name: 'probe.pdf' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');

  const sandbox = await page.getByTestId('file-viewer-frame').getAttribute('sandbox');
  expect(sandbox).toBe('allow-scripts');
  const found = await frameReaches(page);
  // A message from another window than the frame (here, the page itself) is not taken.
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        const done = (event: MessageEvent): void => {
          if (event.data !== 'probe-done') return;
          window.removeEventListener('message', done);
          resolve();
        };
        window.addEventListener('message', done);
        window.postMessage({ kind: 'failed', reason: 'locked' }, '*');
        window.postMessage('probe-done', '*');
      }),
  );
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  expect(found.origin).toBe('null');
  for (const [what, outcome] of Object.entries(found)) {
    if (what !== 'origin') expect(outcome, what).toMatch(/^refused/);
  }
});

test('a Word document in Korean shows its header, its table and its picture', async ({ page }) => {
  await openCard(page);
  const row = await attach(page, '회의록.docx', koreanDocx(), DOCX_TYPE);
  await row.getByRole('link', { name: '회의록.docx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  // Two pages: the second where Word last broke the page.
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 2');

  const frame = frameOf(page);
  const pageOne = frame.locator('section.docx').first();
  await expect(pageOne.locator('header')).toContainText(DOCX_TEXT.header);
  await expect(pageOne).toContainText(DOCX_TEXT.title);
  await expect(pageOne).toContainText(DOCX_TEXT.body);
  for (const cell of DOCX_TEXT.cells) await expect(pageOne.locator('table')).toContainText(cell);
  const image = pageOne.locator('img');
  await expect(image).toBeVisible();
  expect(await image.evaluate((element: HTMLImageElement) => element.naturalWidth)).toBe(200);
  // The Korean falls back to the app's own font rather than to whatever the system has.
  const family = await pageOne
    .getByText(DOCX_TEXT.title)
    .evaluate((element) => getComputedStyle(element).fontFamily);
  expect(family).toContain('Noto Sans KR');
  await expect(frame.locator('html')).toHaveAttribute('lang', 'en');
  await picture(page, SCREENS, 'docx');

  // A link of the document goes nowhere: the frame keeps the document, and the viewer its pages.
  const link = pageOne.getByRole('link', { name: DOCX_TEXT.link });
  await expect(link).toHaveAttribute('href', DOCX_LINK);
  await link.click();
  await link.click({ button: 'middle' });
  await expect(pageOne).toContainText(DOCX_TEXT.title);
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const inside = page.frames().find((each) => each.url().endsWith('/static/viewer-frame.html'));
  expect(inside, 'the frame is still on its own document').toBeDefined();

  await viewer(page).getByRole('button', { name: 'Next page' }).click();
  await expect(page.getByTestId('file-viewer-page')).toContainText('2 / 2');
  await expect(frame.locator('section.docx').nth(1)).toContainText(DOCX_TEXT.second);
  await expect(frame.locator('section.docx').nth(1)).toBeInViewport();
});

test('a file of a kind the viewer does not draw is offered as a download', async ({ page }) => {
  await openCard(page);
  // Named as a PDF, but its bytes are not one.
  const row = await attach(page, 'notes.pdf', Buffer.from('just some notes'), 'application/pdf');
  await row.getByRole('link', { name: 'notes.pdf' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'unknown');
  const notice = page.getByTestId('file-viewer-notice');
  await expect(notice).toContainText("This file can't be previewed.");
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  await picture(page, SCREENS, 'unknown');

  const downloading = page.waitForEvent('download');
  await notice.getByRole('button', { name: 'Download' }).click();
  expect((await downloading).suggestedFilename()).toBe('notes.pdf');
});

test('a file over the limit is not read, and is offered as a download', async ({ page }, info) => {
  test.slow();
  await openCard(page);
  // A PDF's header, then nothing, up to one byte past the limit (a sparse file).
  const large = info.outputPath('large.pdf');
  const handle = openSync(large, 'w');
  writeSync(handle, '%PDF-1.7\n');
  ftruncateSync(handle, 50 * 1024 * 1024 + 1);
  closeSync(handle);
  await page.getByTestId('attach-input').setInputFiles(large);
  const row = page.getByTestId('card-attachments').locator('[data-attachment]', {
    hasText: 'large.pdf',
  });
  await row.getByRole('link', { name: 'large.pdf' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'too-large');
  await expect(page.getByTestId('file-viewer-notice')).toContainText('too large to preview');
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  await picture(page, SCREENS, 'too-large');
});

test('a Word document that says it unpacks past the limits is not drawn', async ({ page }) => {
  await openCard(page);
  // A few bytes, whose archive says its picture unpacks to over 200 MB.
  const archive = zip([
    { name: 'word/document.xml', data: Buffer.from('<w/>') },
    { name: 'word/media/image1.png', data: Buffer.from('x'), said: 200 * 1024 * 1024 },
  ]);
  const row = await attach(page, 'huge.docx', archive, DOCX_TYPE);
  await row.getByRole('link', { name: 'huge.docx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'too-large');
  const notice = page.getByTestId('file-viewer-notice');
  await expect(notice).toContainText("This file's contents are too large to preview.");
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);

  const downloading = page.waitForEvent('download');
  await notice.getByRole('button', { name: 'Download' }).click();
  expect((await downloading).suggestedFilename()).toBe('huge.docx');
});

test('a Word document whose XML is past the limit under another name is stopped in the frame', async ({
  page,
}) => {
  await openCard(page);
  // The same document with few blanks is drawn: what stops the large one is its size.
  const small = await attach(page, 'small.docx', blankPaddedDocx(1000), DOCX_TYPE);
  await small.getByRole('link', { name: 'small.docx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(frameOf(page).getByText(DOCX_TEXT.title)).toBeVisible();
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer(page)).toHaveCount(0);

  // Its directory tells the truth, but its main part is word/doc.bin: 30 MB of blanks, then XML.
  const archive = blankPaddedDocx(30 * 1024 * 1024);
  const row = await attach(page, 'hidden.docx', archive, DOCX_TYPE);
  await row.getByRole('link', { name: 'hidden.docx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(page.getByTestId('file-viewer-notice')).toContainText(
    'The file could not be shown: it may be damaged.',
  );
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
});

for (const [name, make] of [
  ['pages.docx', () => repeatedHeaderDocx(600, 2000)],
  ['nested.docx', () => nestedSuperscriptDocx(30)],
] as const) {
  test(`a small Word document that would draw too much is refused: ${name}`, async ({ page }) => {
    await openCard(page);
    const row = await attach(page, name, make(), DOCX_TYPE);
    await row.getByRole('link', { name }).click();
    await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
    await expect(page.getByTestId('file-viewer-notice')).toContainText(
      "This file's contents are too large to preview.",
    );
    await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  });
}

test('while a Word document is drawn, the app answers and "Close" closes the viewer', async ({
  page,
}) => {
  await openCard(page);
  // 60,000 links: docx-preview looks each up among all, for seconds, in the worker.
  const row = await attach(page, 'links.docx', manyLinksDocx(60_000), DOCX_TYPE);
  await row.getByRole('link', { name: 'links.docx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  // The page's timers keep their time: nothing holds its thread.
  expect(await timerLateness(page)).toBeLessThan(500);
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer(page)).toHaveCount(0, { timeout: 2000 });
  await expect(row.getByRole('link', { name: 'links.docx' })).toBeFocused();
});

test('the frame still takes the focus when a stylesheet is loading as the document appears', async ({
  page,
}) => {
  await openCard(page);
  const row = await attach(page, '보고서.pdf', koreanPdf(1), 'application/pdf');
  let release: (() => void) | undefined;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  let requested = false;
  await page.route('**/fvoci-pending-sheet.css', async (route) => {
    requested = true;
    await held;
    await route.fulfill({ contentType: 'text/css', body: '' }).catch(() => undefined);
  });
  try {
    await page.evaluate(() => {
      const link = document.createElement('link');
      link.rel = 'stylesheet';
      link.href = '/fvoci-pending-sheet.css';
      document.head.append(link);
    });
    await expect.poll(() => requested).toBe(true);
    await row.getByRole('link', { name: '보고서.pdf' }).click();
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    await expect(page.getByTestId('file-viewer-frame')).toBeFocused();
  } finally {
    release?.();
    await page.unroute('**/fvoci-pending-sheet.css').catch(() => undefined);
  }
});
