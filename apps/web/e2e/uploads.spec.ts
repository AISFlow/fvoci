/**
 * Uploads in the stack at the bottom right of the page (features/shell/Transfers.svelte),
 * on the backend in memory: a file attached to a card shows how far it is, then that it is
 * in, a moment; it goes on when the card is closed; closing its card in the stack asks,
 * then gives it up; a failure stays with why, and can be sent again; many at once are one
 * card. The stack is over the card's dialog, which does not close when it is pressed, and
 * clear of the comment field; below 40rem it steps aside while the reader writes, and F8
 * shows it again. The moments it waits are the page's clock's, run on by the test.
 *
 * With `FVOCI_SCREENS=1` it also takes the pictures of the stack into
 * `target/web-pair/screens/`.
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';

interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  card(boardId: string, cardId: string): Promise<unknown>;
  failNext(method: string, error: unknown): void;
  latency: number;
}
interface Dev {
  backend: DevBackend;
}

/** Opens the card everything is tried on, by its address; its identifier. */
async function openTaskCard(page: Page): Promise<string> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title.startsWith('Project schedule'));
    if (!board) throw new Error('no board of tasks');
    const views = await backend.views(board.id);
    const cards = await backend.cards(board.id);
    const card = cards.find((each) => each.title === 'Design the screens');
    if (!card) throw new Error('no card');
    const first = [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
    return { boardId: board.id, viewId: first?.id ?? '', cardId: card.id };
  });
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('card-comments')).toBeVisible();
  return place.cardId;
}

async function latency(page: Page, milliseconds: number): Promise<void> {
  await page.evaluate((ms) => {
    (window as unknown as { fvociDev: Dev }).fvociDev.backend.latency = ms;
  }, milliseconds);
}

/** Has the next upload fail with the backend's error of `status`, and `body`. */
async function failNextUpload(page: Page, status: number, body: object): Promise<void> {
  await page.evaluate(
    async ([code, details]) => {
      const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
      const refusal = await backend.card('nowhere', 'nothing').catch((error: unknown) => error);
      const BackendError = (
        refusal as { constructor: new (status: number, body: object) => unknown }
      ).constructor;
      backend.failNext('upload', new BackendError(code, details));
    },
    [status, body] as const,
  );
}

const MIB = 1024 * 1024;
const file = (name: string, size = 64): { name: string; mimeType: string; buffer: Buffer } => ({
  name,
  mimeType: 'application/octet-stream',
  buffer: Buffer.alloc(size, 7),
});

const stack = (page: Page): Locator => page.getByTestId('transfers');
const cardOf = (page: Page, name: string): Locator =>
  stack(page).locator(`[data-transfer="upload"][data-name="${name}"]`);
/** What the screen reader was told last: each saying is a line of the stack's log. */
const said = (page: Page): Locator => page.getByTestId('transfers-said').locator('p').last();

/** Whether two boxes share any pixel. */
const overlap = (
  a: { x: number; y: number; width: number; height: number },
  b: { x: number; y: number; width: number; height: number },
): boolean =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

/** A phone's window is 390 pixels across, the narrowest the stack is measured at. */
async function phoneWidth(page: Page, project: string): Promise<void> {
  if (project === 'mobile') await page.setViewportSize({ width: 390, height: 844 });
}

