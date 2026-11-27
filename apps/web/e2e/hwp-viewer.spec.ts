/**
 * The file viewer with HWP and HWPX documents, opened from a card's
 * attachments: the pages of a Korean document turned through, its renderer
 * and WebAssembly fetched only when one is first opened, a locked one, another
 * compound file (an old Word document) and one past the limit offered as a
 * download, and the worker the renderer runs in kept off the network and
 * stopped when the viewer closes; a long document read past the time limit
 * asks whether to wait on, and waited on is shown by the worker that read it.
 *
 * With `FVOCI_SCREENS=1` the tests also take pictures of each state into
 * `target/viewer-h1/` (light and dark), at every size the project runs at
 * (the phone at 390 pixels), named by the browser too.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { deflateRawSync } from 'node:zlib';

import { expect, test, type Locator, type Page, type Worker } from '@playwright/test';

import {
  compoundFile,
  compoundStreams,
  denseHwpx,
  HWP_TEXT,
  hwpFileHeader,
  koreanHwp,
  koreanHwpx,
  lockedHwpx,
  longHwpx,
  rewriteArchive,
} from '../tests/support/hwp-files.ts';

import { attach, frameOf, openCard, viewer } from './file-viewer.support.ts';

const SCREENS = process.env.FVOCI_SCREENS === '1';
const SCREENS_DIR = path.resolve(import.meta.dirname, '../../../target/viewer-h1');
const HWPX_TYPE = 'application/hwp+zip';
const HWP_TYPE = 'application/x-hwp';
/** How long the app gives rhwp's engine to arrive (ENGINE_FETCH_LIMIT in lib/file-viewer/engine.ts). */
const ENGINE_LIMIT = 120_000;
/** How long the frame has to say it is ready (session.svelte.ts). */
const READY_LIMIT = 15_000;

async function view(page: Page, name: string, buffer: Buffer, mimeType: string): Promise<void> {
  const row = await attach(page, name, buffer, mimeType);
  await row.getByRole('link', { name }).click();
}

const notice = (page: Page): Locator => page.getByTestId('file-viewer-notice');

/** The next worker the viewer's frame starts (from a `blob:` of its opaque origin), and its end. */
function nextWorker(page: Page): Promise<{ worker: Worker; ended: Promise<void> }> {
  const inFrame = (worker: Worker): boolean => worker.url().startsWith('blob:null/');
  return page.waitForEvent('worker', { predicate: inFrame }).then((worker) => ({
    worker,
    ended: new Promise<void>((resolve) => {
      worker.once('close', () => {
        resolve();
      });
    }),
  }));
}

async function picture(page: Page, name: string): Promise<void> {
  if (!SCREENS) return;
  const { project } = test.info();
  // The engine that runs: device presets name it apart from `browserName`.
  const browser = page.context().browser()?.browserType().name() ?? 'unknown';
  // The phone at 390 pixels: narrowed when wider, its height kept (WebKit keeps `dvh` as it was).
  const size = page.viewportSize();
  if (project.name === 'mobile' && size && size.width > 390) {
    await page.setViewportSize({ width: 390, height: size.height });
  }
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    await page.waitForFunction(
      (dark) =>
        document.documentElement.classList.contains('dark') === dark &&
        document.getAnimations().every((animation) => animation.playState !== 'running'),
      theme === 'dark',
    );
    const file = `${name}-${browser}-${project.name}-${theme}.png`;
    await page.screenshot({ path: path.join(SCREENS_DIR, file) });
  }
  await page.emulateMedia({ colorScheme: null });
}

