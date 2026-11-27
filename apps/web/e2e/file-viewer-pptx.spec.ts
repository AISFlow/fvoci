/**
 * PowerPoint decks in the file viewer, opened from a card's attachments: a
 * deck in Korean turned through slide by slide, its links kept in the frame,
 * decks that pass a ceiling of the PowerPoint worker and an old PowerPoint
 * file offered as a download, and the app kept answering (and "Close"
 * closing) while a deck is read and while its slides are drawn; a deck with
 * slides hidden from a show, opened on the others and its hidden ones shown
 * in place.
 *
 * With `FVOCI_SCREENS=1` the tests take pictures into `target/viewer-p3/`.
 */
import { expect, test, type Page } from '@playwright/test';

import { HOSTILE_PPTX, heavyPptx, slowPptx } from '../tests/support/hostile-pptx.ts';
import { koreanPptx, numberedPptx, numberedText, PPTX_TEXT } from '../tests/support/viewer-pptx.ts';

import {
  attach,
  frameOf,
  frameReaches,
  recordTicks,
  settlePageFonts,
  sharesThread,
  ticks,
  openCard,
  picture,
  timerLateness,
  viewer,
  viewerFrame,
} from './file-viewer.support.ts';

const PPTX_TYPE = 'application/vnd.openxmlformats-officedocument.presentationml.presentation';
const SCREENS = 'viewer-p3';
/** The pictures of the hidden slides, of the follow-up to viewer-p2. */
const SCREENS_HIDDEN = 'viewer-p2';

async function open(page: Page, name: string, file: Buffer): Promise<void> {
  const row = await attach(page, name, file, PPTX_TYPE);
  await row.getByRole('link', { name }).click();
}

test('a deck in Korean opens in the viewer, and its slides are turned', async ({ page }) => {
  const fetched: string[] = [];
  page.on('request', (request) => fetched.push(new URL(request.url()).pathname));
  await openCard(page);
  await open(page, '분기 보고.pptx', koreanPptx());
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  // Only the PowerPoint renderer is fetched, the Word and PDF renderers not.
  expect(fetched.some((file) => /\/pptx-[0-9a-f]{16}\.js$/.test(file))).toBe(true);
  expect(fetched.some((file) => /\/(docx|pdf)-[0-9a-f]{16}\.js$/.test(file))).toBe(false);
  const counter = page.getByTestId('file-viewer-page');
  await expect(counter).toContainText('1 / 3');
  await expect(counter).toContainText('Slide 1 of 3');

  const frame = frameOf(page);
  const first = frame.locator('.slide[data-page="1"]');
  await expect(first).toContainText(PPTX_TEXT.title);
  await expect(first).toContainText(PPTX_TEXT.body);
  const image = first.locator('img');
  await expect(image).toBeVisible();
  expect(await image.evaluate((element: HTMLImageElement) => element.naturalWidth)).toBe(300);
  // The Korean falls back to the app's own font rather than to whatever the system has.
  const family = await first
    .getByText(PPTX_TEXT.title)
    .evaluate((element) => getComputedStyle(element).fontFamily);
  expect(family).toContain('Noto Sans KR');
  await picture(page, SCREENS, 'pptx');

  await viewer(page).getByRole('button', { name: 'Next slide' }).click();
  await expect(counter).toContainText('2 / 3');
  const second = frame.locator('.slide[data-page="2"]');
  await expect(second).toBeInViewport();
  for (const cell of PPTX_TEXT.cells.flat()) await expect(second).toContainText(cell);
  // Charts are not drawn: a box says so.
  await expect(second.locator('[data-fvoci-chart]')).toHaveText(
    'Charts are not shown in the preview',
  );
  // A link to a web page is text that goes nowhere.
  const link = second.getByText(PPTX_TEXT.link);
  await expect(link).toBeVisible();
  expect(await link.evaluate((element) => element.closest('[href]'))).toBeNull();
  await link.click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');

  // The keys turn the slides with the focus in the deck as well as around it.
  await second.click();
  await page.keyboard.press('ArrowRight');
  await expect(counter).toContainText('3 / 3');
  await expect(frame.locator('.slide[data-page="3"]')).toContainText(PPTX_TEXT.third);
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowLeft');
  await expect(counter).toContainText('1 / 3');
  // A jump to another slide, which the deck has as a link, goes to that slide.
  await first.getByText(PPTX_TEXT.jump).click();
  await expect(counter).toContainText('3 / 3');

  const zoom = page.getByTestId('file-viewer-zoom');
  await page.keyboard.press('+');
  await expect(zoom).toHaveText('125%');
  // Zoomed out, several slides are in view at once.
  for (const _ of [1, 2, 3]) await page.keyboard.press('-');
  await expect(zoom).toHaveText('50%');
  await expect
    .poll(() =>
      viewerFrame(page).evaluate(
        () =>
          [...document.querySelectorAll('.slide')].filter((box) => {
            const { top, bottom } = box.getBoundingClientRect();
            return box.childElementCount > 0 && bottom > 0 && top < innerHeight;
          }).length,
      ),
    )
    .toBeGreaterThanOrEqual(2);
  await expect(frame.locator('.slide[data-page="2"]')).toContainText(PPTX_TEXT.tableTitle);
  await picture(page, SCREENS, 'pptx-slides');
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
});