test('a file attached is told in the stack: how far it is, then that it is in, a moment', async ({
  page,
}, info) => {
  await phoneWidth(page, info.project.name);
  await openTaskCard(page);
  // Two parts (8 MiB, then 1), each a while on its way.
  await latency(page, 600);
  await page.getByTestId('attach-input').setInputFiles([file('scan.bin', 9 * MIB)]);
  const card = cardOf(page, 'scan.bin');
  await expect(card).toContainText('Uploading scan.bin');
  await expect(card.getByRole('progressbar')).toBeVisible();
  await expect(said(page)).toHaveText('Uploading scan.bin');
  // The same upload has its place in the card's list.
  await expect(page.getByTestId('card-uploads').locator('[data-upload="scan.bin"]')).toBeVisible();
  // How much has gone shows, where there is room for it; it is never said.
  if ((page.viewportSize()?.width ?? 0) >= 640) {
    await expect(card).toContainText(/8\.0 MiB of 9\.0 MiB/);
  }
  await expect(said(page)).toHaveText('Uploading scan.bin');

  // Clear of the comment field, and over the dialog, which a press here does not close.
  const box = await stack(page).boundingBox();
  const composer = await page.getByTestId('comment-composer').boundingBox();
  if (!box || !composer) throw new Error('not laid out');
  expect(overlap(box, composer)).toBe(false);
  const center = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  const topmost = await page.evaluate(
    ({ x, y }) => Boolean(document.elementFromPoint(x, y)?.closest('[data-transfers]')),
    center,
  );
  expect(topmost).toBe(true);
  if ((page.viewportSize()?.width ?? 0) < 640) {
    // In a narrow window it steps aside while the reader writes.
    await page.getByTestId('comment-field').focus();
    await expect(stack(page)).toBeHidden();
    await page.getByTestId('comment-field').blur();
    await expect(stack(page)).toBeVisible();
  }

  await latency(page, 40);
  await expect(card).toHaveAttribute('data-finished', '');
  await expect(card).toContainText('scan.bin is uploaded.');
  await expect(said(page)).toHaveText('scan.bin is uploaded.');
  await expect(page.getByTestId('card-attachments').locator('[data-attachment]')).toContainText([
    'scan.bin',
  ]);
  await expect(page.getByTestId('card-uploads').locator('[data-upload]')).toHaveCount(0);
  await expect(card).toHaveCount(0, { timeout: 10_000 });
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.locator('[data-problem]')).toHaveCount(0);
});

test('a finished card stays while the pointer is over the stack', async ({ page }) => {
  test.skip((page.viewportSize()?.width ?? 0) < 640, 'a pointer that rests is a wide window’s');
  await page.clock.install();
  await openTaskCard(page);
  await page.getByTestId('attach-input').setInputFiles([file('kept.txt')]);
  const card = cardOf(page, 'kept.txt');
  await expect(card).toHaveAttribute('data-finished', '');
  await card.hover();
  // Longer than a finished one stays (TRANSFER_STAYS).
  await page.clock.runFor(7000);
  await expect(card).toBeVisible();
  // Let go of, it stays its whole moment from then.
  await page.mouse.move(5, 5);
  await page.clock.runFor(5000);
  await expect(card).toBeVisible();
  await page.clock.runFor(1000);
  await expect(card).toHaveCount(0);
});

test('an upload goes on when its card is closed, and is attached', async ({ page }) => {
  const cardId = await openTaskCard(page);
  await latency(page, 700);
  await page.getByTestId('attach-input').setInputFiles([file('later.txt')]);
  const card = cardOf(page, 'later.txt');
  await expect(card).toContainText('Uploading later.txt');
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toHaveCount(0);
  await expect(card).toBeVisible();
  await expect(card).toHaveAttribute('data-finished', '', { timeout: 10_000 });
  await latency(page, 40);
  // The card again, in the same page: the backend in memory is the page's.
  await page.locator(`[data-card="${cardId}"]`).first().click();
  await expect(page.getByTestId('card-attachments').locator('[data-attachment]')).toContainText([
    'later.txt',
  ]);
});

test('closing an upload under way asks first; given up, nothing is attached', async ({ page }) => {
  await page.clock.install();
  await openTaskCard(page);
  const attached = page.getByTestId('card-attachments').locator('[data-attachment]');
  const before = await attached.count();
  await latency(page, 1500);
  await page.getByTestId('attach-input').setInputFiles([file('maybe.txt')]);
  const card = cardOf(page, 'maybe.txt');
  await card.getByRole('button', { name: 'Stop maybe.txt' }).click();
  // Pressed over the card's dialog: the dialog stays.
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(card).toContainText('Stop it? What went is thrown away.');
  await card.getByRole('button', { name: 'Keep going' }).click();
  await expect(card.getByRole('progressbar')).toBeVisible();

  await card.getByRole('button', { name: 'Stop maybe.txt' }).click();
  await card.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(card).toHaveCount(0);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await latency(page, 40);
  // Past the moment the upload under way would have been in, and attached.
  await page.clock.runFor(2000);
  await expect(attached).toHaveCount(before);
  await expect(said(page)).toHaveText('Uploading maybe.txt');
});

