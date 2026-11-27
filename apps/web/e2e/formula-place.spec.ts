/**
 * A formula on a line of its own in the body of a card: its drawing alone,
 * and the place where its source is changed, opened under the drawing by a
 * press or by Enter and closed by Escape, the way a diagram's is
 * (diagram-place.spec.ts). Where the body may only be read, it does not open.
 */
import { expect, test, type Locator } from '@playwright/test';

import { primaryKey } from './keyboard.support.ts';
import { focusAtEnd, openCardWithBody } from './embeds.support.ts';

const formulaBody = (latex: string): object => ({
  type: 'doc',
  content: [
    { type: 'paragraph', content: [{ type: 'text', text: 'Above' }] },
    { type: 'blockMath', attrs: { latex } },
    { type: 'paragraph', content: [{ type: 'text', text: 'Below' }] },
  ],
});

test.describe.configure({ timeout: 90_000 });

const blockOf = (body: Locator): Locator => body.locator('.doc-math-block');
const drawingOf = (body: Locator): Locator => blockOf(body).locator('> .doc-math');
const fieldOf = (body: Locator): Locator =>
  blockOf(body).getByRole('textbox', { name: 'LaTeX source' });

async function closedAndDrawn(body: Locator): Promise<void> {
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await expect(drawingOf(body)).toHaveAttribute('data-state', 'drawn', { timeout: 30_000 });
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
  await expect(fieldOf(body)).toHaveCount(0);
}

test('a press opens the source under the drawing, typing draws it again, Escape closes', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Getting started', formulaBody('x^2'));
  await closedAndDrawn(body);
  await drawingOf(body).click();
  await expect(fieldOf(body)).toBeFocused();
  // Both are there once the place is open: the drawing is put anew as it opens.
  await expect
    .poll(async () => {
      const drawing = await drawingOf(body).boundingBox();
      const field = await fieldOf(body).boundingBox();
      return Boolean(drawing && field && field.y >= drawing.y + drawing.height - 1);
    })
    .toBe(true);

  await page.keyboard.press('End');
  await page.keyboard.type(' + \\sqrt{y}');
  await expect(drawingOf(body).locator('.katex .sqrt')).toBeVisible();
  // A source that cannot be drawn leaves the last drawing, dimmed, and says why.
  await page.keyboard.type(' \\frac{');
  await expect(drawingOf(body)).toHaveAttribute('data-stale', '');
  await expect(blockOf(body).locator('.doc-place-problem')).toHaveText(
    'This formula cannot be drawn.',
  );
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await page.keyboard.press('Backspace');
  await expect(drawingOf(body)).not.toHaveAttribute('data-stale');

  await page.keyboard.press('Escape');
  await expect(fieldOf(body)).toHaveCount(0);
  await expect(blockOf(body)).toHaveClass(/ProseMirror-selectednode/);
  await expect(body.locator('.ProseMirror')).toBeFocused();
  await expect(drawingOf(body)).toHaveAttribute('aria-label', 'x^2 + \\sqrt{y}');
});

test('with the keys alone: an arrow selects it, Enter opens, Ctrl+Enter closes', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Getting started', formulaBody('a+b'));
  await closedAndDrawn(body);
  await focusAtEnd(body);
  await page.keyboard.press('ArrowUp');
  await expect(blockOf(body)).toHaveClass(/ProseMirror-selectednode/);
  await expect(blockOf(body)).not.toHaveAttribute('data-open');
  await page.keyboard.press('Enter');
  await expect(fieldOf(body)).toBeFocused();
  await page.keyboard.press('End');
  await page.keyboard.type('+c');
  await page.keyboard.press(await primaryKey(page, 'Enter'));
  await expect(fieldOf(body)).toHaveCount(0);
  await expect(blockOf(body)).toHaveClass(/ProseMirror-selectednode/);
  await expect(drawingOf(body)).toHaveAttribute('aria-label', 'a+b+c');
});

test('a source that cannot be drawn shows, closed, as itself with what went wrong', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Getting started', formulaBody('\\frac{'));
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await expect(drawingOf(body)).toHaveAttribute('data-state', 'error', { timeout: 30_000 });
  await expect(drawingOf(body)).toContainText('\\frac{');
  await expect(drawingOf(body).getByRole('note')).toHaveText('This formula cannot be drawn.');
  await drawingOf(body).click();
  await expect(fieldOf(body)).toHaveValue('\\frac{');
});

test('where the body may only be read, the formula does not open', async ({ page }) => {
  const body = await openCardWithBody(page, 'Study notes', formulaBody('x^2'));
  await expect(body).toHaveAttribute('data-editor', 'reading');
  const formula = body.locator('div.doc-math[data-state="drawn"]');
  await expect(formula).toBeVisible({ timeout: 30_000 });
  await formula.click();
  await expect(body.locator('textarea, [contenteditable="true"]')).toHaveCount(0);
});

test.describe('on a narrow screen', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('the place opens in the flow, as wide as the body', async ({ page }) => {
    const body = await openCardWithBody(page, 'Getting started', formulaBody('x^2'));
    await closedAndDrawn(body);
    await drawingOf(body).click();
    const done = blockOf(body).getByRole('button', { name: 'Done' });
    for (const part of [fieldOf(body), done]) {
      const box = await part.boundingBox();
      expect(box && box.x >= 0 && box.x + box.width <= 390 + 0.5).toBe(true);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      390,
    );
    await done.click();
    await expect(fieldOf(body)).toHaveCount(0);
    await expect(blockOf(body)).toHaveClass(/ProseMirror-selectednode/);
  });
});