/** The share of the pixels of two pictures of one size that differ by more than `by` in a colour. */
function pixelsApart(page: Page, one: Buffer, other: Buffer, by: number): Promise<number> {
  return page.evaluate(
    async ([first, second, most]) => {
      const pixels = async (base64: string): Promise<Uint8ClampedArray> => {
        const bytes = Uint8Array.from(atob(base64), (character) => character.charCodeAt(0));
        const picture = await createImageBitmap(new Blob([bytes], { type: 'image/png' }));
        const canvas = new OffscreenCanvas(picture.width, picture.height);
        const context = canvas.getContext('2d');
        if (!context) throw new Error('no canvas');
        context.drawImage(picture, 0, 0);
        return context.getImageData(0, 0, picture.width, picture.height).data;
      };
      const [a, b] = [await pixels(first), await pixels(second)];
      if (a.length !== b.length) return 1;
      let apart = 0;
      for (let at = 0; at < a.length; at += 4) {
        const each = [0, 1, 2].map((channel) =>
          Math.abs((a[at + channel] ?? 0) - (b[at + channel] ?? 0)),
        );
        if (Math.max(...each) > most) apart += 1;
      }
      return apart / (a.length / 4);
    },
    [one.toString('base64'), other.toString('base64'), by] as const,
  );
}

/**
 * The numbers of the slides laid out in the frame, in order. The frame is told of a change by
 * a message, after the page around it shows it: asked until it is so, not once.
 */
function laidOut(page: Page): Promise<number[]> {
  return viewerFrame(page).evaluate(() =>
    [...document.querySelectorAll<HTMLElement>('.slide:not([hidden])')].map((box) =>
      Number(box.dataset.page),
    ),
  );
}

