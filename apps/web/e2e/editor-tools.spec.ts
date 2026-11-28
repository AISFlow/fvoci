/**
 * The tools of a body being written, on the app with its in-memory backend:
 * "/" offers the kinds of block and finds them by what is typed, Korean too;
 * Markdown pasted as text is read as Markdown; chosen text gets a bubble of
 * marks, inside a card's transformed dialog as well; a phone gets a bar over
 * its keyboard instead; and an image opens larger, its Escape closing only it.
 *
 * The desktop runs 1440 pixels across, the phone 390. `FVOCI_SCREENS=1` also
 * takes the pictures of each tool into `target/screens/editor-tools/`
 * (`e2e/screens.ts`).
 */
import path from 'node:path';
import { crc32, deflateSync } from 'node:zlib';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import { screensDir } from './screens.ts';
import { openApp } from './shell.support.ts';
import { paste, slash, slashMenu } from './tables.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

const SIZES: Record<string, { width: number; height: number }> = {
  desktop: { width: 1440, height: 900 },
  mobile: { width: 390, height: 844 },
};

test.beforeEach(async ({ page }, info) => {
  const size = SIZES[info.project.name];
  if (size) await page.setViewportSize(size);
});

/** Takes a picture of the page, with `FVOCI_SCREENS=1`, named by what it shows and the width. */
async function picture(page: Page, name: string): Promise<void> {
  if (process.env.FVOCI_SCREENS !== '1') return;
  const width = page.viewportSize()?.width ?? 0;
  await page.screenshot({
    path: path.join(screensDir('editor-tools'), `${name}-${String(width)}.png`),
    animations: 'disabled',
  });
}

/** A PNG of `width` × `height` pixels, a gradient: something to look at. */
function png(width: number, height: number): Buffer {
  const chunk = (type: string, data: Buffer): Buffer => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
    const sum = Buffer.alloc(4);
    sum.writeUInt32BE(crc32(body) >>> 0);
    return Buffer.concat([length, body, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  const rows: Buffer[] = [];
  for (let y = 0; y < height; y += 1) {
    const row = Buffer.alloc(1 + width * 3);
    for (let x = 0; x < width; x += 1) {
      row[1 + x * 3] = 60 + Math.floor((x / width) * 160);
      row[2 + x * 3] = 110 + Math.floor((y / height) * 100);
      row[3 + x * 3] = 210;
    }
    rows.push(row);
  }
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(Buffer.concat(rows))),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/** A new wiki's first page, named, its body ready to be written. */
async function newPage(page: Page): Promise<Locator> {
  await openApp(page);
  await newWiki(page, 'Notes');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Week');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  return pageBody(page);
}

/** Opens a card of the first board by its title, and gives its body's editor. */
async function openCard(page: Page, title: string): Promise<Locator> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async (cardTitle) => {
    const { backend } = window.fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title === 'Project schedule');
    if (!board) throw new Error('no board');
    const [view] = await backend.views(board.id);
    const card = (await backend.cards(board.id)).find((each) => each.title === cardTitle);
    if (!card) throw new Error(`no card ${cardTitle}`);
    return { boardId: board.id, viewId: view?.id ?? '', cardId: card.id };
  }, title);
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  const body = page.getByTestId('card-body').locator('[contenteditable="true"]').first();
  await expect(body).toBeVisible();
  return body;
}

/** Writes a new last line of a body and gives back the line. */
async function writeLine(page: Page, body: Locator, words: string): Promise<Locator> {
  await body.click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.type(words);
  return body.locator('p', { hasText: words }).last();
}

const bubble = (page: Page): Locator => page.locator('.doc-bubble');
const lightbox = (page: Page): Locator => page.getByTestId('image-lightbox');

