/**
 * The file block of a page's body, on the backend in memory: a file chosen
 * through the slash menu, or dropped, that is no picture goes up and becomes
 * a block with its name and size, on one line however narrow the screen; its
 * link saves the same bytes under its name, and Enter opens a PDF in the
 * viewer (file-block-viewer.spec.ts). Files chosen together become blocks in
 * the order chosen.
 */
import { expect, test, type Download, type Locator, type Page } from '@playwright/test';

import { hideSidebar, openApp } from './shell.support.ts';
import { nameThePage, newWiki, pageAction, pageBody, showTree, treeItem } from './wiki.support.ts';

const PDF = '%PDF-1.4\n% a file block\n%%EOF\n';
const NAME = '2026-11 회의록 of the steering committee and of everybody else who came.pdf';

async function bytesOf(download: Download): Promise<string> {
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString();
}

/** A new wiki's first page, named, its body ready to be written. */
async function newPage(page: Page): Promise<Locator> {
  await openApp(page);
  await newWiki(page, 'Committee');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Minutes');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  return pageBody(page);
}

test('a file chosen through the slash menu is a file block that saves its bytes under its name', async ({
  page,
  browserName,
}) => {
  const body = await newPage(page);
  await body.click();
  await page.keyboard.type('Minutes: ');
  await page.keyboard.press('Enter');
  await page.keyboard.type('/file');
  const menu = page.locator('[data-suggestions="slash"]');
  await expect(menu.getByRole('option')).toHaveText(['File or image']);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.keyboard.press('Enter'),
  ]);
  await chooser.setFiles({ name: NAME, mimeType: 'application/pdf', buffer: Buffer.from(PDF) });

  const block = page.getByTestId('page-body').locator('[data-file-block]');
  await expect(block).toHaveAttribute('data-state', 'shown');
  await expect(block).toHaveAttribute('aria-label', `File: ${NAME}, ${PDF.length} B`);
  await expect(
    page.getByTestId('transfers').locator('[data-transfer="upload"]').filter({ hasText: NAME }),
  ).toHaveAttribute('data-finished', '');
  await expect(block.locator('.doc-file-name')).toHaveText(NAME);
  await expect(body).not.toContainText('/');

  const link = block.getByRole('link', { name: `Download ${NAME}` });
  const [saved] = await Promise.all([page.waitForEvent('download'), link.click()]);
  // WebKit saves a name's spaces as `_`.
  expect(saved.suggestedFilename()).toBe(
    browserName === 'webkit' ? NAME.replaceAll(' ', '_') : NAME,
  );
  expect(await bytesOf(saved)).toBe(PDF);

  // Reached by the keyboard: an arrow from the line above selects it, Enter opens it in the viewer.
  await body.getByText('Minutes:').click();
  await page.keyboard.press('End');
  await page.keyboard.press('ArrowDown');
  await expect(block).toHaveClass(/ProseMirror-selectednode/);
  await page.keyboard.press('Enter');
  await expect(page.getByTestId('file-viewer')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('file-viewer')).toHaveCount(0);
  // Enter opened it, and left the block where it was.
  await expect(block).toHaveCount(1);

  // 390 pixels across: one line, the name cut short, the button in reach.
  await page.setViewportSize({ width: 390, height: 844 });
  await hideSidebar(page);
  await expect(block).toBeVisible();
  const box = await block.boundingBox();
  const button = await link.boundingBox();
  expect(box?.height ?? 0).toBeLessThan(60);
  expect((button?.x ?? 0) + (button?.width ?? 0)).toBeLessThanOrEqual(390);
  expect(
    await block.locator('.doc-file-name').evaluate((name) => name.scrollWidth > name.clientWidth),
  ).toBe(true);
});