test('a deck with hidden slides opens on those a show gives; the hidden ones come in place, marked', async ({
  page,
}) => {
  await openCard(page);
  // Hidden: the first, two together, and the last.
  await open(page, '숨긴 장.pptx', numberedPptx(7, [1, 3, 4, 7]));
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const counter = page.getByTestId('file-viewer-page');
  const announce = page.getByTestId('file-viewer-announce');
  const line = page.getByTestId('file-viewer-slides');
  const toggle = page.getByTestId('file-viewer-hidden');
  const frame = frameOf(page);
  await expect(line.getByRole('status')).toHaveText('Slides: 3');
  await expect(toggle).toHaveText('Show hidden slides (4)');
  // The first slide a show gives is 2, numbered as in the file.
  await expect(counter).toContainText('2 / 7');
  await expect(announce).toHaveText('Slide 2 of 7');
  await expect.poll(() => laidOut(page)).toEqual([2, 5, 6]);
  await expect(frame.locator('.slide[data-page="2"]')).toContainText(numberedText(2));
  await picture(page, SCREENS_HIDDEN, 'pptx-hidden');

  // Turned past the hidden slides, by the buttons and the keys; the ends are 2 and 6.
  const next = viewer(page).getByRole('button', { name: 'Next slide' });
  const previous = viewer(page).getByRole('button', { name: 'Previous slide' });
  await expect(previous).toBeDisabled();
  await next.click();
  await expect(counter).toContainText('5 / 7');
  // The keys from within the deck (the button "Next" lets the focus go once it is disabled).
  await frame.locator('.slide[data-page="5"]').click();
  await page.keyboard.press('ArrowRight');
  await expect(counter).toContainText('6 / 7');
  await expect(next).toBeDisabled();
  await page.keyboard.press('ArrowLeft');
  await expect(counter).toContainText('5 / 7');

  // Reached by the keyboard: from the viewer's buttons, Tab, and Enter.
  await viewer(page).getByTestId('file-viewer-close').focus();
  await page.keyboard.press('Tab');
  await expect(toggle).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(toggle).toHaveText('Hide hidden slides');
  await expect(toggle).toBeFocused();
  await expect(line.getByRole('status')).toHaveText('Slides: 7, 4 hidden');
  await expect.poll(() => laidOut(page)).toEqual([1, 2, 3, 4, 5, 6, 7]);
  // The reader stays on slide 5, where it was.
  await expect(counter).toContainText('5 / 7');
  await expect(frame.locator('.slide[data-page="5"]')).toBeInViewport();
  // Each hidden slide is marked in words, not by its colour alone.
  const marked = frame.locator('.slide-hidden');
  await expect(marked).toHaveCount(4);
  for (const number of [1, 3, 4, 7]) {
    await expect(frame.locator(`.slide[data-page="${number}"] .slide-mark`)).toHaveText('Hidden');
  }
  await expect(frame.locator('.slide[data-page="5"] .slide-mark')).toHaveCount(0);
  await previous.click();
  await expect(counter).toContainText('4 / 7');
  await expect(announce).toHaveText('Slide 4 of 7, hidden');
  await expect(frame.locator('.slide[data-page="4"]')).toContainText(numberedText(4));
  await picture(page, SCREENS_HIDDEN, 'pptx-hidden-shown');

  // Left out again while on hidden slide 4: the reader lands on 5, the next a show gives.
  await toggle.click();
  await expect(toggle).toHaveText('Show hidden slides (4)');
  await expect(counter).toContainText('5 / 7');
  await expect.poll(() => laidOut(page)).toEqual([2, 5, 6]);
  await expect(frame.locator('.slide[data-page="5"]')).toBeInViewport();
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);

  // Opened again, the deck is on the slides a show gives: the choice is not kept.
  await page
    .getByTestId('card-attachments')
    .locator('[data-attachment]', { hasText: '숨긴 장.pptx' })
    .getByRole('link', { name: '숨긴 장.pptx' })
    .click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(toggle).toHaveText('Show hidden slides (4)');
});

for (const [locale, words] of [
  ['en-US', { count: 'Slides: 49', show: 'Show hidden slides (3)', hide: 'Hide hidden slides' }],
  [
    'ko-KR',
    { count: '슬라이드 49장', show: '숨긴 슬라이드 3장 보기', hide: '숨긴 슬라이드 감추기' },
  ],
] as const) {
  test.describe(locale, () => {
    test.use({ locale });

    test('the line of hidden slides fits at the narrowest width', async ({ page }) => {
      await narrowest(page, words);
    });
  });
}

async function narrowest(
  page: Page,
  words: { count: string; show: string; hide: string },
): Promise<void> {
  await page.setViewportSize({ width: 390, height: 844 });
  await openCard(page);
  await open(page, '숨긴 장이 든 아주 긴 이름의 발표 자료.pptx', numberedPptx(52, [3, 4, 5]));
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const toggle = page.getByTestId('file-viewer-hidden');
  await expect(toggle).toHaveText(words.show);
  await expect(page.getByTestId('file-viewer-slides').getByRole('status')).toHaveText(words.count);
  const overflows = (): Promise<boolean[]> =>
    page.evaluate(() =>
      ['file-viewer', 'file-viewer-slides'].map((id) => {
        const element = document.querySelector(`[data-testid="${id}"]`);
        return !element || element.scrollWidth > element.clientWidth;
      }),
    );
  expect(await overflows()).toEqual([false, false]);
  await toggle.click();
  await expect(toggle).toHaveText(words.hide);
  expect(await overflows()).toEqual([false, false]);
  const box = await toggle.boundingBox();
  expect((box?.x ?? -1) + (box?.width ?? 0)).toBeLessThanOrEqual(390);
}

test('a deck whose slides are all hidden shows them all, marked, and no button', async ({
  page,
}) => {
  await openCard(page);
  await open(page, '모두 숨김.pptx', numberedPptx(2, [1, 2]));
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(page.getByTestId('file-viewer-slides').getByRole('status')).toHaveText(
    'Slides: 2, all hidden',
  );
  await expect(page.getByTestId('file-viewer-hidden')).toHaveCount(0);
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 2');
  await expect(page.getByTestId('file-viewer-announce')).toHaveText('Slide 1 of 2, hidden');
  await expect.poll(() => laidOut(page)).toEqual([1, 2]);
  await expect(frameOf(page).locator('.slide-mark')).toHaveCount(2);
});