test('"/" offers the kinds of block, found by what is typed, Korean too', async ({ page }) => {
  const body = await newPage(page);
  await body.click();
  await page.keyboard.type('/');
  await expect(slashMenu(page).getByRole('option')).toHaveText([
    'Heading 1',
    'Heading 2',
    'Heading 3',
    'To-do list',
    'Bulleted list',
    'Numbered list',
    'Quote',
    'Toggle',
    'Callout',
    'Code block',
    'Formula block',
    'Divider',
    'Table',
    'Board view',
    'File or image',
  ]);
  await picture(page, 'slash-menu');
  await page.keyboard.press('Escape');
  await page.keyboard.press('Backspace');

  // By the Korean of its name, and by its first consonants.
  await page.keyboard.type('/제목');
  await expect(slashMenu(page).getByRole('option')).toHaveText([
    'Heading 1',
    'Heading 2',
    'Heading 3',
  ]);
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(slashMenu(page)).toHaveCount(0);
  await page.keyboard.type('Plan');
  await page.keyboard.press('Enter');
  await slash(page, 'ㅎㅇ', 'To-do list');
  await page.keyboard.type('Ship');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await slash(page, 'divider', 'Divider');
  await slash(page, 'quote', 'Quote');
  await page.keyboard.type('Said');

  await expect(body.locator('h2')).toHaveText('Plan');
  await expect(body.locator('ul[data-type="taskList"] > li > div')).toHaveText(['Ship']);
  await expect(body.locator('hr')).toHaveCount(1);
  await expect(body.locator('blockquote')).toHaveText('Said');
});

test('"/" opens a formula on its own line', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop', 'chosen with the keys of a desktop');
  const body = await newPage(page);
  await body.click();
  await slash(page, '수식', 'Formula block');
  const field = page.locator('.doc-place textarea');
  await expect(field).toBeFocused();
  await field.fill('a^2 + b^2');
  await page.keyboard.press('ControlOrMeta+Enter');
  await expect(body.locator('.doc-math-block')).toHaveCount(1);
});

test('Markdown pasted as text is read as Markdown, a line alone as text', async ({ page }) => {
  const body = await newPage(page);
  await body.click();
  await paste(body, {
    'text/plain': '# Plan\n\n- [ ] Draft\n- [x] Review\n\n> Keep it short\n\n1. one\n2. two',
  });
  await expect(body.locator('h1')).toHaveText('Plan');
  await expect(body.locator('ul[data-type="taskList"] > li > div')).toHaveText(['Draft', 'Review']);
  await expect(body.locator('ul[data-type="taskList"] > li[data-checked="true"] > div')).toHaveText(
    'Review',
  );
  await expect(body.locator('blockquote')).toHaveText('Keep it short');
  await expect(body.locator('ol > li')).toHaveText(['one', 'two']);

  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await paste(body, { 'text/plain': '# not a heading' });
  await expect(body.locator('h1')).toHaveCount(1);
  await expect(body.locator('p', { hasText: '# not a heading' })).toHaveCount(1);
});

test('chosen text gets a bubble of marks in a card, over it or under it on a touch screen', async ({
  page,
}, info) => {
  test.skip(info.project.name === 'mobile', 'a phone has the bar over its keyboard');
  const body = await openCard(page, 'Agree on the scope');
  const line = await writeLine(page, body, 'Ship the plan');
  for (let at = 0; at < 4; at += 1) await page.keyboard.press('Shift+ArrowLeft');
  await expect(bubble(page)).toBeVisible();
  await expect(bubble(page)).toHaveAttribute(
    'data-side',
    info.project.name === 'middle' ? 'bottom' : 'top',
  );
  // In sight, and next to the text chosen.
  const [shown, chosen, seen] = [
    await bubble(page).boundingBox(),
    await line.boundingBox(),
    page.viewportSize(),
  ];
  if (!shown || !chosen || !seen) throw new Error('nothing to measure');
  expect(shown.x).toBeGreaterThanOrEqual(0);
  expect(shown.x + shown.width).toBeLessThanOrEqual(seen.width);
  expect(Math.abs(shown.y + shown.height / 2 - (chosen.y + chosen.height / 2))).toBeLessThan(80);
  await picture(page, 'bubble');

  await bubble(page).getByRole('button', { name: 'Bold' }).click();
  await expect(line.locator('strong')).toHaveText('plan');
  await bubble(page).getByRole('button', { name: 'Link' }).click();
  await bubble(page).getByRole('textbox', { name: 'Link address' }).fill('example.com');
  await page.keyboard.press('Enter');
  await expect(line.locator('a')).toHaveAttribute('href', 'https://example.com');
  await expect(page.getByTestId('card-dialog')).toBeVisible();
});

