/**
 * Undo and redo while a write is still under way, on a backend that answers
 * 400ms late: Ctrl+Z right after a change takes back that change, not the
 * one before it; a change the backend refuses leaves the undo that waited
 * for it with nothing to do; redo follows the same rule. The change is the
 * board's title, written from the top bar.
 *
 * So that the keys are surely pressed while the write is under way, also on
 * a machine too busy to press within 400ms, the test holds that write in
 * the page until it has pressed them (`holdNextWrite`); it then waits its
 * 400ms as any other.
 */
import { expect, test, type Page } from '@playwright/test';

import { primaryKey } from './keyboard.support.ts';
import { FIRST_BOARD, openApp, topBar } from './shell.support.ts';

interface Dev {
  fvociDev: {
    backend: {
      latency: number;
      patchContainer: (...args: unknown[]) => Promise<unknown>;
      failNext: (method: string, error: Error) => void;
      world: { containers: Map<string, { title: string }> };
    };
    workspace: { board: { boardId: string } };
  };
}

/** What a press of Ctrl+Z or Ctrl+Shift+Z found: the title stored then, and whether undo said it was waiting. */
interface Pressed {
  stored: string;
  busy: boolean;
}

/** The board's title as the backend has it, read without its delay. */
function stored(page: Page): Promise<string> {
  return page.evaluate(() => {
    const { fvociDev } = window as unknown as Dev;
    return fvociDev.backend.world.containers.get(fvociDev.workspace.board.boardId)?.title ?? '';
  });
}

/**
 * Every following press of the keys of undo records what the backend had
 * stored at that moment, and whether the undo button said it was busy once
 * the app had handled the press: the listener comes after the app's own,
 * and reads the button after the page has been updated (a microtask queued
 * after the app's), before any write can come back.
 */
async function recordPresses(page: Page): Promise<() => Promise<Pressed[]>> {
  await page.evaluate(() => {
    const { fvociDev } = window as unknown as Dev;
    const presses: Pressed[] = [];
    (window as unknown as { presses: Pressed[] }).presses = presses;
    window.addEventListener('keydown', (event) => {
      if (event.key.toLowerCase() !== 'z' || !(event.ctrlKey || event.metaKey)) return;
      const board = fvociDev.backend.world.containers.get(fvociDev.workspace.board.boardId);
      const press = { stored: board?.title ?? '', busy: false };
      presses.push(press);
      queueMicrotask(() => {
        press.busy =
          document.querySelector('[data-testid="undo"]')?.getAttribute('aria-busy') === 'true';
      });
    });
  });
  return () => page.evaluate(() => (window as unknown as { presses: Pressed[] }).presses);
}

/** The board on a backend that answers 400ms late, its title changed once to "One" and stored. */
async function openSlow(page: Page): Promise<void> {
  await openApp(page);
  await page.evaluate(() => {
    (window as unknown as Dev).fvociDev.backend.latency = 400;
  });
  const title = topBar(page).getByTestId('board-title');
  await title.fill('One');
  await title.press('Enter');
  await expect.poll(() => stored(page)).toBe('One');
}

/** Holds the next write of the board until `release` is called. */
async function holdNextWrite(page: Page): Promise<{ release: () => Promise<void> }> {
  await page.evaluate(() => {
    const { backend } = (window as unknown as Dev).fvociDev;
    const write = backend.patchContainer.bind(backend);
    const held = new Promise<void>((resolve) => {
      (window as unknown as { releaseWrite: () => void }).releaseWrite = resolve;
    });
    backend.patchContainer = async (...args: unknown[]) => {
      backend.patchContainer = write;
      await held;
      return write(...args);
    };
  });
  return {
    release: () =>
      page.evaluate(() => {
        (window as unknown as { releaseWrite: () => void }).releaseWrite();
      }),
  };
}

function refuseNextWrite(page: Page): Promise<void> {
  return page.evaluate(() => {
    (window as unknown as Dev).fvociDev.backend.failNext(
      'patchContainer',
      new Error('refused by the test'),
    );
  });
}

