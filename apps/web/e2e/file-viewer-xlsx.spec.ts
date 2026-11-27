/**
 * Workbooks (XLSX) in the file viewer, on the backend in memory: the Korean
 * workbook opened from a card's attachments shows its sheets as pages, its
 * values in their formats, its merged title and frozen rows, and says which
 * sheet has drawings it does not draw; a large sheet draws only what is in
 * view; whole rows and columns of a style of their own show it on their
 * empty cells; a workbook opened from a page's file block; one past a ceiling, an
 * old Excel file, and one read slowly that "Close" ends, worker and all, and
 * that past the time limit asks whether to wait on: waited on, it is shown in
 * the same frame; downloaded instead, or closed, it goes with its worker. The
 * frame asks the network for nothing.
 */
import { expect, test, type Locator, type Page, type Worker } from '@playwright/test';

import { manyCellsXlsx, manyMergesXlsx } from '../tests/support/hostile-xlsx.ts';
import {
  koreanXlsx,
  largeXlsx,
  ROW_COLUMN_TITLE,
  rowColumnXlsx,
  XLSX_TEXT,
} from '../tests/support/viewer-xlsx.ts';

import { attach, frameOf, openCard, picture, viewer } from './file-viewer.support.ts';
import { openApp } from './shell.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

/** Where the pictures go with `FVOCI_SCREENS=1`: `target/viewer-p2/`. */
const SCREENS = 'viewer-p2';

const XLSX_TYPE = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet';

/**
 * The texts of the cells of `sheet` drawn as the frame scrolls across it, from its left to its
 * right and back: how many times each was drawn at once, at most. All in the frame, at once: a
 * round trip for each step costs a remote browser seconds.
 */
async function cellsAcross(sheet: Locator): Promise<Map<string, number>> {
  const seen = await sheet.evaluate(async (table) => {
    const scroll = async (x: number): Promise<number> => {
      window.scrollTo(x, 0);
      // The sheet is drawn anew on the scroll, before the next frame.
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return document.documentElement.scrollWidth - window.innerWidth;
    };
    const most = new Map<string, number>();
    const across = await scroll(0);
    for (let x = 0; ; x = Math.min(across, x + 150)) {
      await scroll(x);
      const here = new Map<string, number>();
      for (const cell of table.querySelectorAll('[role="cell"]')) {
        // As the role finds them: not in what is hidden from screen readers.
        if (cell.closest('[aria-hidden="true"]')) continue;
        const text = cell.textContent.trim();
        here.set(text, (here.get(text) ?? 0) + 1);
      }
      for (const [text, count] of here) most.set(text, Math.max(count, most.get(text) ?? 0));
      if (x >= across) break;
    }
    await scroll(0);
    return [...most];
  });
  return new Map(seen);
}

/** The requests the viewer's frame makes, from its document on. */
function frameRequests(page: Page): string[] {
  const made: string[] = [];
  page.on('request', (request) => {
    const frame = request.frame();
    const url = new URL(request.url());
    // What the frame holds in memory (`blob:`, `data:`) is not the network.
    if (!url.protocol.startsWith('http')) return;
    if (
      frame.url().includes('/static/viewer-frame.html') ||
      request.url().includes('/static/viewer-frame.html')
    ) {
      made.push(`${request.resourceType()} ${url.pathname}`);
    }
  });
  return made;
}

