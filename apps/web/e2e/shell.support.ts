/**
 * What the tests of the shell share: where its parts are on the page, and
 * bringing the list of boards into view whatever the stage.
 *
 * The three projects are the three stages (`desktop` expanded, `middle`
 * medium, `mobile` compact). A test asks the page which stage it is in
 * rather than the project for its name, so it also holds when it sets a
 * size of its own.
 */
import { expect, type CDPSession, type Locator, type Page } from '@playwright/test';

import { SETTLED } from './address.ts';
import { touchScreen } from './touch.support.ts';

export type Stage = 'compact' | 'medium' | 'expanded';

export const shell = (page: Page): Locator => page.getByTestId('app-shell');
/** The list of boards: a column beside the board, or the drawer. */
export const sidebar = (page: Page): Locator => page.getByTestId('sidebar');
export const strip = (page: Page): Locator => page.getByTestId('sidebar-strip');
export const topBar = (page: Page): Locator => page.getByTestId('top-bar');
export const pane = (page: Page): Locator => page.locator('main');
export const scrim = (page: Page): Locator => page.getByTestId('sidebar-scrim');

/** The first board of the seed, which the root address shows. */
export const FIRST_BOARD = 'Project schedule';
/** The board of the seed the reader can only read. */
export const READ_ONLY_BOARD = 'Study notes';

/** Opens the app and waits for the board; fails on anything the page throws while it loads. */
export async function openApp(page: Page, address = '/'): Promise<void> {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(address);
  await expect(page.getByTestId('board-page')).toBeVisible();
  // The address has settled on the first view of the board.
  await expect(page).toHaveURL(SETTLED);
  expect(errors).toEqual([]);
}

export async function stageOf(page: Page): Promise<Stage> {
  return (await shell(page).getAttribute('data-stage')) as Stage;
}

/** How the list of boards stands: `fixed` beside the board, out as a `drawer`, or `closed`. */
export async function sidebarState(page: Page): Promise<string | null> {
  return shell(page).getAttribute('data-sidebar');
}

/** Brings the list of boards into view: nothing to do where it is fixed, the drawer elsewhere. */
export async function showSidebar(page: Page): Promise<void> {
  const state = await sidebarState(page);
  if (state === 'closed') {
    const stage = await stageOf(page);
    await page.getByTestId(stage === 'compact' ? 'open-sidebar' : 'expand-sidebar').click();
  }
  await expect(shell(page)).not.toHaveAttribute('data-sidebar', 'closed');
  await expect(sidebar(page).getByTestId('sidebar-search')).toBeVisible();
  // A drawer slides in: what is in it is where it will be once it has arrived.
  await expect.poll(async () => (await sidebar(page).boundingBox())?.x).toBe(0);
}

/** Closes the drawer, if the list is one. */
export async function hideSidebar(page: Page): Promise<void> {
  if ((await sidebarState(page)) !== 'drawer') return;
  await sidebar(page).getByTestId('close-sidebar').click();
  await expect(shell(page)).toHaveAttribute('data-sidebar', 'closed');
}

/** The link of a board in the list. */
export const boardLink = (page: Page, title: string): Locator =>
  sidebar(page).getByRole('link', { name: title, exact: true });

/** A category of the list, by its name: the box with its heading and its boards. */
export function category(page: Page, name: string): Locator {
  const exactly = new RegExp(`^\\s*${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\s*$`);
  return sidebar(page)
    .locator('[data-category]')
    .filter({ has: page.locator('[data-category-toggle]', { hasText: exactly }) });
}

/** The heading of a category: the button that folds and unfolds it. */
export const categoryToggle = (page: Page, name: string): Locator =>
  category(page, name).locator('[data-category-toggle]');

/** The titles of the boards a category shows, top to bottom. */
export async function boardsIn(page: Page, name: string): Promise<string[]> {
  return category(page, name)
    .getByRole('link')
    .evaluateAll((links) =>
      // The title is the last part of a link; before it stands the icon.
      links.map((link) => (link.lastElementChild?.textContent ?? '').trim()),
    );
}

/** The names of the categories, top to bottom. */
export async function categories(page: Page): Promise<string[]> {
  return sidebar(page)
    .locator('[data-category-toggle]')
    .evaluateAll((buttons) => buttons.map((button) => button.textContent.trim()));
}

/** Unfolds a part of a panel that folds (`FoldHeading`) by its button, unless it shows already. */
export async function unfold(toggle: Locator): Promise<void> {
  await expect(async () => {
    if ((await toggle.getAttribute('aria-expanded')) !== 'true') await toggle.click();
    await expect(toggle).toHaveAttribute('aria-expanded', 'true', { timeout: 1000 });
  }).toPass();
}

/** Opens the "…" menu of a board or a category and presses one of its lines. */
export async function chooseFromMenu(page: Page, menu: Locator, action: string): Promise<void> {
  await menu.click();
  await page.locator(`[data-action="${action}"]`).click();
}

export const boardMenu = (page: Page, title: string): Locator =>
  sidebar(page).getByRole('button', { name: `Menu of the board ${title}`, exact: true });

export const categoryMenu = (page: Page, name: string): Locator =>
  sidebar(page).getByRole('button', { name: `Menu of the category ${name}`, exact: true });

/** What shows of undo in the top bar: its own button, or on a phone the bar's menu that holds it. */
export const undoInBar = (page: Page): Locator =>
  topBar(page)
    .getByTestId(/^(undo|bar-menu)$/)
    .filter({ visible: true });

/** The buttons of the bar whose test identifiers are not the names of their lines in "…". */
const BAR_BUTTONS: Partial<Record<string, string>> = {
  'card-trash': 'open-card-trash',
  share: 'share-board',
};

/**
 * Undo, redo, the deleted cards or sharing, from the top bar: their own buttons where the pane
 * has room for them, else the lines of the bar's menu ("…"), as on a phone.
 */

export async function pressInBar(
  page: Page,
  command: 'undo' | 'redo' | 'card-trash' | 'share',
): Promise<void> {
  const button = topBar(page).getByTestId(BAR_BUTTONS[command] ?? command);
  if (await button.isVisible()) {
    await button.click();
    return;
  }
  await chooseFromMenu(page, topBar(page).getByTestId('bar-menu'), command);
}

export async function nextFrame(page: Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise((resolve) => {
        requestAnimationFrame(resolve);
      }),
  );
}

export interface Point {
  x: number;
  y: number;
}

/** A point on an element as it lies now: a third across, `down` of the way from its top. */
export async function pointOn(locator: Locator, down = 0.5): Promise<Point> {
  const box = await locator.boundingBox();
  if (!box) throw new Error('the element is not on the page');
  return { x: box.x + box.width / 3, y: box.y + box.height * down };
}

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
}

/**
 * What of the head is cut or spills: the name of the view on the button of
 * the views if it is cut, and how far the header and the top bar scroll
 * sideways.
 */
export async function headCuts(page: Page): Promise<{ cut: string[]; scroll: number }> {
  return page.evaluate(() => {
    const cut: string[] = [];
    const clipped = (each: Element): boolean => each.scrollWidth > each.clientWidth;
    const switcher = document.querySelector('[data-testid="view-header"] [data-tool="views"]');
    if (switcher && [...switcher.querySelectorAll('span')].some(clipped)) {
      cut.push(`${switcher.textContent.trim()} (on the button)`);
    }
    let scroll = 0;
    for (const id of ['view-header', 'top-bar']) {
      const box = document.querySelector(`[data-testid="${id}"]`);
      if (box) scroll = Math.max(scroll, box.scrollWidth - box.clientWidth);
    }
    return { cut, scroll };
  });
}
