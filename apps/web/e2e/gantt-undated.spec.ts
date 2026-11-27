/** The count and way into undated cards stay in sight, even beside both warnings. */
import { expect, test } from '@playwright/test';

import { openRelated, setAfter } from './gantt.support.ts';
import { screensDir } from './screens.ts';

test.use({ timezoneId: 'Asia/Seoul', locale: 'en-US' });

for (const width of [390, 1280]) {
  test(`undated cards stay reachable without scrolling the toolbar at ${width}px`, async ({
    page,
  }, info) => {
    await openRelated(page, info);
    await setAfter(page, { 'Write in the body': ['Keep your documents in a wiki'] });
    await page.setViewportSize({ width, height: 844 });
    await expect(page.getByTestId('gantt-late')).toBeVisible();
    await expect(page.getByTestId('gantt-loops')).toBeVisible();
    await page.getByTestId('gantt-toolbar').evaluate((element) => {
      element.scrollLeft = 0;
    });
    const button = page.getByTestId('gantt-undated');
    await expect(button).toBeInViewport({ ratio: 1 });
    await expect(button.getByText('No date', { exact: true })).toBeVisible();
    const count = Number((await button.getAttribute('aria-label'))?.match(/\d+/)?.[0]);
    expect(count).toBeGreaterThan(0);
    await expect(button).toContainText(String(count));
    if (width === 1280) {
      // The scroller leaves the same vertical room for focus rings as the outer row.
      const tools = await page.getByTestId('gantt-tools').boundingBox();
      const today = await page.getByTestId('gantt-today').boundingBox();
      if (!tools || !today) throw new Error('the toolbar has no bounds');
      expect(today.y - tools.y).toBeGreaterThan(1);
      expect(tools.y + tools.height - today.y - today.height).toBeGreaterThan(1);
      for (const name of ['gantt-today', 'gantt-zoom-in', 'gantt-late', 'gantt-loops']) {
        await expect(page.getByTestId(name)).toBeInViewport({ ratio: 1 });
      }
    }
    if (process.env.FVOCI_SCREENS) {
      await page.screenshot({
        path: `${screensDir('chores')}/gantt-${width}-${info.project.name}.png`,
      });
    }
    await button.click();
    const list = page.getByTestId('undated-list');
    await expect(list).toBeVisible();
    await expect(list.locator('[data-card]')).toHaveCount(count);
    await expect(list).toContainText('Attach files and open them right here');
    if (process.env.FVOCI_SCREENS) {
      await page.screenshot({
        path: `${screensDir('chores')}/gantt-${width}-${info.project.name}-open.png`,
      });
    }
    await page.keyboard.press('Escape');
    await expect(list).toHaveCount(0);
    await expect(button).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(list).toBeVisible();
  });
}
