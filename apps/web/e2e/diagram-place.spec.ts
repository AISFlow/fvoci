/**
 * A diagram in the body of a card: its picture alone, and the place where its
 * source is changed, opened under the picture by a press or by Enter and
 * closed by Escape. With the mouse, with the keys alone, on a narrow screen,
 * and where the body may only be read.
 *
 * `FVOCI_SCREENS=1` also takes the pictures of it, into `target/screens/mermaid/`.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { primaryKey } from './keyboard.support.ts';
import { diagramBody, drawnDiagram, focusAtEnd, openCardWithBody } from './embeds.support.ts';

const FLOW = 'flowchart LR\n  A[Start] --> B{Choice}\n  B -->|yes| C[End]';

test.describe.configure({ timeout: 90_000 });

const blockOf = (body: Locator): Locator => body.locator('.doc-diagram-block');
const pictureOf = (body: Locator): Locator => blockOf(body).locator('.doc-diagram');
const sourceOf = (body: Locator): Locator => blockOf(body).locator('> pre.doc-diagram-source');

/** The editor is there (not the view it stands in for while it is fetched), its diagram drawn and closed. */
async function closedAndDrawn(body: Locator): Promise<void> {
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await expect(blockOf(body)).toHaveAttribute('data-state', 'drawn', { timeout: 30_000 });
  await drawnDiagram(body);
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
  await expect(sourceOf(body)).toBeHidden();
}

/** The editor has the focus and the diagram is selected as a whole. */
async function selectedWithFocus(page: Page, body: Locator): Promise<void> {
  await expect(blockOf(body)).toHaveClass(/ProseMirror-selectednode/);
  await expect(body.locator('.ProseMirror')).toBeFocused();
}

test('a press opens the source under the picture, typing draws it again, Escape closes', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Getting started', diagramBody(FLOW));
  await closedAndDrawn(body);

  await pictureOf(body).click();
  await expect(blockOf(body)).toHaveAttribute('data-open', '');
  await expect(sourceOf(body)).toBeVisible();
  // The picture stays on top, the source right under it.
  const picture = await pictureOf(body).boundingBox();
  const source = await sourceOf(body).boundingBox();
  expect(picture && source && source.y >= picture.y + picture.height - 1).toBe(true);

  await page.keyboard.press('Enter');
  await expect.poll(() => sourceOf(body).textContent()).toMatch(/\n$/);
  await page.keyboard.type('  C --> D[Done]');
  await expect(pictureOf(body).locator('svg')).toContainText('Done', { timeout: 30_000 });
  await expect(blockOf(body)).toHaveAttribute('data-state', 'drawn');

  await page.keyboard.press('Escape');
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
  await expect(sourceOf(body)).toBeHidden();
  await selectedWithFocus(page, body);
  // Escape belonged to the place: the card is still open.
  await expect(body).toBeVisible();
  await expect(pictureOf(body).locator('svg')).toContainText('Done');
});

test('with the keys alone: arrows reach the diagram, Enter opens, Escape closes', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Getting started', {
    type: 'doc',
    content: [
      { type: 'paragraph', content: [{ type: 'text', text: 'Above' }] },
      ...diagramBody(FLOW).content,
      { type: 'paragraph', content: [{ type: 'text', text: 'Below' }] },
    ],
  });
  await closedAndDrawn(body);
  // The cursor at the end of "Below".
  await focusAtEnd(body);
  await page.keyboard.press('ArrowUp');
  await selectedWithFocus(page, body);
  // Passing over it does not open it.
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
  await page.keyboard.press('ArrowUp');
  await expect(blockOf(body)).not.toHaveClass(/ProseMirror-selectednode/);
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
  await page.keyboard.press('ArrowDown');
  await selectedWithFocus(page, body);

  await page.keyboard.press('Enter');
  await expect(blockOf(body)).toHaveAttribute('data-open', '');
  await expect(body.locator('.doc-announce')).toContainText('Escape closes');
  await page.keyboard.press('Enter');
  await expect.poll(() => sourceOf(body).textContent()).toMatch(/\n$/);
  await page.keyboard.type('  C --> K[Keys]');
  await expect(pictureOf(body).locator('svg')).toContainText('Keys', { timeout: 30_000 });
  await page.keyboard.press(await primaryKey(page, 'Enter'));
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
  await selectedWithFocus(page, body);

  // Closed again, an undo takes the typing back, and the picture follows.
  await page.keyboard.press(await primaryKey(page, 'z'));
  await expect(pictureOf(body).locator('svg')).not.toContainText('Keys', { timeout: 30_000 });
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
});

test('a source that cannot be drawn shows, closed, with what went wrong; open, the last picture stays dimmed', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Getting started', diagramBody(FLOW));
  await closedAndDrawn(body);
  await pictureOf(body).click();
  await page.keyboard.press('Enter');
  await expect.poll(() => sourceOf(body).textContent()).toMatch(/\n$/);
  await page.keyboard.type('  -->');
  await expect(blockOf(body)).toHaveAttribute('data-state', 'error', { timeout: 30_000 });
  await expect(pictureOf(body)).toHaveAttribute('data-stale', '');
  await expect(pictureOf(body).locator('svg')).toBeVisible();
  await expect(blockOf(body).locator('.doc-diagram-problem')).toContainText(
    'This diagram cannot be drawn.',
  );
  await page.keyboard.press('Escape');
  await expect(pictureOf(body).locator('svg')).toHaveCount(0);
  await expect(pictureOf(body).locator('.doc-diagram-copy')).toContainText('-->');
  await expect(pictureOf(body).getByRole('note')).toHaveText('This diagram cannot be drawn.');
  // A press on it opens the same place.
  await pictureOf(body).click();
  await expect(sourceOf(body)).toBeVisible();
});

test('where the body may only be read, the picture does not open', async ({ page }) => {
  const body = await openCardWithBody(page, 'Study notes', diagramBody(FLOW));
  await expect(body).toHaveAttribute('data-editor', 'reading');
  await drawnDiagram(body);
  await expect(body.locator('pre:has(code.language-mermaid)')).toBeHidden();
  await body.locator('.doc-diagram').click();
  await expect(body.locator('pre:has(code.language-mermaid)')).toBeHidden();
  await expect(body.locator('[contenteditable="true"], textarea')).toHaveCount(0);
});

test.describe('on a narrow screen', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('the place opens in the flow, as wide as the body, and the page does not scroll sideways', async ({
    page,
  }) => {
    const body = await openCardWithBody(page, 'Getting started', diagramBody(FLOW));
    await closedAndDrawn(body);
    await pictureOf(body).click();
    await expect(sourceOf(body)).toBeVisible();
    const done = blockOf(body).getByRole('button', { name: 'Done' });
    await expect(done).toBeVisible();
    for (const part of [pictureOf(body), sourceOf(body), done]) {
      const box = await part.boundingBox();
      expect(box && box.x >= 0 && box.x + box.width <= 390 + 0.5).toBe(true);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      390,
    );
    await page.keyboard.press('Enter');
    await expect.poll(() => sourceOf(body).textContent()).toMatch(/\n$/);
    await page.keyboard.type('  C --> N[Narrow]');
    await expect(pictureOf(body).locator('svg')).toContainText('Narrow', { timeout: 30_000 });
    await done.click();
    await expect(blockOf(body)).not.toHaveAttribute('data-open');
    await selectedWithFocus(page, body);
  });
});
