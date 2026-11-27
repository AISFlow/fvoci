/**
 * What the browser tests of the kanban and the gallery share: opening a view
 * of a seeded board, giving the board a width, reading the columns off the
 * page, a mouse that picks up, and a finger.
 *
 * A target is always measured where it lies *now*: the gap that follows the
 * pointer moves the cards below it.
 */
import { expect, type CDPSession, type Locator, type Page, type TestInfo } from '@playwright/test';

import { boardPath } from './address.ts';
import { touchScreen } from './touch.support.ts';

export interface Point {
  x: number;
  y: number;
}

/** The configured device's viewport, less the sidebar used by its layout stage. */
export function paneOf(info: TestInfo): number {
  const width = info.project.use.viewport?.width ?? 1280;
  const sidebar = info.project.name === 'desktop' ? 256 : info.project.name === 'middle' ? 56 : 0;
  return width - sidebar;
}
/** The pane of this project fits one column at a time (34rem is the boundary). */
export const oneColumn = (info: TestInfo): boolean => paneOf(info) < 544;
export const touch = (info: TestInfo): boolean => info.project.name !== 'desktop';

export const boardPage = (page: Page): Locator => page.getByTestId('board-page');
export const kanban = (page: Page): Locator => page.getByTestId('kanban-board');
export const overlay = (page: Page): Locator => page.locator('[data-drag-overlay]');
export const announcer = (page: Page): Locator => page.getByTestId('announcer');
export const notice = (page: Page): Locator => page.getByTestId('drag-notice');
export const dialog = (page: Page): Locator => page.getByTestId('card-dialog');

/** A card by its title. */
export const card = (page: Page, title: string): Locator =>
  page.locator('[data-card]').filter({ has: page.getByText(title, { exact: true }) });
/** A column by the name in its heading. */
export const column = (page: Page, name: string): Locator =>
  page.locator('[data-column]').filter({
    has: page.locator('h3').filter({ hasText: new RegExp(`^\\s*${escape(name)}\\s*$`) }),
  });
export const heading = (page: Page, name: string): Locator =>
  column(page, name).locator('[data-column-heading]');
/** The list of cards of a column: what scrolls. */
export const cardsBox = (page: Page, name: string): Locator =>
  column(page, name).locator('[data-drag-list]');

function escape(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Comes back once the window is `width` wide and the page's own observers of size have been
 * told of it: an observer is told after those made before it, and the state they changed is
 * applied by then, so the next reading lays the page out as the new stage has it. WebKit on
 * Linux takes the new size some time after it was asked for (Playwright waits for it on some
 * Linux images, not on all), so a report of the old width is no answer.
 */
async function observersTold(page: Page, width: number): Promise<void> {
  await page.evaluate(
    (wanted) =>
      new Promise<void>((resolve) => {
        const observer = new ResizeObserver(() => {
          if (window.innerWidth !== wanted) return;
          observer.disconnect();
          resolve();
        });
        observer.observe(document.documentElement);
      }),
    width,
  );
}

/** Changes the width of the window until the board has the width asked for. */
export async function fitPane(page: Page, width: number): Promise<void> {
  for (let pass = 0; pass < 5; pass += 1) {
    const got = await boardPage(page).evaluate((element) => element.getBoundingClientRect().width);
    if (Math.abs(got - width) < 1) return;
    const size = page.viewportSize();
    if (!size) throw new Error('the page has no viewport');
    const wide = Math.round(size.width + width - got);
    await page.setViewportSize({ width: wide, height: size.height });
    await observersTold(page, wide);
  }
  throw new Error(`the board could not be made ${width} pixels wide`);
}

/**
 * Opens a view of a seeded board by the titles of the two, at the width the
 * board has in this project, and fails on any error the page throws.
 */
export async function openView(
  page: Page,
  info: TestInfo,
  boardTitle: string,
  viewTitle: string,
): Promise<void> {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await expect(boardPage(page)).toBeVisible();
  // The identifiers are the same on every load: the seed counts, it does not draw lots.
  const place = await page.evaluate(
    async ([board, view]) => {
      const { backend } = (
        window as unknown as {
          fvociDev: {
            backend: {
              workspaces(): Promise<{ id: string }[]>;
              boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
              views(boardId: string): Promise<{ id: string; title: string }[]>;
            };
          };
        }
      ).fvociDev;
      const workspaces = await backend.workspaces();
      const boards = await backend.boards(workspaces[0]?.id ?? '');
      const found = boards.find((candidate) => candidate.title === board);
      const views = await backend.views(found?.id ?? '');
      const shown = views.find((candidate) => candidate.title === view);
      return found && shown ? { boardId: found.id, viewId: shown.id } : undefined;
    },
    [boardTitle, viewTitle] as const,
  );
  if (!place) throw new Error(`the seed has no view "${viewTitle}" on a board "${boardTitle}"`);
  await page.goto(boardPath(place.boardId, place.viewId));
  await expect(boardPage(page)).toBeVisible();
  await fitPane(page, paneOf(info));
  expect(errors).toEqual([]);
}

/**
 * Takes back the last thing the reader did, the way the shell's undo does.
 * A drop is first shown and then written; the undo waits for the writing.
 */
export async function undo(page: Page): Promise<void> {
  await expect(page.locator('[data-settling]')).toHaveCount(0);
  await page.evaluate(() =>
    (
      window as unknown as { fvociDev: { workspace: { undoLast(): Promise<void> } } }
    ).fvociDev.workspace.undoLast(),
  );
}

/** The names of the columns, in the order they stand in. */
export async function columnNames(page: Page): Promise<string[]> {
  return page
    .locator('[data-column]:not(.hidden) h3')
    .evaluateAll((names) => names.map((element) => element.textContent.trim()));
}

/** The titles of the cards of a column, top to bottom. */
export async function titlesIn(page: Page, name: string): Promise<string[]> {
  return column(page, name)
    .locator('[data-card] [data-slot="card-title-text"]')
    .evaluateAll((titles) => titles.map((element) => element.textContent.trim()));
}

/** A point on an element as it lies now: in the middle across, `down` of the way from its top. */
export async function pointOn(locator: Locator, down = 0.5): Promise<Point> {
  const box = await locator.boundingBox();
  if (!box) throw new Error('the element is not on the page');
  return { x: box.x + box.width / 2, y: box.y + box.height * down };
}

/**
 * Runs `change`, and comes back once an observer made now has been told of another width of
 * `target`: the page's own observers, made before it, have been told by then.
 */
export async function seenResized(target: Locator, change: () => Promise<void>): Promise<void> {
  await target.evaluate((element: HTMLElement) => {
    const was = element.offsetWidth;
    Reflect.set(
      element,
      'seenResized',
      new Promise<void>((resolve) => {
        const observer = new ResizeObserver(() => {
          if (element.offsetWidth === was) return;
          observer.disconnect();
          resolve();
        });
        observer.observe(element);
      }),
    );
  });
  await change();
  await target.evaluate((element) => Reflect.get(element, 'seenResized') as Promise<void>);
}

export async function nextFrame(page: Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise((resolve) => {
        requestAnimationFrame(resolve);
      }),
  );
}