test('Ctrl+Z right after a change takes back that change, and Ctrl+Shift+Z does it again', async ({
  page,
}) => {
  await openSlow(page);
  const title = topBar(page).getByTestId('board-title');
  const undo = topBar(page).getByTestId('undo');
  const redo = topBar(page).getByTestId('redo');
  const presses = await recordPresses(page);

  let write = await holdNextWrite(page);
  await title.fill('Two');
  await title.press('Enter');
  await undo.press(await primaryKey(page, 'z'));
  // Pressed while "Two" was still being written; undo said it was waiting.
  expect(await presses()).toEqual([{ stored: 'One', busy: true }]);
  await write.release();
  await expect(undo).not.toHaveAttribute('aria-busy', 'true');
  await expect.poll(() => stored(page)).toBe('One');
  // "One" is still in force, and is the next to undo.
  await expect(title).toHaveValue('One');
  await expect(undo).toBeEnabled();
  await expect(redo).toBeEnabled();

  // Undo "One", and redo it while the undo is still being written: it waits its turn.
  write = await holdNextWrite(page);
  await undo.press(await primaryKey(page, 'z'));
  await undo.press(await primaryKey(page, 'Shift+z'));
  expect((await presses())[2]).toEqual({ stored: 'One', busy: true });
  await write.release();
  await expect(undo).not.toHaveAttribute('aria-busy', 'true');
  await expect(title).toHaveValue('One');
  await expect.poll(() => stored(page)).toBe('One');
  // "Two" can still be done again.
  await expect(redo).toBeEnabled();

  // Undo "One": the first title of the board comes back, and nothing is left to undo.
  await undo.press(await primaryKey(page, 'z'));
  await expect(undo).toBeDisabled();
  await expect(title).toHaveValue(FIRST_BOARD);
  await expect.poll(() => stored(page)).toBe(FIRST_BOARD);
});

test('an undo that waited for a refused change does not touch the change before it', async ({
  page,
}) => {
  await openSlow(page);
  const title = topBar(page).getByTestId('board-title');
  const undo = topBar(page).getByTestId('undo');
  const redo = topBar(page).getByTestId('redo');
  const presses = await recordPresses(page);

  await refuseNextWrite(page);
  const write = await holdNextWrite(page);
  await title.fill('Two');
  await title.press('Enter');
  await undo.press(await primaryKey(page, 'z'));
  expect(await presses()).toEqual([{ stored: 'One', busy: true }]);
  await write.release();
  await expect(undo).not.toHaveAttribute('aria-busy', 'true');
  // "Two" never happened, and "One" was not taken back.
  await expect(title).toHaveValue('One');
  await expect.poll(() => stored(page)).toBe('One');
  await expect(undo).toBeEnabled();
  await expect(redo).toBeDisabled();
});

test('a redo that waited for a refused undo is dropped, and the change stays in force', async ({
  page,
}) => {
  await openSlow(page);
  const title = topBar(page).getByTestId('board-title');
  const undo = topBar(page).getByTestId('undo');
  const redo = topBar(page).getByTestId('redo');
  const presses = await recordPresses(page);

  await refuseNextWrite(page);
  const write = await holdNextWrite(page);
  await undo.press(await primaryKey(page, 'z'));
  await undo.press(await primaryKey(page, 'Shift+z'));
  // The redo was pressed while the undo still waited for its write.
  expect(await presses()).toEqual([
    { stored: 'One', busy: true },
    { stored: 'One', busy: true },
  ]);
  await write.release();
  await expect(undo).not.toHaveAttribute('aria-busy', 'true');
  await expect(title).toHaveValue('One');
  await expect.poll(() => stored(page)).toBe('One');
  // The undo failed: "One" is still the one to undo, and there is nothing to redo.
  await expect(undo).toBeEnabled();
  await expect(redo).toBeDisabled();
});
