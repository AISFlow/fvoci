/**
 * A Gantt view of a board of 2,000 cards on the in-memory backend: every bar
 * and arrow is drawn, and how long the chart takes to draw, to scroll a
 * frame, to take a change to one card and to draw a frame while a bar is
 * dragged is measured and printed (`[gantt 2000] …`, for a person to read).
 */
import { expect, test, type Locator, type Page, type TestInfo } from '@playwright/test';

import { boardPath } from './address.ts';
import { arrows, bar, bars, gantt, scroller, type Backend, type Dev } from './gantt.support.ts';
import { fitPane, nextFrame, paneOf } from './kanban.support.ts';

// No trace: it takes a picture of the page at every action of the test, and
// at 2,000 rows each picture takes a third of a second of the page's frames,
// which a drag (an action at every step) would be measured with.
test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US', trace: 'off' });

const MANY = 2000;

/**
 * Gives a board the page is not showing 2,000 dated cards, each coming after
 * the one before it, and a Gantt view of them; then shows that view, by the
 * app's own way of going to an address. Answers how long the view took to
 * be drawn, in milliseconds.
 */
async function openMany(page: Page, info: TestInfo): Promise<number> {
  // The clock of the page is left alone: a fixed clock stands in for the
  // page's frames and its timer, and frames are what is measured here.
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  await fitPane(page, paneOf(info));
  const made = await page.evaluate(async (count) => {
    const backend = (window as unknown as { fvociDev: { backend: Backend } }).fvociDev.backend;
    backend.latency = 0;
    const workspaces = await backend.workspaces();
    const boards = await backend.boards(workspaces[0]?.id ?? '');
    const boardId = boards.find((board) => board.title === 'Content calendar')?.id ?? '';
    const due = (await backend.container(boardId)).cardProperties.find(
      (property) => property.name === 'Publishing day',
    );
    if (!due) throw new Error('no board "Content calendar" with a Publishing day');
    await backend.patchContainer(boardId, {
      updatedCardProperties: [{ id: 'after', name: 'After', type: 'relation', options: [] }],
    });
    // From a month before today on: the cards are around today whenever this runs.
    const day = (offset: number): string =>
      new Date(Date.now() + (offset - 30) * 86_400_000).toISOString().slice(0, 10);
    let before: string | undefined;
    for (let at = 0; at < count; at += 1) {
      // Eight in nine are of several days; some one in nine starts before the one it comes after ends.
      const from = (at * 7) % 240;
      const made = await backend.createCard(boardId, {
        title: `Task ${String(at + 1)}`,
        properties: {
          [due.id]: { from: day(from), to: day(from + (at % 9)) },
          ...(before ? { after: [{ card: before, kind: 'fs', lagDays: 0 }] } : {}),
        },
      });
      before = made.id;
    }
    const view = await backend.createView(boardId, {
      title: 'Many',
      fields: {
        viewType: 'gantt',
        dateProperty: due.id,
        dependencyPropertyId: 'after',
        // Days: the chart is some 10,000 pixels across, more than the test scrolls.
        ganttZoom: 'day',
        sort: [],
        shownProperties: ['__title'],
        visibleGroups: [],
        hiddenGroups: [],
        collapsedGroups: [],
        filter: { operation: 'and', filters: [] },
        cardOrder: [],
        columnWidths: {},
        columnCalculations: {},
        groupCalculations: {},
        defaultTemplateId: '',
      },
    });
    return { boardId, viewId: view.id };
  }, MANY);
  const path = boardPath(made.boardId, made.viewId);
  // Timed on the page, from the change of address to the second frame after the last bar is there.
  const drawn = await page.evaluate(
    ([address, count]) =>
      new Promise<number>((resolve) => {
        const started = performance.now();
        history.pushState(null, '', address);
        window.dispatchEvent(new PopStateEvent('popstate'));
        const look = (): void => {
          if (document.querySelectorAll('[data-bar]').length < count) {
            requestAnimationFrame(look);
            return;
          }
          requestAnimationFrame(() => {
            requestAnimationFrame(() => {
              resolve(Math.round(performance.now() - started));
            });
          });
        };
        requestAnimationFrame(look);
      }),
    [path, MANY] as const,
  );
  await expect(gantt(page)).toBeVisible();
  expect(await bars(page).count()).toBeGreaterThanOrEqual(MANY);
  return drawn;
}