/** No drag is going on and nothing is left of one. */
export async function expectNoDrag(page: Page): Promise<void> {
  await expect(kanban(page)).not.toHaveAttribute('data-dragging');
  await expect(overlay(page)).toHaveCount(0);
  await expect(page.locator('[data-drag-gap]')).toHaveCount(0);
  await expect(page.locator('[data-column-gap]')).toHaveCount(0);
}

/** Chooses a line of the menu that is open: a dropdown, or on a compact page a sheet. */
export async function choose(page: Page, action: string): Promise<void> {
  await page.locator(`[data-action="${action}"]`).click();
}

// ------------------------------------------------------------------ mouse

/** Presses on an element and moves far enough for the drag to begin. */
export async function grab(page: Page, target: Locator): Promise<void> {
  const from = await pointOn(target);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + 6, from.y + 6, { steps: 3 });
  await expect(overlay(page)).toBeVisible();
}

/** Moves onto an element as it lies now. Twice: the first move may open a gap that shifts it. */
export async function hover(page: Page, target: Locator, down: number): Promise<void> {
  for (let pass = 0; pass < 2; pass += 1) {
    const to = await pointOn(target, down);
    await page.mouse.move(to.x, to.y, { steps: 8 });
    await nextFrame(page);
  }
}

// ------------------------------------------------------------------ touch

/** One finger, moved by touch events sent to the browser as a touch screen would send them. */
export class Finger {
  readonly #client: CDPSession;
  readonly #page: Page;
  #at: Point = { x: 0, y: 0 };

  constructor(client: CDPSession, page: Page) {
    this.#client = client;
    this.#page = page;
  }

  static async on(page: Page): Promise<Finger> {
    return new Finger(await touchScreen(page), page);
  }

  async #send(type: 'touchStart' | 'touchMove', at: Point): Promise<void> {
    this.#at = at;
    await this.#client.send('Input.dispatchTouchEvent', {
      type,
      touchPoints: [{ x: Math.round(at.x), y: Math.round(at.y), id: 1 }],
    });
  }

  async down(at: Point): Promise<void> {
    await this.#send('touchStart', at);
  }

  /** Moves in steps, a frame apart, as a finger does. */
  async move(to: Point, steps = 8): Promise<void> {
    const from = this.#at;
    for (let step = 1; step <= steps; step += 1) {
      await this.#send('touchMove', {
        x: from.x + ((to.x - from.x) * step) / steps,
        y: from.y + ((to.y - from.y) * step) / steps,
      });
      await nextFrame(this.#page);
    }
  }

  async up(): Promise<void> {
    await this.#client.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  }

  /** Rests on an element until the drag begins. */
  async rest(target: Locator, down = 0.5): Promise<void> {
    await this.down(await pointOn(target, down));
    await expect(overlay(this.#page)).toBeVisible();
  }

  /** Moves onto an element as it lies now. Twice, as with the mouse. */
  async hover(target: Locator, down: number): Promise<void> {
    for (let pass = 0; pass < 2; pass += 1) {
      await this.move(await pointOn(target, down));
    }
  }
}