test('a deck without hidden slides has no line of them', async ({ page }) => {
  await openCard(page);
  await open(page, '셋.pptx', numberedPptx(3, []));
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 3');
  await expect(page.getByTestId('file-viewer-slides')).toHaveCount(0);
});

test('a deck at the narrowest width, in the dark: the slide fits and keeps its colours', async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: 'dark' });
  await openCard(page);
  await open(page, '분기 보고.pptx', koreanPptx());
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(page.getByTestId('file-viewer-announce')).toHaveText('Slide 1 of 3');
  await expect(frameOf(page).locator('.slide[data-page="1"]')).toContainText(PPTX_TEXT.title);
  const seen = await viewerFrame(page).evaluate(() => {
    const box = document.querySelector('.slide');
    const drawn = box?.firstElementChild;
    return {
      dark: document.documentElement.classList.contains('dark'),
      scheme: getComputedStyle(document.documentElement).colorScheme,
      width: document.documentElement.clientWidth,
      scrolls: (document.scrollingElement?.scrollWidth ?? 0) > document.documentElement.clientWidth,
      box: box?.getBoundingClientRect().width ?? 0,
      background: drawn ? getComputedStyle(drawn).backgroundColor : '',
    };
  });
  // The frame is dark as the app is; the slide is as wide as the frame allows, keeps its own
  // white, and nothing scrolls sideways.
  expect(seen.dark).toBe(true);
  expect(seen.scheme).toBe('dark');
  expect(seen.scrolls).toBe(false);
  expect(seen.box).toBeGreaterThan(seen.width - 40);
  expect(seen.box).toBeLessThanOrEqual(seen.width - 32);
  expect(seen.background).toBe('rgb(255, 255, 255)');
  // Neither the app nor its frame is wider than the screen.
  expect(await page.evaluate(() => document.scrollingElement?.scrollWidth)).toBeLessThanOrEqual(
    390,
  );
  const box = await page.getByTestId('file-viewer-frame').boundingBox();
  expect((box?.x ?? -1) >= 0 && (box?.x ?? 0) + (box?.width ?? 391) <= 390).toBe(true);
  // The slide looks the same in the light, its edge (blended with what is around it) left out:
  // nothing of the dark is laid over it.
  const slide = await frameOf(page).locator('.slide[data-page="1"]').boundingBox();
  if (!slide) throw new Error('no slide');
  const inside = {
    x: slide.x + 2,
    y: slide.y + 2,
    width: slide.width - 4,
    height: slide.height - 4,
  };
  const dark = await page.screenshot({ clip: inside, animations: 'disabled' });
  await page.emulateMedia({ colorScheme: 'light' });
  await expect(frameOf(page).locator('html')).not.toHaveClass(/\bdark\b/);
  const light = await page.screenshot({ clip: inside, animations: 'disabled' });
  // The curved edges of shapes are smoothed a little apart from one drawing to the next
  // (WebKit: a few dozen pixels); a dark that showed through, even a veil of it, would turn
  // the slide's white everywhere.
  expect(await pixelsApart(page, light, dark, 24)).toBeLessThan(0.01);
  // The slides are turned and zoomed within the screen's width.
  for (const name of ['Previous slide', 'Next slide', 'Zoom out', 'Zoom in']) {
    const button = await viewer(page).getByRole('button', { name }).boundingBox();
    expect(button, name).not.toBeNull();
    expect((button?.x ?? -1) >= 0 && (button?.x ?? 0) + (button?.width ?? 0) <= 390, name).toBe(
      true,
    );
  }
});

test("a deck's frame reaches neither the network nor the app", async ({ page }) => {
  await openCard(page);
  await page.evaluate(() => {
    localStorage.setItem('fvoci:viewer-probe', 'secret');
  });
  await open(page, 'probe.pptx', koreanPptx());
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const found = await frameReaches(page);
  expect(found.origin).toBe('null');
  for (const [what, outcome] of Object.entries(found)) {
    if (what !== 'origin') expect(outcome, what).toMatch(/^refused/);
  }
});

