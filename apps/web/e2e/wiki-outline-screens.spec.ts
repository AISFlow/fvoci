/**
 * Pictures of the outline of a wiki's page while it is edited, taken only
 * with `FVOCI_SCREENS=1`, to `target/wiki-toc/screens/`: wide (beside the
 * page, a long outline scrolled to the heading being read), a middle width
 * (behind the button, opened) and 390 pixels (the list open), light and
 * dark. The read-only and public ones are taken on the real server
 * (e2e-live/wiki-outline.spec.ts).
 */
import path from 'node:path';

import { expect, test, type Page } from '@playwright/test';

import {
  bodyHeading,
  currentItem,
  longBody,
  outline,
  outlineBeside,
  outlineItems,
  pasteIntoBody,
} from './outline.support.ts';
import { openApp } from './shell.support.ts';
import { nameThePage, newWiki } from './wiki.support.ts';

test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');
test.use({ locale: 'ko-KR', reducedMotion: 'reduce' });

const SCREENS = path.resolve(import.meta.dirname, '../../../target/wiki-toc/screens');

async function pictured(page: Page, name: string): Promise<void> {
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
    await page.screenshot({ path: `${SCREENS}/${name}-${scheme}.png`, animations: 'disabled' });
  }
}

for (const [width, height, size] of [
  [1600, 900, 'wide'],
  [1280, 800, 'middle'],
  [390, 844, '390'],
] as const) {
  test(`the outline of a page being edited, ${size}`, async ({ page }, info) => {
    test.skip(info.project.name !== 'desktop', 'one set of pictures');
    await page.setViewportSize({ width, height });
    await openApp(page);
    await newWiki(page, '팀 안내서');
    await page.getByTestId('wiki-first-page').click();
    await nameThePage(page, '일하는 방식');
    await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
    await pasteIntoBody(page, longBody(40));
    if (size === 'wide') {
      await expect(outlineBeside(page)).toBeVisible();
      await outline(page).getByRole('button', { name: 'Section 23', exact: true }).click();
      await expect(currentItem(page)).toHaveText('Section 23');
      await pictured(page, `editing-long-${size}`);
      return;
    }
    await page.getByTestId('open-page-outline').click();
    await expect(outlineItems(page)).toHaveCount(40);
    await pictured(page, `editing-open-${size}`);
    await outline(page).getByRole('button', { name: 'Section 9', exact: true }).click();
    await expect(outline(page)).toHaveCount(0);
    await expect(bodyHeading(page, 'Section 9')).toBeInViewport();
    await pictured(page, `editing-${size}`);
  });
}