/** Scrolls a box a step a frame and answers the time each frame took, in milliseconds. */
async function scrollFrames(
  page: Page,
  axis: 'scrollTop' | 'scrollLeft',
  step: number,
  frames: number,
): Promise<number[]> {
  return scroller(page).evaluate(
    (element, [property, by, count]) =>
      new Promise<number[]>((resolve) => {
        const times: number[] = [];
        let last = performance.now();
        const frame = (now: number): void => {
          times.push(now - last);
          last = now;
          if (times.length > count) {
            // The first interval is the wait for the first frame, not a frame.
            resolve(times.slice(1));
            return;
          }
          element[property] += by;
          requestAnimationFrame(frame);
        };
        requestAnimationFrame(frame);
      }),
    [axis, step, frames] as const,
  );
}

/**
 * Drags a bar across with the mouse, a step a frame, and answers the time
 * each frame took meanwhile, in milliseconds; then lets it go. Without
 * `press`, the same moves without the button down: what the page costs
 * before a drag adds to it.
 */
async function dragFrames(page: Page, target: Locator, press = true): Promise<number[]> {
  const box = await target.boundingBox();
  if (!box) throw new Error('the bar is not on the page');
  const from = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await page.evaluate(() => {
    const record = { times: [] as number[], going: true };
    Reflect.set(window, 'dragFrames', record);
    let last = performance.now();
    const frame = (now: number): void => {
      record.times.push(now - last);
      last = now;
      if (record.going) requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  });
  await page.mouse.move(from.x, from.y);
  if (press) await page.mouse.down();
  for (let step = 1; step <= 60; step += 1) {
    await page.mouse.move(from.x + step * 4, from.y);
    await nextFrame(page);
  }
  await expect(gantt(page).locator('[data-bar][data-dragged]')).toHaveCount(press ? 1 : 0);
  const times = await page.evaluate(() => {
    const record = Reflect.get(window, 'dragFrames') as { times: number[]; going: boolean };
    record.going = false;
    return record.times.slice(1);
  });
  if (press) await page.mouse.up();
  await expect(gantt(page).locator('[data-bar][data-dragged]')).toHaveCount(0);
  return times;
}

/** The middle, the 95th of a hundred, and the longest of some times, rounded to a tenth. */
function summary(times: number[]): { median: number; p95: number; max: number } {
  const sorted = [...times].sort((a, b) => a - b);
  const at = (part: number): number =>
    Math.round((sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * part))] ?? 0) * 10) /
    10;
  return { median: at(0.5), p95: at(0.95), max: at(1) };
}