for (const ceiling of ['slides', 'textBodies', 'pixels'] as const) {
  test(`a deck past a ceiling is not drawn, and is offered as a download: ${ceiling}`, async ({
    page,
  }) => {
    await openCard(page);
    await open(page, `${ceiling}.pptx`, HOSTILE_PPTX[ceiling]?.() ?? Buffer.alloc(0));
    await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
    const notice = page.getByTestId('file-viewer-notice');
    await expect(notice).toContainText("This file's contents are too large to preview.");
    await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
    if (ceiling === 'slides') await picture(page, SCREENS, 'pptx-too-large');
    const downloading = page.waitForEvent('download');
    await notice.getByRole('button', { name: 'Download' }).click();
    expect((await downloading).suggestedFilename()).toBe(`${ceiling}.pptx`);
  });
}

test('an old PowerPoint file (not a ZIP archive) is offered as a download', async ({ page }) => {
  await openCard(page);
  const compound = Buffer.alloc(2048);
  compound.set([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
  const row = await attach(page, '옛 발표.ppt', compound, 'application/vnd.ms-powerpoint');
  await row.getByRole('link', { name: '옛 발표.ppt' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'unknown');
  await expect(page.getByTestId('file-viewer-notice')).toContainText(
    "This file can't be previewed.",
  );
});

/**
 * Holds `{ kind: 'read' }` replies coming back from the PowerPoint worker inside
 * `viewer-frame.html`, while forwarding any failure reply immediately. The real
 * worker still runs and checks/parses the deck on its own thread, while a fast
 * engine cannot finish and leave `data-status="opening"` before the test checks
 * responsiveness and clicks "Close".
 */
async function holdPptxWorker(page: Page): Promise<void> {
  await page.addInitScript(() => {
    if (!location.pathname.endsWith('/viewer-frame.html')) return;
    const NativeWorker = globalThis.Worker;
    const isReadReply = (event: Event): boolean => {
      const data = (event as MessageEvent<unknown>).data;
      return (
        typeof data === 'object' && data !== null && (data as { kind?: unknown }).kind === 'read'
      );
    };
    class HeldWorker extends NativeWorker {
      #onmessage: ((this: Worker, event: MessageEvent) => unknown) | null = null;
      override get onmessage(): ((this: Worker, event: MessageEvent) => unknown) | null {
        return this.#onmessage;
      }
      override set onmessage(handler: ((this: Worker, event: MessageEvent) => unknown) | null) {
        this.#onmessage = handler;
        super.onmessage = handler
          ? function (this: Worker, event: MessageEvent) {
              if (!isReadReply(event)) return handler.call(this, event);
              return undefined;
            }
          : null;
      }
      override addEventListener(
        type: string,
        listener: EventListenerOrEventListenerObject,
        options?: boolean | AddEventListenerOptions,
      ): void {
        if (type !== 'message') {
          super.addEventListener(type, listener, options);
          return;
        }
        const wrapped: EventListener = (event) => {
          if (isReadReply(event)) return;
          if (typeof listener === 'function') listener.call(this, event);
          else listener.handleEvent(event);
        };
        super.addEventListener(type, wrapped, options);
      }
    }
    globalThis.Worker = HeldWorker;
  });
}

test('while a deck is read, the app answers and "Close" closes the viewer', async ({ page }) => {
  await holdPptxWorker(page);
  await openCard(page);
  await settlePageFonts(page);
  // Megabytes of XML under every ceiling: the worker reads it for seconds.
  const row = await attach(page, 'long.pptx', slowPptx(900, 3_000), PPTX_TYPE);
  const started = page.waitForEvent('worker');
  await row.getByRole('link', { name: 'long.pptx' }).click();
  await started;
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  expect(await timerLateness(page)).toBeLessThan(500);
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer(page)).toHaveCount(0, { timeout: 2000 });
  await expect.poll(() => page.workers().length).toBe(0);
});

/**
 * Holds `IntersectionObserver` deliveries inside `viewer-frame.html` until the
 * returned function is called. `drawFirst()` still draws slide 1 and reports
 * `shown`, while the remaining near slides stay queued until release: releasing
 * synchronously starts `#drawInView()` (drawing slide 2 and scheduling the rest)
 * and reports the live `{ drawn, waiting }` counts with no IPC gap.
 */
async function holdSlideSight(
  page: Page,
): Promise<() => Promise<{ drawn: number; waiting: number }>> {
  await page.addInitScript(() => {
    if (!location.pathname.endsWith('/viewer-frame.html')) return;
    const NativeObserver = globalThis.IntersectionObserver;
    let held = true;
    const queued: (() => void)[] = [];
    let notifyFirst: (() => void) | undefined;
    class HeldObserver extends NativeObserver {
      constructor(callback: IntersectionObserverCallback, options?: IntersectionObserverInit) {
        super((entries, observer) => {
          const deliver = (): void => {
            callback(entries, observer);
          };
          if (held) {
            queued.push(deliver);
            notifyFirst?.();
            notifyFirst = undefined;
          } else {
            deliver();
          }
        }, options);
      }
    }
    globalThis.IntersectionObserver = HeldObserver;
    const hook = window as unknown as {
      __releaseSlideSight?: () => Promise<{ drawn: number; waiting: number }>;
    };
    hook.__releaseSlideSight = async () => {
      if (held && queued.length === 0) {
        await new Promise<void>((resolve) => {
          notifyFirst = resolve;
        });
      }
      held = false;
      for (const deliver of queued.splice(0)) deliver();
      const boxes = [...document.querySelectorAll('.slide')];
      const near = (box: Element): boolean => {
        const { top, bottom } = box.getBoundingClientRect();
        return bottom > -innerHeight && top < 2 * innerHeight;
      };
      return {
        drawn: boxes.filter((box) => box.childElementCount > 0).length,
        waiting: boxes.filter((box) => box.childElementCount === 0 && near(box)).length,
      };
    };
  });
  return () =>
    viewerFrame(page).evaluate(() => {
      const release = (
        window as unknown as {
          __releaseSlideSight?: () => Promise<{ drawn: number; waiting: number }>;
        }
      ).__releaseSlideSight;
      if (!release) throw new Error('slide sight hold hook not installed');
      return release();
    });
}

/** Starts noting, in the frame, when each slide is put in its box (the times as `recordTicks`'). */
async function recordDrawn(page: Page): Promise<void> {
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(1);
  await expect
    .poll(() => page.frames().some((each) => each.url().endsWith('/static/viewer-frame.html')))
    .toBe(true);
  await viewerFrame(page).evaluate(() => {
    const noted = window as unknown as { fvociDrawn: number[] };
    noted.fvociDrawn = [];
    new MutationObserver((records) => {
      for (const record of records) {
        const box = record.target;
        if (
          box instanceof HTMLElement &&
          box.classList.contains('slide') &&
          record.addedNodes.length > 0
        ) {
          noted.fvociDrawn.push(performance.timeOrigin + performance.now());
        }
      }
    }).observe(document.body, { childList: true, subtree: true });
  });
}

/** Small slides of many text boxes (`perSlide`): many in view, each a while to draw. */
const boxes = (perSlide = 80): Buffer => heavyPptx(40, perSlide, { cx: 3_048_000, cy: 914_400 });

test('while slides are drawn, the app answers', async ({ page }) => {
  await openCard(page);
  await settlePageFonts(page);
  await recordTicks(page);
  await open(page, 'boxes.pptx', boxes());
  await recordDrawn(page);
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const drawnTimes = (): Promise<number[]> =>
    viewerFrame(page).evaluate(() => (window as unknown as { fvociDrawn: number[] }).fvociDrawn);
  await expect.poll(async () => (await drawnTimes()).length).toBeGreaterThanOrEqual(5);
  // Slide after slide was put in place, and meanwhile the page's timer ran: at least as often
  // as a slide was put in place (drawn at one go, they would have it run never). The browser
  // may still run the frame's turn before the page's now and then, so not between every two.
  const drawn = await drawnTimes();
  const ran = await ticks(page);
  const during = ran.filter((tick) => tick >= (drawn[0] ?? 0) && tick <= (drawn.at(-1) ?? 0));
  expect(during.length).toBeGreaterThanOrEqual(drawn.length - 1);
  // Nor did one slide hold the thread long: a bound well past the slowest engine's slide.
  const longest = Math.max(0, ...during.slice(1).map((tick, index) => tick - (during[index] ?? 0)));
  expect(longest).toBeLessThan(1_500);
  // And all this says something: the frame holds the page's thread when it holds its own.
  expect(await sharesThread(page)).toBe(true);
});

test('while slides are drawn, "Close" closes the viewer', async ({ page }) => {
  const release = await holdSlideSight(page);
  await openCard(page);
  await settlePageFonts(page);
  // Slides that take longer, so that some near the view are still to draw once the viewer shows.
  await open(page, 'boxes.pptx', boxes(120));
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const started = await release();
  expect(started.drawn).toBeGreaterThanOrEqual(2);
  expect(started.waiting).toBeGreaterThan(0);
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer(page)).toHaveCount(0, { timeout: 2000 });
});
