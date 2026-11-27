/**
 * Pictures of a diagram and of a block formula in the body of a card: closed,
 * pointed at, being edited, and with a source that cannot be drawn (closed and
 * being edited), on a wide screen and at 390 pixels. Taken only with
 * `FVOCI_SCREENS=1`, into `target/screens/mermaid/` (`e2e/screens.ts`); without
 * it these tests are not there at all.
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { diagramBody, openCardWithBody } from './embeds.support.ts';
import { screensDir } from './screens.ts';

const SCREENS = screensDir('mermaid');
const FLOW = 'flowchart LR\n  A[Draft] --> B{Review}\n  B -->|pass| C[Ship]\n  B -->|fix| A';

const WIDTHS = [
  { name: 'wide', viewport: { width: 1280, height: 860 } },
  { name: '390', viewport: { width: 390, height: 844 } },
] as const;

interface Kind {
  name: 'diagram' | 'formula';
  body: object;
  block: string;
  /** What is pressed to open it. */
  picture: string;
  /** Typed at the end of the source: one that draws, one that cannot be drawn. */
  more: string;
  broken: string;
  /** Says the picture is drawn, and drawn again with `more`. */
  drawn: (block: Locator) => Promise<void>;
  edited: (block: Locator) => Promise<void>;
}

const withText = (middle: object): object => ({
  type: 'doc',
  content: [
    { type: 'paragraph', content: [{ type: 'text', text: 'The release goes through review:' }] },
    middle,
    { type: 'paragraph', content: [{ type: 'text', text: 'Anything after it.' }] },
  ],
});

const KINDS: Kind[] = [
  {
    name: 'diagram',
    body: withText(diagramBody(FLOW).content[0] ?? {}),
    block: '.doc-diagram-block',
    picture: '.doc-diagram',
    more: '\n  C --> D[Done]',
    broken: '\n  D -->',
    drawn: (block) => expect(block.locator('.doc-diagram svg')).toBeVisible({ timeout: 30_000 }),
    edited: (block) =>
      expect(block.locator('.doc-diagram svg')).toContainText('Done', { timeout: 30_000 }),
  },
  {
    name: 'formula',
    body: withText({ type: 'blockMath', attrs: { latex: '\\int_0^1 x^2\\,dx = \\frac{1}{3}' } }),
    block: '.doc-math-block',
    picture: '> .doc-math',
    more: ' + \\sqrt{2}',
    broken: ' + \\frac{',
    drawn: (block) => expect(block.locator('> .doc-math .katex')).toBeVisible({ timeout: 30_000 }),
    edited: (block) =>
      expect(block.locator('> .doc-math .katex .sqrt')).toBeVisible({ timeout: 30_000 }),
  },
];

async function shoot(page: Page, block: Locator, name: string): Promise<void> {
  await page.waitForLoadState('networkidle');
  await page.evaluate(() => document.fonts.ready);
  await block.scrollIntoViewIfNeeded();
  await page.screenshot({ path: path.join(SCREENS, `${name}.png`) });
}

if (process.env.FVOCI_SCREENS) {
  test.describe.configure({ timeout: 120_000 });
  for (const width of WIDTHS) {
    for (const kind of KINDS) {
      test(`pictures of a ${kind.name}, ${width.name}`, async ({ page }, testInfo) => {
        test.skip(testInfo.project.name !== 'desktop', 'once, at the sizes it sets');
        await page.setViewportSize(width.viewport);
        await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' });
        const body = await openCardWithBody(page, 'Getting started', kind.body);
        await expect(body).toHaveAttribute('data-editor', 'ready');
        const block = body.locator(kind.block);
        const picture = block.locator(kind.picture);
        await kind.drawn(block);
        const at = (scene: string): string => `${kind.name}-${width.name}-${scene}`;

        await page.mouse.move(0, 0);
        await shoot(page, block, at('1-closed'));
        await picture.hover();
        await shoot(page, block, at('2-pointed'));

        await picture.click();
        await expect(block).toHaveAttribute('data-open', '');
        await page.keyboard.press('End');
        await page.keyboard.type(kind.more);
        await page.mouse.move(0, 0);
        await kind.edited(block);
        await shoot(page, block, at('3-editing'));

        await page.keyboard.type(kind.broken);
        await expect(block.locator('.doc-diagram-problem p, .doc-place-problem p')).toBeVisible({
          timeout: 30_000,
        });
        await shoot(page, block, at('5-broken-editing'));

        await page.keyboard.press('Escape');
        await expect(block).not.toHaveAttribute('data-open');
        // Off the block, so that its selection does not frame it.
        await body.getByText('Anything after it.').click();
        await shoot(page, block, at('4-broken-closed'));
      });
    }
  }
}