test('a dropped document becomes a file block, not an image', async ({ page }) => {
  const body = await newPage(page);
  await body.click();
  await page.keyboard.type('Dropped:');
  const transfer = await page.evaluateHandle(() => {
    const data = new DataTransfer();
    data.items.add(new File(['sheet'], 'budget.xlsx', { type: 'application/vnd.ms-excel' }));
    return data;
  });
  // Where the line is: the editor takes a drop only at a place in its text.
  const line = await body.getByText('Dropped:').boundingBox();
  await body.dispatchEvent('drop', {
    dataTransfer: transfer,
    clientX: (line?.x ?? 0) + (line?.width ?? 0) - 2,
    clientY: (line?.y ?? 0) + (line?.height ?? 0) / 2,
  });
  const block = page.getByTestId('page-body').locator('[data-file-block]');
  await expect(block).toHaveAttribute('data-state', 'shown');
  await expect(block).toHaveAttribute('aria-label', 'File: budget.xlsx, 5 B');
  await expect(page.getByTestId('page-body').locator('img[data-file-id]')).toHaveCount(0);
});

test('files chosen together become file blocks in the order chosen', async ({ page }) => {
  const body = await newPage(page);
  await body.click();
  await page.keyboard.type('Two:');
  await page.keyboard.press('Enter');
  await page.keyboard.type('/file');
  await expect(page.locator('[data-suggestions="slash"]').getByRole('option')).toHaveText([
    'File or image',
  ]);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.keyboard.press('Enter'),
  ]);
  expect(chooser.isMultiple()).toBe(true);
  await chooser.setFiles([
    { name: 'first.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF one') },
    { name: 'second.zip', mimeType: 'application/zip', buffer: Buffer.from('PK two') },
  ]);
  const blocks = page.getByTestId('page-body').locator('[data-file-block]');
  await expect(blocks).toHaveCount(2);
  await expect(blocks.locator('.doc-file-name')).toHaveText(['first.pdf', 'second.zip']);
  await expect(page.getByTestId('page-body').locator('.doc-upload')).toHaveCount(0);
});

/** Sets how long the backend in memory takes over each request, in milliseconds. */
async function latency(page: Page, milliseconds: number): Promise<void> {
  await page.evaluate((ms) => {
    (window as unknown as { fvociDev: { backend: { latency: number } } }).fvociDev.backend.latency =
      ms;
  }, milliseconds);
}

test('a file still going up when the page is left is given up, and the reader is told so', async ({
  page,
}) => {
  const body = await newPage(page);
  await pageAction(page, 'Minutes', 'add-after');
  await nameThePage(page, 'Agenda');
  await showTree(page);
  await treeItem(page, 'Minutes').click();
  await expect(page.getByTestId('page-title')).toHaveValue('Minutes');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');

  await body.click();
  await page.keyboard.type('/file');
  await expect(page.locator('[data-suggestions="slash"]').getByRole('option')).toHaveText([
    'File or image',
  ]);
  // Slow enough to be left on its way.
  await latency(page, 5000);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.keyboard.press('Enter'),
  ]);
  await chooser.setFiles({
    name: 'slow.pdf',
    mimeType: 'application/pdf',
    buffer: Buffer.from(PDF),
  });
  await expect(page.getByTestId('page-body').locator('.doc-upload')).toHaveCount(1);
  // Going up is told where every upload is, not in a line under the body that blinks away.
  await expect(
    page.getByTestId('transfers').locator('[data-transfer="upload"][data-name="slow.pdf"]'),
  ).toContainText('Uploading slow.pdf');
  await expect(page.getByTestId('page-body').locator('.doc-editor-status')).toHaveText('');
  await latency(page, 40);

  // Given up as the page is left, and told so in the stack until dismissed: not where problems are.
  const told = page.getByTestId('transfers').locator('[data-failed="left"]');
  await expect(told).toHaveCount(0);
  await showTree(page);
  await treeItem(page, 'Agenda').click();
  await expect(page.getByTestId('page-title')).toHaveValue('Agenda');
  await expect(told).toContainText('slow.pdf was not uploaded.');
  await expect(told).toContainText('You left before it was in. Add it once more.');
  await expect(page.locator('[data-problem]')).toHaveCount(0);

  // Nothing of it is on the page it was going up for.
  await showTree(page);
  await treeItem(page, 'Minutes').click();
  await expect(page.getByTestId('page-title')).toHaveValue('Minutes');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  await expect(page.getByTestId('page-body').locator('[data-file-block], .doc-upload')).toHaveCount(
    0,
  );
});