test('the keyboard reaches the stack over the card’s dialog by F8; Escape goes back, the dialog stays', async ({
  page,
}) => {
  await openTaskCard(page);
  const attached = page.getByTestId('card-attachments').locator('[data-attachment]');
  const before = await attached.count();
  await latency(page, 1500);
  await page.getByTestId('attach-input').setInputFiles([file('keys.txt')]);
  const card = cardOf(page, 'keys.txt');
  await expect(card).toBeVisible();
  const field = page.getByTestId('comment-field');
  await field.focus();

  await page.keyboard.press('F8');
  const close = card.getByRole('button', { name: 'Stop keys.txt' });
  await expect(close).toBeFocused();
  await page.keyboard.press('Enter');
  // The question takes the focus, and the dialog's trap does not take it back.
  await expect(card.getByRole('button', { name: 'Keep going' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(close).toBeFocused();
  await expect(card.getByRole('progressbar')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(field).toBeFocused();
  await expect(page.getByTestId('card-dialog')).toBeVisible();

  // Stopped with the keyboard alone.
  await page.keyboard.press('F8');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Shift+Tab');
  await expect(card.getByRole('button', { name: 'Stop', exact: true })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(card).toHaveCount(0);
  // The focus is not left on the page, from where Escape would close the dialog: it is back.
  await expect(field).toBeFocused();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await latency(page, 40);
  await expect(page.getByTestId('card-uploads').locator('[data-upload]')).toHaveCount(0);
  await expect(attached).toHaveCount(before);

  // A failure dismissed with the keyboard alone: the focus is back too.
  await failNextUpload(page, 0, { detail: 'no answer' });
  await page.getByTestId('attach-input').setInputFiles([file('lost.txt')]);
  const lost = cardOf(page, 'lost.txt');
  await expect(lost).toHaveAttribute('data-failed', 'network');
  await field.focus();
  await page.keyboard.press('F8');
  await expect(lost.getByRole('button', { name: 'Dismiss lost.txt' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(lost).toHaveCount(0);
  await expect(field).toBeFocused();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
});

test('a sheet from the bottom of a narrow window is not covered by the stack', async ({
  page,
}, info) => {
  await phoneWidth(page, info.project.name);
  test.skip(
    info.project.name !== 'mobile',
    'the card’s menu is a sheet from the bottom on a phone',
  );
  await openTaskCard(page);
  await failNextUpload(page, 0, { detail: 'no answer' });
  await page.getByTestId('attach-input').setInputFiles([file('stays.txt')]);
  const card = cardOf(page, 'stays.txt');
  await expect(card).toHaveAttribute('data-failed', 'network');
  await page.getByTestId('card-menu').click();
  await expect(page.getByTestId('bottom-sheet')).toBeVisible();
  await expect(stack(page)).toBeHidden();
  // F8 shows it while the focus is in it, and Escape takes the focus back to the sheet.
  const line = page.getByTestId('bottom-sheet').locator('[role="menuitem"]:enabled').first();
  await line.focus();
  await page.keyboard.press('F8');
  await expect(stack(page)).toBeVisible();
  await expect(card.getByRole('button', { name: 'Dismiss stays.txt' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(line).toBeFocused();
  await expect(stack(page)).toBeHidden();
  await expect(page.getByTestId('bottom-sheet')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('bottom-sheet')).toHaveCount(0);
  await expect(card).toBeVisible();

  // Aside while the reader writes, F8 shows it too.
  const field = page.getByTestId('comment-field');
  await field.focus();
  await expect(stack(page)).toBeHidden();
  await page.keyboard.press('F8');
  await expect(card.getByRole('button', { name: 'Dismiss stays.txt' })).toBeFocused();
  await expect(stack(page)).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(field).toBeFocused();
});

test('a failed upload stays with why until dismissed, and can be sent again', async ({ page }) => {
  await page.clock.install();
  await openTaskCard(page);
  await failNextUpload(page, 0, { detail: 'no answer' });
  await page.getByTestId('attach-input').setInputFiles([file('flaky.txt')]);
  const card = cardOf(page, 'flaky.txt');
  await expect(card).toHaveAttribute('data-failed', 'network');
  await expect(card).toContainText('flaky.txt was not uploaded.');
  await expect(card).toContainText('The connection dropped.');
  await expect(said(page)).toHaveText('flaky.txt was not uploaded. The connection dropped.');
  // Told here, and not again where problems are.
  await expect(page.locator('[data-problem]')).toHaveCount(0);
  // Longer than a finished one stays (TRANSFER_STAYS).
  await page.clock.runFor(7000);
  await expect(card).toBeVisible();
  await card.getByRole('button', { name: 'Upload flaky.txt again' }).click();
  await expect(card).toHaveAttribute('data-finished', '');
  // Let go of, by the pointer and the focus.
  await page.mouse.move(5, 5);
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
  });
  await expect(page.getByTestId('card-attachments').locator('[data-attachment]')).toContainText([
    'flaky.txt',
  ]);

  // Gone a moment after, before the next: two at once are one card in a narrow window.
  await page.clock.runFor(6000);
  await expect(card).toHaveCount(0);
  await failNextUpload(page, 413, { limit: 1024 });
  await page.getByTestId('attach-input').setInputFiles([file('big.bin', 2048)]);
  const big = cardOf(page, 'big.bin');
  await expect(big).toHaveAttribute('data-failed', 'too-large');
  await expect(big).toContainText(/It is larger than 1(\.0)? KiB/);
  // Sending the same file again would be refused again: it is not offered.
  await expect(big.getByRole('button', { name: 'Upload big.bin again' })).toHaveCount(0);
  await big.getByRole('button', { name: 'Dismiss big.bin' }).click();
  await expect(big).toHaveCount(0);
  // Dismissed in the stack, it is gone from the card's list too.
  await expect(page.getByTestId('card-uploads').locator('[data-upload]')).toHaveCount(0);
});

test('many files at once are one card, with each behind a button', async ({ page }) => {
  await openTaskCard(page);
  const wide = (page.viewportSize()?.width ?? 0) >= 640;
  const names = wide ? ['1.txt', '2.txt', '3.txt', '4.txt', '5.txt'] : ['1.txt', '2.txt'];
  await latency(page, 1500);
  await page.getByTestId('attach-input').setInputFiles(names.map((name) => file(name)));
  const group = stack(page).locator('[data-transfer-group]');
  await expect(group).toHaveAttribute('data-transfer-group', String(names.length));
  await expect(group).toContainText(`Uploading ${String(names.length)} files`);
  await expect(stack(page).locator('[data-transfer]')).toHaveCount(0);
  // Each start is said, though they started at once.
  await expect(page.getByTestId('transfers-said').locator('p')).toHaveText(
    names.map((name) => `Uploading ${name}`),
  );
  await group.getByRole('button', { name: 'Each file' }).click();
  await expect(group.locator('[data-transfer="upload"]')).toHaveCount(names.length);
  await latency(page, 40);
  await expect(group).toContainText(`${String(names.length)} files are uploaded.`);
  // The stack never covers the comment field, however many.
  const box = await stack(page).boundingBox();
  const composer = await page.getByTestId('comment-composer').boundingBox();
  if (!box || !composer) throw new Error('not laid out');
  expect(overlap(box, composer)).toBe(false);
});

if (process.env.FVOCI_SCREENS) {
  const SCREENS = path.resolve(import.meta.dirname, '../../../target/web-pair/screens');

  for (const scheme of ['light', 'dark'] as const) {
    test(`pictures of the stack, ${scheme}`, async ({ page }, info) => {
      await page.emulateMedia({ colorScheme: scheme });
      await phoneWidth(page, info.project.name);
      await openTaskCard(page);
      const width = String(page.viewportSize()?.width ?? 0);
      const shot = async (state: string): Promise<void> => {
        await page.screenshot({
          path: path.join(SCREENS, `uploads-${state}-${width}-${scheme}.png`),
        });
      };
      await latency(page, 1200);
      await page.getByTestId('attach-input').setInputFiles([file('회의록 2026-11.hwp', 9 * MIB)]);
      await expect(cardOf(page, '회의록 2026-11.hwp').getByRole('progressbar')).toBeVisible();
      await page.waitForTimeout(1500);
      await shot('progress');
      await latency(page, 40);
      await expect(cardOf(page, '회의록 2026-11.hwp')).toHaveAttribute('data-finished', '');
      await shot('done');
      await cardOf(page, '회의록 2026-11.hwp')
        .getByRole('button', { name: 'Dismiss 회의록 2026-11.hwp' })
        .click();
      await failNextUpload(page, 413, { limit: 1024 });
      await page.getByTestId('attach-input').setInputFiles([file('too-big.pdf', 2048)]);
      await expect(cardOf(page, 'too-big.pdf')).toHaveAttribute('data-failed', 'too-large');
      await shot('failed');
      await latency(page, 1500);
      await page
        .getByTestId('attach-input')
        .setInputFiles(['a.png', 'b.png', 'c.png', 'd.png', 'e.png'].map((name) => file(name)));
      await expect(stack(page).locator('[data-transfer-group]')).toBeVisible();
      await shot('many');
    });
  }
}