test('a board of 2,000 cards draws every bar and arrow, and scrolls in frames that are measured', async ({
  page,
}, info) => {
  test.setTimeout(180_000);
  const drawn = await openMany(page, info);
  await expect(gantt(page)).toHaveAttribute('data-edges', String(MANY - 1));
  await expect(arrows(page)).toHaveCount(MANY - 1);
  // The frames are the page's own.
  expect(await page.evaluate(() => String(Reflect.get(window, 'requestAnimationFrame')))).toContain(
    '[native code]',
  );
  expect(Number(await gantt(page).getAttribute('data-late'))).toBeGreaterThan(100);

  const place = (): Promise<{ top: number; left: number }> =>
    scroller(page).evaluate((element) => ({ top: element.scrollTop, left: element.scrollLeft }));
  const from = await place();
  const down = summary(await scrollFrames(page, 'scrollTop', 90, 120));
  const across = summary(await scrollFrames(page, 'scrollLeft', 40, 120));
  // Every frame scrolled: the box did not come to an end on the way.
  const to = await place();
  expect(to.top - from.top).toBeGreaterThanOrEqual(120 * 90);
  expect(to.left - from.left).toBeGreaterThanOrEqual(120 * 40);

  // The arrows are drawn as high as the rows are, also after blocks of rows
  // were drawn and skipped again on the way down.
  const heights = async (): Promise<{ rows: number; arrows: number }> => ({
    rows: (await gantt(page).getByRole('list').boundingBox())?.height ?? 0,
    arrows: (await page.getByTestId('gantt-arrows').boundingBox())?.height ?? -1,
  });
  await scroller(page).evaluate((element) => {
    element.scrollTop = 0;
  });
  expect((await heights()).arrows).toBeCloseTo((await heights()).rows, 0);

  // A focused bar keeps the focus when a card above it leaves the chart and
  // its row moves from one block of rows to the one before.
  const kept = bars(page).nth(25);
  const keptId = (await kept.getAttribute('data-bar')) ?? '';
  await kept.focus();
  await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((property) => property.name === 'Publishing day');
    const first = document.querySelector('[data-bar]')?.getAttribute('data-bar');
    if (!due || !first) throw new Error('nothing to take the date from');
    await store.setCardProperty(first, due.id, undefined);
  });
  await expect(bars(page).nth(24)).toHaveAttribute('data-bar', keptId);
  await expect(bars(page).nth(24)).toBeFocused();
  // So does the title of a row, which has the focus after a card opened from it has closed.
  const title = gantt(page).locator('[data-row-title]').nth(25);
  const titleText = await title.textContent();
  await title.click();
  const cardDialog = page.getByTestId('card-dialog');
  await expect(cardDialog.locator('[data-card-title]')).toBeVisible();
  await cardDialog.press('Escape');
  await expect(cardDialog).toHaveCount(0);
  await expect(title).toBeFocused();
  await page.evaluate(async () => {
    const store = (window as unknown as { fvociDev: Dev }).fvociDev.workspace.board;
    const due = store.board.cardProperties.find((property) => property.name === 'Publishing day');
    const first = document.querySelector('[data-bar]')?.getAttribute('data-bar');
    if (!due || !first) throw new Error('nothing to take the date from');
    await store.setCardProperty(first, due.id, undefined);
  });
  const moved = gantt(page).locator('[data-row-title]').nth(24);
  await expect(moved).toHaveText(titleText ?? '');
  await expect(moved).toBeFocused();
  expect((await heights()).arrows).toBeCloseTo((await heights()).rows, 0);

  // One card renamed: how long the page is busy before it can draw again.
  const renamed = await page.evaluate(async () => {
    const store = (
      window as unknown as {
        fvociDev: {
          workspace: {
            board: {
              cards: { id: string; title: string }[];
              setCardTitle(cardId: string, title: string): Promise<void>;
            };
          };
        };
      }
    ).fvociDev.workspace.board;
    const card = store.cards.find((each) => each.title === 'Task 1000');
    if (!card) throw new Error('no card to rename');
    const started = performance.now();
    const written = store.setCardTitle(card.id, 'Task 1000, renamed');
    await new Promise((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(resolve));
    });
    const busy = performance.now() - started;
    await written;
    return Math.round(busy);
  });
  await expect(bar(page, 'Task 1000, renamed')).toHaveCount(1);

  // A bar dragged across 240 pixels, a step a frame, after the same moves without a press.
  await scroller(page).evaluate((element) => {
    element.scrollTop = 0;
  });
  const dragged = bars(page).nth(3);
  await dragged.scrollIntoViewIfNeeded();
  // Its middle in the middle of the days in sight: not under the titles, nor near an edge.
  const box = await dragged.boundingBox();
  const corner = await gantt(page).locator('[data-gantt-corner]').boundingBox();
  const sight = await scroller(page).boundingBox();
  if (!box || !corner || !sight) throw new Error('nothing to drag');
  await scroller(page).evaluate(
    (element, by) => {
      element.scrollLeft += by;
    },
    box.x + box.width / 2 - (corner.x + corner.width + sight.x + sight.width - 240) / 2,
  );
  const pointed = summary(await dragFrames(page, dragged, false));
  const drag = summary(await dragFrames(page, dragged));

  const figures = {
    project: info.project.name,
    cards: MANY,
    arrows: MANY - 1,
    drawnMs: drawn,
    scrollDownFrameMs: down,
    scrollAcrossFrameMs: across,
    renameOneCardMs: renamed,
    pointFrameMs: pointed,
    dragFrameMs: drag,
  };
  // Printed for a person to read, with the output of the run.
  process.stdout.write(`[gantt 2000] ${JSON.stringify(figures)}\n`);
  info.annotations.push({ type: 'gantt 2000', description: JSON.stringify(figures) });
  // Generous bounds: the figures are for reading, the bounds catch a chart that stopped scrolling.
  expect(down.median).toBeLessThan(100);
  expect(across.median).toBeLessThan(100);
  expect(renamed).toBeLessThan(2000);
  expect(drag.median).toBeLessThan(100);
});