test.describe('in Korean', () => {
  test.use({ locale: 'ko-KR' });

  test('a workbook in Korean opens on its first sheet, and its sheets are its pages', async ({
    page,
  }) => {
    const requests = frameRequests(page);
    await openCard(page);
    const row = await attach(page, '분기 매출.xlsx', koreanXlsx(), XLSX_TYPE);
    await row.getByRole('link', { name: '분기 매출.xlsx' }).click();
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    const frame = frameOf(page);
    const sheet = frame.getByRole('table', { name: XLSX_TEXT.sheets[0] });
    await expect(sheet).toBeVisible();
    await expect(page.getByTestId('file-viewer-announce')).toHaveText('시트 2개 중 1번째: 매출');
    const tabs = page.getByTestId('file-viewer-sheets').getByRole('button');
    await expect(tabs).toHaveText([...XLSX_TEXT.sheets]);
    await expect(tabs.first()).toHaveAttribute('aria-current', 'true');

    // The values in their formats, each in one cell (on a phone, some only once scrolled to),
    // the merged title over its five columns, the note one cell of text.
    const seen = await cellsAcross(sheet);
    for (const text of [
      XLSX_TEXT.title,
      ...XLSX_TEXT.headers,
      ...XLSX_TEXT.sales,
      ...XLSX_TEXT.rates,
      ...XLSX_TEXT.deadlines,
      XLSX_TEXT.total,
      '#DIV/0!',
      'TRUE',
      XLSX_TEXT.inline,
      XLSX_TEXT.note,
    ]) {
      expect(seen.get(text), text).toBe(1);
    }
    await expect(sheet.getByRole('cell', { name: XLSX_TEXT.title })).toHaveAttribute(
      'aria-colspan',
      '5',
    );
    await expect(page.getByTestId('file-viewer-drawings')).toHaveCount(0);
    await picture(page, SCREENS, 'xlsx-korean');

    // The frozen rows stay as the sheet scrolls under them.
    const header = sheet.getByRole('cell', { name: XLSX_TEXT.headers[0], exact: true });
    const before = await header.boundingBox();
    await frame.locator('body').evaluate(() => {
      window.scrollTo(0, 300);
    });
    await expect.poll(async () => (await header.boundingBox())?.y).toBe(before?.y);

    // The next sheet: by the button, announced by its name; its drawings are said not to be drawn.
    await page.getByRole('button', { name: '다음 시트' }).click();
    // Its drawings are said not to be shown, in what is announced as well as under the sheet.
    await expect(page.getByTestId('file-viewer-announce')).toHaveText(
      '시트 2개 중 2번째: 일정 이 시트의 차트와 그림은 보이지 않습니다.',
    );
    const schedule = frame.getByRole('table', { name: XLSX_TEXT.sheets[1] });
    for (const text of [XLSX_TEXT.schedule, ...XLSX_TEXT.meetings, XLSX_TEXT.builtIn]) {
      await expect(schedule.getByRole('cell', { name: text, exact: true })).toHaveCount(1);
    }
    // The hidden column is not drawn.
    await expect(schedule.getByRole('cell', { name: '숨긴 열' })).toHaveCount(0);
    await expect(page.getByTestId('file-viewer-drawings')).toHaveText(
      '이 시트의 차트와 그림은 보이지 않습니다.',
    );
    await picture(page, SCREENS, 'xlsx-schedule');
    // With Shift, the arrows across turn no sheet from the viewer's buttons either (the sheet
    // turned by a key is drawn before the next command reaches the page).
    await page.getByRole('button', { name: '이전 시트' }).focus();
    await page.keyboard.press('Shift+ArrowLeft');
    await expect(page.getByTestId('file-viewer-page')).toContainText('2 / 2');

    // And back by its tab.
    await tabs.first().click();
    await expect(page.getByTestId('file-viewer-announce')).toHaveText('시트 2개 중 1번째: 매출');
    await expect(frame.getByRole('table', { name: XLSX_TEXT.sheets[0] })).toBeVisible();

    // Nothing but the frame's own document and what it is built of.
    expect(
      requests.filter((request) => !/^(document|script|stylesheet|font) /.test(request)),
    ).toEqual([]);
  });
});

