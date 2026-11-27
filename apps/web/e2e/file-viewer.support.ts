/**
 * What the tests of the file viewer share: the card files are attached to,
 * the viewer and its frame, and the pictures of each state.
 *
 * With `FVOCI_SCREENS=1` the tests take pictures of each state (light and
 * dark) into `target/<folder>/`, at every size the project runs at (the
 * phone at 390 pixels).
 */
import path from 'node:path';

import {
  expect,
  test,
  type Frame,
  type FrameLocator,
  type Locator,
  type Page,
} from '@playwright/test';

import { boardPath } from './address.ts';

const SCREENS = process.env.FVOCI_SCREENS === '1';

interface Dev {
  backend: {
    workspaces(): Promise<{ id: string }[]>;
    boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
    views(boardId: string): Promise<{ id: string; title: string }[]>;
    cards(boardId: string): Promise<{ id: string; title: string }[]>;
  };
}

/** Opens the card everything is tried on, by its address. */
export async function openCard(page: Page): Promise<void> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title.startsWith('Project schedule'));
    if (!board) throw new Error('no board of tasks');
    const views = await backend.views(board.id);
    const card = (await backend.cards(board.id)).find(
      (each) => each.title === 'Design the screens',
    );
    if (!card) throw new Error('no card');
    const first = [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
    return { boardId: board.id, viewId: first?.id ?? '', cardId: card.id };
  });
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  await expect(page.getByTestId('card-attachments')).toBeVisible();
}

export async function attach(
  page: Page,
  name: string,
  buffer: Buffer,
  mimeType: string,
): Promise<Locator> {
  await page.getByTestId('attach-input').setInputFiles({ name, mimeType, buffer });
  const row = page.getByTestId('card-attachments').locator('[data-attachment]', { hasText: name });
  await expect(row).toBeVisible();
  return row;
}

export const viewer = (page: Page): Locator => page.getByTestId('file-viewer');
export const frameOf = (page: Page): FrameLocator =>
  page.frameLocator('[data-testid="file-viewer-frame"]');

/** Takes the pictures of the page as it is, into `target/<folder>/<name>-<project>-<theme>.png`. */
export async function picture(page: Page, folder: string, name: string): Promise<void> {
  if (!SCREENS) return;
  const project = test.info().project.name;
  // The phone pictures at the narrowest width the app is drawn for.
  if (project === 'mobile') await page.setViewportSize({ width: 390, height: 844 });
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    // The app has turned (its root's class) and nothing on it is still fading.
    await page.waitForFunction(
      (dark) =>
        document.documentElement.classList.contains('dark') === dark &&
        document.getAnimations().every((animation) => animation.playState !== 'running'),
      theme === 'dark',
    );
    const file = path.resolve(
      import.meta.dirname,
      '../../../target',
      folder,
      `${name}-${project}-${theme}.png`,
    );
    await page.screenshot({ path: file });
  }
  await page.emulateMedia({ colorScheme: null });
}

/**
 * What the frame reaches when it tries: the network (the app's address and
 * its own), the app's storage and cookies, and the app's page.
 */
export async function frameReaches(page: Page): Promise<Record<string, string>> {
  return viewerFrame(page).evaluate(async () => {
    const attempt = async (action: () => unknown): Promise<string> => {
      try {
        const result = await action();
        return `reached: ${String(result)}`;
      } catch (error) {
        return `refused: ${error instanceof Error ? error.name : String(error)}`;
      }
    };
    return {
      origin: self.origin,
      // `location.origin` is "null" here; the address is still the app's.
      fetchApp: await attempt(async () => (await fetch(new URL('/', location.href))).status),
      fetchStatic: await attempt(async () => (await fetch(location.href)).status),
      storage: await attempt(() => localStorage.getItem('fvoci:viewer-probe')),
      cookies: await attempt(() => document.cookie),
      parent: await attempt(() => window.parent.document.title),
    };
  });
}

/** The viewer's frame, to run code in. */
export function viewerFrame(page: Page): Frame {
  const inside = page.frames().find((each) => each.url().endsWith('/static/viewer-frame.html'));
  if (!inside) throw new Error('no viewer frame');
  return inside;
}

/**
 * Starts noting, on the app's page, each run of a timer every 20 ms (`ticks`
 * reads them), in milliseconds since 1970 (`performance.timeOrigin` plus
 * `performance.now()`), which the frame reads alike.
 */
export async function recordTicks(page: Page): Promise<void> {
  await page.evaluate(() => {
    const noted = window as unknown as { fvociTicks: number[] };
    noted.fvociTicks = [];
    setInterval(() => {
      noted.fvociTicks.push(performance.timeOrigin + performance.now());
    }, 20);
  });
}

export const ticks = (page: Page): Promise<number[]> =>
  page.evaluate(() => (window as unknown as { fvociTicks: number[] }).fvociTicks);

/**
 * Whether the viewer's frame runs on the app's page's thread, as the freeze
 * tests take it to (a browser may give a sandboxed frame a process of its
 * own, and then they would show nothing): the frame keeps its thread for
 * 300 ms while a timer of the page's is due.
 */
export async function sharesThread(page: Page): Promise<boolean> {
  const late = timerLateness(page);
  await viewerFrame(page).evaluate(() => {
    const until = performance.now() + 300;
    while (performance.now() < until) {
      // Holding the thread.
    }
  });
  return (await late) > 200;
}

/** How late a timer of 100 ms on the app's page fires, in milliseconds. */
export function timerLateness(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const start = performance.now();
        setTimeout(() => {
          resolve(performance.now() - start - 100);
        }, 100);
      }),
  );
}

/**
 * Waits for the app's idle-loaded CJK and monospace font sheets (`lib/fonts.ts`)
 * to settle on the page before a main-thread responsiveness check: WebKit has no
 * `requestIdleCallback` and loads them on a 1.5 s timer, decoding the 2.6 MB
 * Hanja subset (`noto-sans-kr-rest.woff2`) synchronously on the main thread when
 * Han characters on the card match.
 */
export async function settlePageFonts(page: Page): Promise<void> {
  const state = await page.waitForFunction(() => {
    const faces = [...document.fonts];
    // When fonts are not built (`pnpm run fonts` skipped), no Noto Sans rule exists.
    if (!faces.some((face) => face.family.includes('Noto Sans'))) return 'none';
    const required = ['Noto Sans KR', 'Noto Sans JP', 'Noto Sans SC', 'Noto Sans TC', 'D2Coding'];
    return required.every((name) => faces.some((face) => face.family.includes(name)))
      ? 'loaded'
      : false;
  });
  if ((await state.jsonValue()) === 'none') return;
  await page.evaluate(async () => {
    // Let a full style/layout/paint cycle match the newly added rules against text on screen.
    await new Promise<void>((resolve) => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          resolve();
        });
      });
    });
    while (document.fonts.status === 'loading') {
      await document.fonts.ready;
    }
    await document.fonts.ready;
  });
}