test('an HWPX document opens in the viewer, and its pages are turned with the buttons and the keys', async ({
  page,
}) => {
  // The renderer and its WebAssembly are fetched when an HWP document is first opened.
  const fetched: string[] = [];
  page.on('request', (request) => fetched.push(new URL(request.url()).pathname));
  const hwpFiles = (): string[] => fetched.filter((file) => /\/(hwp|rhwp-wasm)-/.test(file));
  await openCard(page);
  const worker = nextWorker(page);
  await view(page, '회의록.hwpx', koreanHwpx(), HWPX_TYPE);
  expect(hwpFiles()).toEqual([]);
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  expect(hwpFiles().some((file) => /\/hwp-[0-9a-f]{16}\.js$/.test(file))).toBe(true);
  expect(hwpFiles().some((file) => file.includes('rhwp-wasm'))).toBe(true);
  const { ended } = await worker;

  await expect(page.getByTestId('file-viewer-frame')).toBeFocused();
  const counter = page.getByTestId('file-viewer-page');
  await expect(counter).toContainText('1 / 2');
  const frame = frameOf(page);
  const first = frame.locator('.page[data-page="1"]');
  await expect(first.locator('svg')).toBeVisible();
  // The picture is hidden from a screen reader; the page's text, over it, is what it reads.
  await expect(first.locator('svg')).toHaveAttribute('aria-hidden', 'true');
  const text = first.locator('.hwp-text');
  await expect(text).toContainText(HWP_TEXT.header);
  await expect(text).toContainText(HWP_TEXT.title);
  await expect(text).toContainText(HWP_TEXT.body);
  for (const cell of ['항목', '담당', '설계 검토', '김민지'])
    await expect(text).toContainText(cell);
  await expect(first.locator('svg image')).toHaveCount(1);
  // The document's fonts fall back to the app's Korean font rather than to whatever the system has.
  const family = await first.locator('svg text').first().getAttribute('font-family');
  expect(family).toContain('Noto Sans KR');
  await picture(page, 'hwpx');

  await viewer(page).getByRole('button', { name: 'Next page' }).click();
  await expect(counter).toContainText('2 / 2');
  await expect(frame.locator('.page[data-page="2"] .hwp-text')).toContainText(HWP_TEXT.second);
  await expect(frame.locator('.page[data-page="2"]')).toBeInViewport();
  // The keys work with the focus in the document (the button went disabled at the last page).
  await frame.locator('.page[data-page="2"]').click();
  await page.keyboard.press('ArrowLeft');
  await expect(counter).toContainText('1 / 2');

  const zoom = page.getByTestId('file-viewer-zoom');
  const width = async (): Promise<number> => (await first.boundingBox())?.width ?? Number.NaN;
  const before = await width();
  await page.keyboard.press('+');
  await expect(zoom).toHaveText('125%');
  await expect.poll(async () => Math.abs((await width()) - before * 1.25)).toBeLessThan(2);

  // Closing the viewer stops the worker.
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await ended;
});