test('a styled row or column shows its fill on its empty cells; a cell its own', async ({
  page,
}) => {
  await openCard(page);
  const row = await attach(page, '행과 열.xlsx', rowColumnXlsx(), XLSX_TYPE);
  await row.getByRole('link', { name: '행과 열.xlsx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const sheet = frameOf(page).getByRole('table', { name: '서식' });
  await expect(sheet.getByRole('cell', { name: ROW_COLUMN_TITLE })).toBeVisible();
  // The fills of the empty cells drawn in each row, and of what is written in row 2.
  const fills = await sheet.evaluate((table) => {
    const of = (selector: string): string[] => [
      ...new Set(
        [...table.querySelectorAll<HTMLElement>(selector)].map(
          (cell) => getComputedStyle(cell).backgroundColor,
        ),
      ),
    ];
    const blank = (row: number): string =>
      `[aria-rowindex="${row}"] .xlsx-cell[aria-hidden="true"]`;
    return {
      row2: of(blank(2)),
      row3: of(blank(3)),
      row4: of(blank(4)),
      written2: of('[aria-rowindex="2"] [role="cell"]'),
      merged: of('.xlsx-merged'),
    };
  });
  expect(fills).toEqual({
    row2: ['rgb(255, 0, 0)'],
    row3: ['rgb(0, 0, 255)'],
    row4: [],
    written2: ['rgba(0, 0, 0, 0)'],
    merged: ['rgb(0, 0, 255)'],
  });
  await picture(page, SCREENS, 'xlsx-row-column');
});

test('a large sheet draws only what is in view, and as far down as it goes', async ({ page }) => {
  await openCard(page);
  const row = await attach(page, 'large.xlsx', largeXlsx(5_000, 30), XLSX_TYPE);
  await row.getByRole('link', { name: 'large.xlsx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const frame = frameOf(page);
  await expect(frame.getByRole('cell', { name: '행 2', exact: true })).toBeVisible();
  const cells = (): Promise<number> => frame.locator('.xlsx-cell').count();
  // A screen's worth: 30 columns by 5,000 rows is 150,000 cells.
  expect(await cells()).toBeLessThan(2_000);
  await picture(page, SCREENS, 'xlsx-large');
  // From the keyboard, in the frame: Shift with the arrows across scrolls the sheet across, and
  // turns no sheet.
  const across = (): Promise<number> => frame.locator('body').evaluate(() => window.scrollX);
  await frame.getByRole('cell', { name: '행 2', exact: true }).click();
  const start = await across();
  await page.keyboard.press('Shift+ArrowRight');
  await expect.poll(across).toBeGreaterThan(start + 39);
  await page.keyboard.press('Shift+ArrowLeft');
  await expect.poll(across).toBe(start);
  await expect(page.getByTestId('file-viewer-announce')).toHaveText('Sheet 1 of 1: 큰 시트');
  await frame.locator('body').evaluate(() => {
    window.scrollTo(0, document.documentElement.scrollHeight);
  });
  // The sheet goes on 50 rows past its last, as a spreadsheet does, and no further.
  const numbers = frame.locator('.xlsx-ruler-down');
  const last = numbers.getByText('5050', { exact: true });
  await expect(last).toBeVisible();
  await expect(numbers.getByText('5051', { exact: true })).toHaveCount(0);
  expect(await cells()).toBeLessThan(2_000);
  // Fifty rows up, the last row of cells is at the bottom of the view.
  const height = (await last.boundingBox())?.height ?? 0;
  await frame.locator('body').evaluate((_, up) => {
    window.scrollBy(0, -up);
  }, height * 50);
  await expect(frame.getByRole('cell', { name: '행 5000', exact: true })).toBeVisible();
  expect(await cells()).toBeLessThan(2_000);
  // The first row is frozen: it stays at the top at the bottom of the sheet.
  await expect(frame.getByRole('cell', { name: '행 1', exact: true })).toBeVisible();
});

test('a workbook in a page opens from its file block', async ({ page }) => {
  await openApp(page);
  await newWiki(page, 'Budget');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Sales');
  const body = pageBody(page);
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  await body.click();
  await page.keyboard.type('/file');
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.keyboard.press('Enter'),
  ]);
  await chooser.setFiles({ name: 'sales.xlsx', mimeType: XLSX_TYPE, buffer: koreanXlsx() });
  const block = body.locator('[data-file-block]');
  await expect(block).toHaveAttribute('data-state', 'shown');
  await block.getByRole('button', { name: 'View sales.xlsx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(page.getByTestId('file-viewer-announce')).toHaveText('Sheet 1 of 2: 매출');
  await expect(frameOf(page).getByRole('cell', { name: XLSX_TEXT.sales[0] })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
});

test('a workbook past a ceiling, and an old Excel file, are offered as downloads', async ({
  page,
}) => {
  await openCard(page);
  const merges = await attach(page, 'merges.xlsx', manyMergesXlsx(100_001), XLSX_TYPE);
  await merges.getByRole('link', { name: 'merges.xlsx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(page.getByTestId('file-viewer-notice')).toContainText(
    "This file's contents are too large to preview.",
  );
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  await page.getByTestId('file-viewer-close').click();

  // The compound file of Excel 97 to 2003: a kind the viewer does not draw.
  const ole = Buffer.concat([
    Buffer.from([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]),
    Buffer.alloc(504),
  ]);
  const old = await attach(page, 'old.xls', ole, 'application/vnd.ms-excel');
  await old.getByRole('link', { name: 'old.xls' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'unknown');
  await expect(page.getByTestId('file-viewer-notice')).toContainText(
    "This file can't be previewed.",
  );
});

/**
 * A workbook under every ceiling that takes its worker seconds to read: 900,000 cells of a
 * number each (13.5 MB of the sheet's markup, under the 30 MB a part may have).
 */
const slowXlsx = (): Buffer => manyCellsXlsx(900_000, 9_000, '<c><v>1</v></c>');

/**
 * Opens the workbook of `row` and gives once its worker is there: by its event, which comes
 * however short the worker's life (a poll of the workers may look before it and after it).
 */
async function openReading(page: Page, row: Locator, name: string): Promise<void> {
  const started = page.waitForEvent('worker');
  await row.getByRole('link', { name }).click();
  await started;
  // Still reading: the worker is ended by what follows, not by its answer.
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
}

/**
 * Holds messages coming back from the workbook worker inside `viewer-frame.html`
 * until the returned function is called. The real worker still runs and parses the
 * workbook, so termination and frame checks stay real, while fast engines cannot
 * finish reading and unmount the 30 s notice before the test acts on it.
 */
async function holdWorkbookWorker(page: Page): Promise<() => Promise<void>> {
  await page.addInitScript(() => {
    if (!location.pathname.endsWith('/viewer-frame.html')) return;
    const NativeWorker = globalThis.Worker;
    type OnMessage = InstanceType<typeof NativeWorker>['onmessage'];
    let held = true;
    const queued: (() => void)[] = [];
    class HeldWorker extends NativeWorker {
      #terminated = false;
      #onmessage: OnMessage = null;
      override get onmessage(): OnMessage {
        return this.#onmessage;
      }
      override set onmessage(handler: OnMessage) {
        this.#onmessage = handler;
        super.onmessage = handler
          ? (event: MessageEvent) => {
              const deliver = (): void => {
                if (this.#terminated || this.#onmessage !== handler) return;
                handler.call(this, event);
              };
              if (held) queued.push(deliver);
              else deliver();
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
          const deliver = (): void => {
            if (this.#terminated) return;
            if (typeof listener === 'function') listener.call(this, event);
            else listener.handleEvent(event);
          };
          if (held) queued.push(deliver);
          else deliver();
        };
        super.addEventListener(type, wrapped, options);
      }
      override terminate(): void {
        this.#terminated = true;
        super.terminate();
      }
    }
    globalThis.Worker = HeldWorker;
    (window as unknown as { __releaseWorkbookWorker?: () => void }).__releaseWorkbookWorker =
      () => {
        held = false;
        for (const deliver of queued.splice(0)) deliver();
      };
  });
  return async () => {
    await frameOf(page)
      .locator(':root')
      .evaluate(() => {
        const release = (window as unknown as { __releaseWorkbookWorker?: () => void })
          .__releaseWorkbookWorker;
        if (!release) throw new Error('workbook worker hold hook not installed');
        release();
      });
  };
}

test('while a large workbook is read, "Close" ends the viewer and its worker', async ({ page }) => {
  await holdWorkbookWorker(page);
  await openCard(page);
  const row = await attach(page, 'cells.xlsx', slowXlsx(), XLSX_TYPE);
  await openReading(page, row, 'cells.xlsx');
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer(page)).toHaveCount(0, { timeout: 2000 });
  await expect.poll(() => page.workers().length).toBe(0);
});

/**
 * Opens the slow workbook under a clock of the test's, and runs the clock past the time the
 * viewer gives a file to be drawn in: it asks the reader whether to wait on. The workers the
 * page started are counted from before it opened.
 */
async function askedToWait(
  page: Page,
): Promise<{ notice: Locator; workers: Worker[]; release: () => Promise<void> }> {
  const release = await holdWorkbookWorker(page);
  await page.clock.install();
  await openCard(page);
  const row = await attach(page, 'slow.xlsx', slowXlsx(), XLSX_TYPE);
  const workers: Worker[] = [];
  page.on('worker', (worker) => workers.push(worker));
  await openReading(page, row, 'slow.xlsx');
  await page.clock.runFor(31_000);
  const notice = page.getByTestId('file-viewer-notice');
  await expect(notice).toContainText('This file is taking long to show.');
  await expect(viewer(page)).toHaveAttribute('data-status', 'opening');
  return { notice, workers, release };
}

test('a workbook read past the time limit asks, and waited on is shown in the frame it was handed to', async ({
  page,
}) => {
  const { notice, workers, release } = await askedToWait(page);
  const frame = await page.getByTestId('file-viewer-frame').elementHandle();
  await notice.getByRole('button', { name: 'Keep waiting' }).click();
  await release();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(frameOf(page).getByRole('cell').first()).toBeVisible();
  // Not drawn anew: the same frame, and the one worker it read the workbook in.
  expect(await frame.evaluate((element) => element.isConnected)).toBe(true);
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(1);
  expect(workers).toHaveLength(1);
});

test('a workbook read past the time limit, downloaded instead, is given up, worker and all', async ({
  page,
}) => {
  const { notice } = await askedToWait(page);
  const downloading = page.waitForEvent('download');
  await notice.getByRole('button', { name: 'Download' }).click();
  expect((await downloading).suggestedFilename()).toBe('slow.xlsx');
  await expect(viewer(page)).toHaveAttribute('data-status', 'failed');
  await expect(notice).toContainText('The file took too long to show.');
  await expect(notice.getByRole('button', { name: 'Keep waiting' })).toHaveCount(0);
  await expect(page.getByTestId('file-viewer-frame')).toHaveCount(0);
  await expect.poll(() => page.workers().length).toBe(0);
});

test('a workbook read past the time limit, closed while it asks, ends the viewer and its worker', async ({
  page,
}) => {
  await askedToWait(page);
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer(page)).toHaveCount(0);
  await expect.poll(() => page.workers().length).toBe(0);
});