test('a phone gets a bar over its keyboard, a finger wide', async ({ page }, info) => {
  test.skip(info.project.name !== 'mobile', 'a phone’s');
  const body = await openCard(page, 'Agree on the scope');
  const bar = page.getByTestId('mobile-editor-toolbar');
  await expect(bar).toHaveCount(0);
  await writeLine(page, body, 'Buy milk');
  await expect(bar).toBeVisible();
  const box = await bar.getByRole('button', { name: 'To-do list' }).boundingBox();
  expect(box?.height).toBeGreaterThanOrEqual(44);
  expect(box?.width).toBeGreaterThanOrEqual(44);
  // In sight, and clear of the comment field pinned under the body.
  const barBox = await bar.boundingBox();
  const composer = await page.getByTestId('comment-composer').boundingBox();
  const barBottom = (barBox?.y ?? 0) + (barBox?.height ?? 0);
  expect(barBottom).toBeLessThanOrEqual(page.viewportSize()?.height ?? 0);
  expect(barBottom).toBeLessThanOrEqual((composer?.y ?? 0) + 1);
  await picture(page, 'mobile-toolbar');

  const toDo = body.locator('ul[data-type="taskList"] > li', { hasText: 'Buy milk' });
  await bar.getByRole('button', { name: 'To-do list' }).tap();
  await expect(toDo).toHaveCount(1);
  // The shared document's undo takes back what was done within a moment as one step:
  // the words typed just before may go with the list.
  await bar.getByRole('button', { name: 'Undo' }).tap();
  await expect(toDo).toHaveCount(0);
  await bar.getByRole('button', { name: 'Redo' }).tap();
  await expect(toDo).toHaveCount(1);
  // The marks are on the bar: chosen text gets no bubble of them.
  await page.keyboard.press('Shift+ArrowLeft');
  await expect(bubble(page)).toHaveCount(0);
  // With a keyboard, Tab stops in the bar once, the arrows go along it, and Tab goes on past it;
  // from a line of text, since in a list Tab moves the line in.
  await bar.getByRole('button', { name: 'Undo' }).tap();
  await expect(toDo).toHaveCount(0);
  await page.keyboard.press('Tab');
  const focusedButton = bar.locator('button:focus');
  await expect(focusedButton).toHaveCount(1);
  const first = await focusedButton.getAttribute('data-format');
  await page.keyboard.press('ArrowRight');
  await expect(focusedButton).not.toHaveAttribute('data-format', first ?? '');
  await page.keyboard.press('Tab');
  await expect(bar).toHaveCount(0);
});

test('an image opens larger from its bubble, and Escape closes only it', async ({ page }) => {
  const body = await openCard(page, 'Agree on the scope');
  await writeLine(page, body, 'Above:');
  await page.keyboard.press('Enter');
  await page.keyboard.type('/image');
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.keyboard.press('Enter'),
  ]);
  await chooser.setFiles({ name: 'chart.png', mimeType: 'image/png', buffer: png(640, 400) });
  const image = body.locator('img[data-file-id][src]');
  await expect(image).toBeVisible();
  await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(640);

  await image.click();
  const zoom = bubble(page).getByRole('button', { name: 'View larger' });
  await expect(zoom).toBeVisible();
  await picture(page, 'image-bubble');
  await zoom.click();
  await expect(lightbox(page)).toBeVisible();
  await expect(lightbox(page).locator('img')).toHaveAttribute(
    'src',
    (await image.getAttribute('src')) ?? '',
  );
  await picture(page, 'lightbox');
  await page.keyboard.press('Escape');
  await expect(lightbox(page)).toHaveCount(0);
  await expect(page.getByTestId('card-dialog')).toBeVisible();

  // Beside the image, and its close button. Each press of the image is at another point of it:
  // two within half a second at one point are a double click to ProseMirror, which chooses no image.
  await image.click({ position: { x: 20, y: 20 } });
  await zoom.click();
  await lightbox(page).click({ position: { x: 4, y: 200 } });
  await expect(lightbox(page)).toHaveCount(0);
  await image.click({ position: { x: 60, y: 60 } });
  await zoom.click();
  await lightbox(page).getByRole('button', { name: 'Close' }).click();
  await expect(lightbox(page)).toHaveCount(0);
  await expect(page.getByTestId('card-dialog')).toBeVisible();
});