test('an HWP document opens the same way', async ({ page }) => {
  await openCard(page);
  await view(page, '회의록.hwp', koreanHwp(), HWP_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 2');
  const text = frameOf(page).locator('.page[data-page="1"] .hwp-text');
  await expect(text).toContainText(HWP_TEXT.header);
  await expect(text).toContainText(HWP_TEXT.title);
  await expect(text).toContainText('김민지');
  await picture(page, 'hwp');
});

test('another compound file, as an old Word document is, is offered as a download', async ({
  page,
}) => {
  await openCard(page);
  const word = compoundFile([
    { path: 'WordDocument', data: Buffer.alloc(5000, 7) },
    { path: '\u0001CompObj', data: Buffer.alloc(100, 1) },
  ]);
  await view(page, 'notes.hwp', word, HWP_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'unknown');
  await expect(notice(page)).toContainText("This file can't be previewed.");
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  await picture(page, 'ole');
  const downloading = page.waitForEvent('download');
  await notice(page).getByRole('button', { name: 'Download' }).click();
  expect((await downloading).suggestedFilename()).toBe('notes.hwp');
});

test('an HWP document whose streams say they hold more than the limit is not drawn', async ({
  page,
}) => {
  await openCard(page);
  const huge = compoundFile([
    { path: 'FileHeader', data: hwpFileHeader(1) },
    { path: 'BodyText/Section0', data: Buffer.alloc(5000), said: 200 * 1024 * 1024 },
  ]);
  await view(page, 'huge.hwp', huge, HWP_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'too-large');
  await expect(notice(page)).toContainText("This file's contents are too large to preview.");
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
});

test('the frame and the worker the renderer runs in reach no network', async ({ page }) => {
  await openCard(page);
  const started = nextWorker(page);
  await view(page, 'probe.hwpx', koreanHwpx(), HWPX_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const { worker } = await started;
  const attempt = async (address: string): Promise<string> => {
    try {
      return `reached: ${(await fetch(address)).status}`;
    } catch (error) {
      return `refused: ${error instanceof Error ? error.name : String(error)}`;
    }
  };
  const inside = page.frames().find((each) => each.url().endsWith('/static/viewer-frame.html'));
  if (!inside) throw new Error('no viewer frame');
  // A file of the app that any origin may read (`Access-Control-Allow-Origin: *`): only the
  // frame's policy stops it, and the app's own page reaches it.
  const open = inside.url();
  expect(await page.evaluate(attempt, open)).toBe('reached: 200');
  expect(await inside.evaluate(attempt, open)).toMatch(/^refused/);
  expect(await worker.evaluate(attempt, open)).toMatch(/^refused/);
});

test('an HWP whose picture unpacks past the limit is stopped in the worker before rhwp reads it', async ({
  page,
}) => {
  await openCard(page);
  // A file of about 200 KB whose picture unpacks to 1 byte past 200 MiB; its directory says less.
  const bomb = deflateRawSync(Buffer.alloc(200 * 1024 * 1024 + 1));
  const streams = compoundStreams(koreanHwp()).map((stream) =>
    stream.path.startsWith('BinData/') ? { ...stream, data: bomb } : stream,
  );
  const started = nextWorker(page);
  await view(page, 'bomb.hwp', compoundFile(streams), HWP_TYPE);
  const { ended } = await started;
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(notice(page)).toContainText("This file's contents are too large to preview.");
  await ended;
});

test('pictures that each end with a byte rhwp passes over count all they unpack to', async ({
  page,
}) => {
  await openCard(page);
  // 13 pictures of 16 MiB, each deflated to less than 16 KiB and followed by a byte of junk: rhwp
  // passes over the byte, and a browser's unpacker fails on it and drops what it unpacked last.
  const picture = Buffer.concat([deflateRawSync(Buffer.alloc(16 * 1024 * 1024)), Buffer.from([7])]);
  expect(picture.length).toBeLessThan(16 * 1024);
  const pictures = Array.from({ length: 13 }, (_, at) => ({
    path: `BinData/BIN${String(at + 100).padStart(4, '0')}.png`,
    data: picture,
  }));
  const started = nextWorker(page);
  await view(
    page,
    'junk.hwp',
    compoundFile([...compoundStreams(koreanHwp()), ...pictures]),
    HWP_TYPE,
  );
  const { ended } = await started;
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(notice(page)).toContainText("This file's contents are too large to preview.");
  await ended;
});

test('a page drawn past what the frame takes on says so in its place, and the others are drawn', async ({
  page,
}) => {
  await openCard(page);
  // 30,000 characters on the first page, each drawn apart: past 20,000 elements.
  const started = nextWorker(page);
  await view(page, 'dense.hwpx', denseHwpx(), HWPX_TYPE);
  const { ended } = await started;
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 2');
  const frame = frameOf(page);
  const first = frame.locator('.page[data-page="1"]');
  await expect(first.locator('.page-notice')).toContainText('This page is too large to show.');
  await expect(first.locator('svg')).toHaveCount(0);
  await picture(page, 'hwp-page-too-large');
  const downloading = page.waitForEvent('download');
  await first.getByRole('button', { name: 'Download' }).click();
  expect((await downloading).suggestedFilename()).toBe('dense.hwpx');
  await viewer(page).getByRole('button', { name: 'Next page' }).click();
  const second = frame.locator('.page[data-page="2"]');
  await expect(second.locator('.hwp-text')).toContainText(HWP_TEXT.second);
  await expect(second.locator('svg')).toBeVisible();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await page.keyboard.press('Escape');
  await ended;
});

test('a page past the memory the renderer may take: it fails, and its worker is stopped', async ({
  page,
}) => {
  await openCard(page);
  // A picture of 190 MiB (within what a file may unpack to, deflated to little), which rhwp
  // writes into the page whole: more than the 1 GiB it may take.
  const picture = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    Buffer.alloc(190 << 20, 7),
  ]);
  const heavy = rewriteArchive(koreanHwpx(), (member) =>
    member.name === 'BinData/image1.png' ? { ...member, data: picture, stored: false } : member,
  );
  const started = nextWorker(page);
  await view(page, 'heavy.hwpx', heavy, HWPX_TYPE);
  const { ended } = await started;
  // The renderer writes until it has no memory left, which is seconds of work and more on a busy
  // machine: the worker's end is what says it is over, and the failure is shown by then.
  await ended;
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(notice(page)).toContainText('The file could not be shown.');
});

