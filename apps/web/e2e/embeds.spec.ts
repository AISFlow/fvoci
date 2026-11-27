/**
 * Formulas and diagrams in the assembled app: the body of a card draws them
 * in the editor (a board the reader may change) and in the read-only view (a
 * board the reader may only look at), and both run under the page's content
 * security policy without a refusal.
 *
 * What the two libraries may and may not do with hostile input is tested
 * where it can be tested widely: e2e/editor-attacks.spec.ts and the unit
 * tests. Here is only that the assembled app, with the policy, draws them.
 *
 * `FVOCI_SCREENS=1` also takes the pictures `embeds-*`, into
 * `target/screens/card/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';
import { drawnDiagram, openCardWithBody, type Dev } from './embeds.support.ts';
import { screensDir } from './screens.ts';

const BODY = {
  type: 'doc',
  content: [
    {
      type: 'paragraph',
      content: [
        { type: 'text', text: 'Energy: ' },
        { type: 'inlineMath', attrs: { latex: 'E = mc^2' } },
      ],
    },
    { type: 'blockMath', attrs: { latex: '\\int_0^1 x^2\\,dx = \\frac{1}{3}' } },
    {
      type: 'codeBlock',
      attrs: { language: 'mermaid' },
      content: [
        { type: 'text', text: 'flowchart LR\n  A[Start] --> B{Choice}\n  B -->|yes| C[End]' },
      ],
    },
  ],
};

async function openCardWithEmbeds(page: Page, board: string): Promise<Locator> {
  return openCardWithBody(page, board, BODY);
}

function refusals(page: Page): string[] {
  const seen: string[] = [];
  page.on('console', (message) => {
    if (message.text().includes('Content Security Policy')) seen.push(message.text());
  });
  return seen;
}

async function drawn(body: Locator): Promise<void> {
  // The inline formula and the one on its own line.
  await expect(body.locator('.doc-math[data-state="drawn"]')).toHaveCount(2, { timeout: 30_000 });
  await expect(body.locator('.doc-math .katex').first()).toBeVisible();
  await expect(body.locator('.doc-math[data-state="error"]')).toHaveCount(0);
  await drawnDiagram(body);
  await expect(body.locator('.doc-diagram svg')).toContainText('Start');
}

test.describe.configure({ timeout: 90_000 });

test('the editor of a card draws a formula and a diagram, under the policy', async ({ page }) => {
  const seen = refusals(page);
  const body = await openCardWithEmbeds(page, 'Getting started');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await drawn(body);
  expect(seen).toEqual([]);
});

test('a card the reader may only look at draws them too, in an editor that only reads', async ({
  page,
}) => {
  const seen = refusals(page);
  const body = await openCardWithEmbeds(page, 'Study notes');
  await expect(body).toHaveAttribute('data-editor', 'reading');
  await expect(body.locator('[contenteditable="true"]')).toHaveCount(0);
  await drawn(body);
  expect(seen).toEqual([]);
});

test('a reader who asked for less motion gets the whole diagram too', async ({ page }) => {
  // The page shortens every transition for this reader; the diagram is
  // measured while it is drawn, and a shortened transition once hid from the
  // measuring where the nodes had just been put.
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const body = await openCardWithEmbeds(page, 'Getting started');
  await drawn(body);
});

test('on the dark screen the diagram is drawn again with the colours of the dark screen', async ({
  page,
}) => {
  const body = await openCardWithEmbeds(page, 'Study notes');
  await drawn(body);
  const fillOf = (): Promise<string> =>
    body
      .locator('.doc-diagram svg text, .doc-diagram svg tspan')
      .first()
      .evaluate((element) => getComputedStyle(element).fill);
  const light = await fillOf();
  // The way the settings dialog does it: the reader's account says dark.
  await page.evaluate(() =>
    (window as unknown as { fvociDev: Dev }).fvociDev.backend.patchSettings({ theme: 'dark' }),
  );
  await expect(page.locator('html')).toHaveClass(/dark/);
  // The diagram watches the class of the page and is drawn again.
  await expect.poll(fillOf, { timeout: 30_000 }).not.toBe(light);
  await drawn(body);
});

// ------------------------------------------------------------ the pictures

const STAGE_OF_PROJECT: Record<string, string> = {
  desktop: 'expanded',
  middle: 'medium',
  mobile: 'compact',
};

test.describe('pictures of a formula and a diagram', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'set FVOCI_SCREENS=1 to take the pictures');

  const SCREENS = screensDir('card');

  for (const scheme of ['light', 'dark'] as const) {
    test(`a card with a formula and a diagram (${scheme})`, async ({ page }, testInfo) => {
      const stage = STAGE_OF_PROJECT[testInfo.project.name] ?? 'expanded';
      // Nothing fades or slides: a picture is not taken halfway through.
      await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
      const body = await openCardWithEmbeds(page, 'Getting started');
      await drawn(body);
      await page.waitForLoadState('networkidle');
      await page.evaluate(() => document.fonts.ready);
      await body.locator('.doc-diagram').scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(SCREENS, `embeds-${stage}-${scheme}.png`) });
    });
  }
});
