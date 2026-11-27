/**
 * A member's profile, on the in-memory backend: the sharing dialog lists each member by
 * their name with title and credentials, a button that opens the card of their profile.
 * Another user of the same world (`fvociDev.backend.as`) names themselves, and the list
 * follows without the page being loaded again; the narrow places (the faces of who is
 * here) keep the name alone. At 390px, light and dark, the list and the card fit.
 */
import path from 'node:path';

import { expect, test, type Page } from '@playwright/test';

import { othersArrive, type Dev } from './presence.support.ts';
import { screensDir } from './screens.ts';
import { openApp, pressInBar, topBar } from './shell.support.ts';

/**
 * Has the first other user of the world, on the board, name themselves (shown by
 * `customName` when one is given, and signing in as `login` when one is); their login
 * name.
 */
async function otherNamesThemselves(page: Page, customName = '', login = ''): Promise<string> {
  return page.evaluate(
    async ([customName, login]) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      // The one `othersArrive` brings onto the board first.
      const other = [...dev.backend.world.users.keys()].find((id) => id !== dev.workspace.me?.id);
      if (!other) throw new Error('nobody else is on the board');
      const held = dev.backend.world.users.get(other);
      if (login && held) held.username = login;
      const named = await (
        dev.backend.as(other) as unknown as {
          patchMe(patch: Record<string, unknown>): Promise<{ username: string }>;
        }
      ).patchMe({
        lastName: '김',
        firstName: '영환',
        latinFirstName: 'Younghwan',
        latinLastName: 'Kim',
        title: 'dr',
        credentials: ['phd', 'kr_attorney'],
        ...(customName ? { customName, nameShown: 'custom_name' } : {}),
      });
      return named.username;
    },
    [customName, login] as const,
  );
}

test("a member's name with title and credentials opens their profile, which follows a change of it", async ({
  page,
}) => {
  await openApp(page);
  await othersArrive(page, 1);
  await pressInBar(page, 'share');
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog).toBeVisible();
  const members = dialog.getByTestId('share-members');

  const login = await otherNamesThemselves(page);
  // Told, and read again by the page: the list shows the new name with its title.
  const opener = members.getByRole('button', {
    name: `Dr. 김영환, PhD, Attorney at law (Korea) @${login}`,
  });
  await expect(opener).toBeVisible();
  await expect(opener).toHaveAttribute('aria-haspopup', 'dialog');

  // By the keyboard: Enter opens it, Escape closes it and gives the focus back.
  await opener.focus();
  await page.keyboard.press('Enter');
  const card = page.getByRole('dialog', { name: 'Dr. 김영환, PhD, Attorney at law (Korea)' });
  await expect(card).toBeVisible();
  await expect(card.locator('[data-row="name"]')).toHaveText('김영환');
  await expect(card.locator('[data-row="latin"]')).toHaveText('Younghwan Kim');
  await expect(card.locator('[data-row="title"]')).toHaveText('Dr.');
  await expect(card.locator('[data-row="credentials"]')).toHaveText('PhD, Attorney at law (Korea)');
  await expect(card).toContainText(`@${login}`);
  await page.keyboard.press('Escape');
  await expect(card).toHaveCount(0);
  await expect(opener).toBeFocused();
  // The sharing dialog stays: Escape closed the card alone.
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);

  // The list of who is here is a narrow place: the name alone, without title.
  await page.setViewportSize({ width: 1280, height: 800 });
  await topBar(page).getByTestId('board-presence').click();
  const here = page.getByTestId('presence-list');
  await expect(here).toContainText(`김영환 (@${login})`);
  await expect(here).not.toContainText('Dr.');
  await expect(here).not.toContainText('PhD');
});

test.describe('at 390px, dark', () => {
  test.use({ viewport: { width: 390, height: 844 }, colorScheme: 'dark' });

  test('a long name and login name stay in the row, and the card in the window', async ({
    page,
  }, testInfo) => {
    test.skip(testInfo.project.name !== 'desktop', 'the size is set here, once');
    await openApp(page);
    await othersArrive(page, 1);
    const login = await otherNamesThemselves(
      page,
      '김영환김영환김영환김영환김영환',
      'younghwan.kim.cardiology.department',
    );
    await pressInBar(page, 'share');
    const opener = page.getByTestId('share-members').locator(`[data-person="${login}"]`);
    await expect(opener).toBeVisible();
    // Nothing of the name spills out of the button, nor the button out of its row.
    const inside = await opener.evaluate((button) => {
      const box = button.getBoundingClientRect();
      const parts = [...button.children].map((part) => part.getBoundingClientRect());
      const rowBox = button.closest('[data-member]')?.getBoundingClientRect();
      return (
        parts.every((part) => part.right <= box.right + 0.5) &&
        rowBox !== undefined &&
        box.right <= rowBox.right + 0.5
      );
    });
    expect(inside).toBe(true);
    await opener.click();
    const card = page.getByTestId('profile-card');
    await expect(card).toBeVisible();
    const fits = await card.evaluate((element) => {
      const box = element.getBoundingClientRect();
      return box.left >= 0 && box.right <= window.innerWidth;
    });
    expect(fits).toBe(true);
    // The page scrolls nowhere sideways.
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      390,
    );
    // Dark: the card is drawn in the dark theme's colours (read back as a pixel).
    expect(await page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(
      true,
    );
    const red = await card.evaluate((element) => {
      const canvas = document.createElement('canvas');
      canvas.width = 1;
      canvas.height = 1;
      const context = canvas.getContext('2d');
      if (!context) return 255;
      context.fillStyle = getComputedStyle(element).backgroundColor;
      context.fillRect(0, 0, 1, 1);
      return context.getImageData(0, 0, 1, 1).data[0] ?? 255;
    });
    expect(red).toBeLessThan(96);
  });
});

/**
 * Pictures of the card over the sharing dialog, wide and at 390px, light and dark, into
 * `target/screens/profile-card/`: taken only with `FVOCI_SCREENS=1`.
 */
test('pictures of the profile card', async ({ page }, testInfo) => {
  test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');
  test.skip(testInfo.project.name !== 'desktop', 'once, at the sizes it sets');
  for (const [width, viewport] of [
    ['wide', { width: 1280, height: 900 }],
    ['390', { width: 390, height: 844 }],
  ] as const) {
    for (const colorScheme of ['light', 'dark'] as const) {
      await page.setViewportSize(viewport);
      await page.emulateMedia({ colorScheme, reducedMotion: 'reduce' });
      await openApp(page);
      await othersArrive(page, 1);
      const login = await otherNamesThemselves(page);
      await pressInBar(page, 'share');
      const members = page.getByTestId('share-members');
      await members.getByTestId('person-profile').filter({ hasText: login }).click();
      await expect(page.getByTestId('profile-card')).toBeVisible();
      await page.evaluate(() => document.fonts.ready);
      await page.screenshot({
        path: path.join(screensDir('profile-card'), `card-${width}-${colorScheme}.png`),
      });
    }
  }
});