test('a document locked with a password says so', async ({ page }) => {
  await openCard(page);
  await view(page, 'locked.hwpx', lockedHwpx(), HWPX_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(notice(page)).toContainText('protected with a password');
});

test('a frame whose script a gate in front of the server refuses says the preview did not start', async ({
  page,
}) => {
  await page.clock.install();
  await openCard(page);
  // As an access gate answers a request that came without its cookie: its login page, in the
  // place of the frame's script (a redirect to it, followed, comes to the same).
  await page.route(/\/static\/viewer-frame-[0-9a-f]+\.js$/, (route) =>
    route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Sign in</title>' }),
  );
  await view(page, 'gated.hwpx', koreanHwpx(), HWPX_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  await page.clock.fastForward(READY_LIMIT + 1000);
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(notice(page)).toContainText(
    'This file could not be shown: the preview did not start.',
  );
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
});

test('an engine answered with a gate’s login page is told as an engine that did not come', async ({
  page,
}) => {
  await openCard(page);
  await page.route(/rhwp-wasm/, (route) =>
    route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>Sign in</title>' }),
  );
  const workers: Worker[] = [];
  page.on('worker', (worker) => workers.push(worker));
  await view(page, 'gated.hwpx', koreanHwpx(), HWPX_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(notice(page)).toContainText(
    'This file could not be shown: what the preview needs could not be loaded.',
  );
  // The frame was never handed the document.
  expect(workers).toEqual([]);

  // What was not the engine is not kept: the next file asks for it again, and is shown.
  await page.unroute(/rhwp-wasm/);
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await page.getByRole('link', { name: 'gated.hwpx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
});

test('an engine that does not arrive is given up past its time, and asked for again by the next file', async ({
  page,
}) => {
  await page.clock.install();
  await openCard(page);
  // rhwp's WebAssembly, which the app fetches for the frame, never arrives.
  const held: (() => Promise<void>)[] = [];
  await page.route(/rhwp-wasm/, (route) => {
    held.push(() => route.abort());
  });
  const workers: Worker[] = [];
  page.on('worker', (worker) => workers.push(worker));
  await view(page, 'slow.hwpx', koreanHwpx(), HWPX_TYPE);
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  await expect.poll(() => held.length).toBe(1);
  await page.clock.fastForward(ENGINE_LIMIT + 1000);
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  // Told as an engine that did not come, not as a file too slow to draw.
  await expect(notice(page)).toContainText(
    'This file could not be shown: what the preview needs could not be loaded.',
  );
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  // The frame was never handed the document: nothing of it was read.
  expect(workers).toEqual([]);

  // The fetch given up is not kept: the next file asks for the engine again, and is shown.
  await page.unroute(/rhwp-wasm/);
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await page.getByRole('link', { name: 'slow.hwpx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
});

test('a long document read past the time limit asks, and waited on is shown by the worker that read it', async ({
  page,
}) => {
  await page.clock.install();
  await openCard(page);
  const started = nextWorker(page);
  await view(page, 'long.hwpx', longHwpx(), HWPX_TYPE);
  // Handed the document: the worker lays its pages out, seconds of work.
  const { ended } = await started;
  let stopped = false;
  void ended.then(() => {
    stopped = true;
  });
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  const frame = page.frames().find((one) => one.url().includes('viewer-frame'));
  const before = await frame?.evaluate(() => Date.now());
  await page.clock.runFor(31_000);
  // The frame's clock is the test's too: a time of its own to read the document in would be over.
  expect((await frame?.evaluate(() => Date.now())) ?? 0).toBeGreaterThanOrEqual(
    (before ?? Infinity) + 31_000,
  );
  await expect(notice(page)).toContainText('This file is taking long to show.');
  await notice(page).getByRole('button', { name: 'Keep waiting' }).click();
  // Laid out at last: the seconds of work the document is made for (5.5 to 6.6 on a machine
  // busy with other tests), so a bound of its own, under what is left of the test's time.
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown', { timeout: 15_000 });
  await expect(frameOf(page).locator('.page[data-page="1"] svg')).toBeVisible();
  expect(stopped).toBe(false);
});

test('the notice in place of a page fits in the page, however it is drawn', async ({ page }) => {
  // The frame's own rules, on page boxes as a page is drawn: on a phone and at half that, a small
  // page after a large one at half that again, pages much wider than tall, and a box just tall
  // enough for the Korean line on two lines (and the taller button of a touch screen).
  const css = readFileSync(
    path.resolve(import.meta.dirname, '../src/viewer-frame/frame.css'),
    'utf8',
  );
  // Each size, whether the line is shown (or left to screen readers, the page drawn small), and
  // in which language.
  const sizes = [
    [358, 506, true, 'en'],
    [179, 253, true, 'ko'],
    [89, 126, false, 'ko'],
    [500, 125, true, 'en'],
    [1000, 150, true, 'en'],
    [300, 100, false, 'en'],
    [200, 116, true, 'ko'],
    [300, 44, false, 'en'],
  ] as const;
  const words = {
    en: ['This page is too large to show.', 'Download'],
    ko: ['이 쪽은 너무 커서 보이지 않습니다.', '다운로드'],
  };
  const boxes = sizes
    .map(([width, height, , language]) => {
      const [line, button] = words[language];
      return `<div class="page" style="width:${String(width)}px;height:${String(height)}px"><div class="page-notice"><p>${line}</p><button type="button">${button}</button></div></div>`;
    })
    .join('');
  await page.setContent(`<style>${css}</style><div class="pages">${boxes}</div>`);
  const laidOut = await page.locator('.page-notice').evaluateAll((notices) =>
    notices.map((notice) => {
      const box = notice.getBoundingClientRect();
      const inside = (element: Element | null): boolean => {
        const part = element?.getBoundingClientRect();
        return (
          part !== undefined &&
          part.left >= box.left &&
          part.right <= box.right &&
          part.top >= box.top &&
          part.bottom <= box.bottom
        );
      };
      const line = notice.querySelector('p');
      return {
        button: inside(notice.querySelector('button')),
        // Left to screen readers, it is a pixel wide (and a little more at some pixel ratios).
        line: (line?.getBoundingClientRect().width ?? 0) > 2 ? inside(line) : 'for screen readers',
      };
    }),
  );
  expect(laidOut).toEqual(
    sizes.map(([, , shown]) => ({ button: true, line: shown ? true : 'for screen readers' })),
  );
});
