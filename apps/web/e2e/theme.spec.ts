/**
 * The theme is on the page before the app's script has run: a reader
 * who chose the dark screen never sees a light one first. The script at the
 * top of index.html does it from what the device remembers.
 */
import { expect, test, type Page } from '@playwright/test';

/** What the root element says, with the app's own script kept from loading. */
async function beforeTheApp(
  page: Page,
): Promise<{ dark: boolean; scheme: string; bar: string; density: string }> {
  await page.route(/\/static\/index-[0-9a-f]+\.js$/, (route) => route.abort());
  await page.goto('/');
  return page.evaluate(() => ({
    dark: document.documentElement.classList.contains('dark'),
    scheme: document.documentElement.style.colorScheme,
    bar: document.querySelector('meta[name="theme-color"]')?.getAttribute('content') ?? '',
    density: document.documentElement.dataset.density ?? '',
  }));
}

async function remember(page: Page, settings: object): Promise<void> {
  await page.addInitScript((stored) => {
    localStorage.setItem('fvoci:display', stored);
  }, JSON.stringify(settings));
}

test('a remembered dark screen is dark before the app starts', async ({ page }) => {
  await remember(page, { theme: 'dark' });
  expect(await beforeTheApp(page)).toMatchObject({ dark: true, scheme: 'dark', bar: '#0a0a0a' });
});

test('a remembered light screen stays light on a device set to dark', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  await remember(page, { theme: 'light' });
  expect(await beforeTheApp(page)).toMatchObject({ dark: false, scheme: 'light', bar: '#ffffff' });
});

test('with nothing remembered the device decides', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  expect((await beforeTheApp(page)).dark).toBe(true);
  await page.emulateMedia({ colorScheme: 'light' });
  await page.reload();
  expect(await page.evaluate(() => document.documentElement.classList.contains('dark'))).toBe(
    false,
  );
});

test('the density follows the pointer until the reader fixes it', async ({ page, hasTouch }) => {
  expect((await beforeTheApp(page)).density).toBe(hasTouch ? 'comfortable' : 'compact');
});

test('a remembered density is used whatever the pointer is', async ({ page }) => {
  await remember(page, { density: 'comfortable' });
  expect((await beforeTheApp(page)).density).toBe('comfortable');
});

test('something else under the key does not stop the page', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem('fvoci:display', 'not json');
  });
  expect(await beforeTheApp(page)).toMatchObject({ dark: false, bar: '#ffffff' });
});

test('once the app runs, the account decides, and what it says is remembered for next time', async ({
  page,
}) => {
  // The account (kept between visits by the in-memory backend) says dark.
  await page.addInitScript(() => {
    localStorage.setItem('fvoci:memory-settings', JSON.stringify({ theme: 'dark' }));
  });
  await page.goto('/');
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await expect(page.locator('html')).toHaveClass(/dark/);
  const background = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(background).not.toBe('rgb(255, 255, 255)');
  // And the device now remembers it for the script that runs before the app.
  const remembered = await page.evaluate(() => localStorage.getItem('fvoci:display'));
  expect(JSON.parse(remembered ?? '{}')).toMatchObject({ theme: 'dark' });
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute('content', '#0a0a0a');
});

test('the page runs under a content security policy that allows its one inline script by hash', async ({
  page,
}) => {
  const violations: string[] = [];
  page.on('console', (message) => {
    if (message.text().includes('Content Security Policy')) violations.push(message.text());
  });
  const response = await page.goto('/');
  const policy = response?.headers()['content-security-policy'] ?? '';
  expect(policy).toMatch(/script-src 'self' 'sha256-[A-Za-z0-9+/=]+'/);
  expect(policy).not.toContain("'unsafe-inline' 'sha256");
  expect(policy).not.toContain('unsafe-eval');
  await expect(page.getByTestId('board-page')).toBeVisible();
  // The app and everything it fetched ran without a refusal.
  expect(violations).toEqual([]);
  // Another inline script does not run.
  const injected = await page.evaluate(() => {
    const script = document.createElement('script');
    script.textContent = 'window.__injected = true;';
    document.head.append(script);
    return (window as unknown as { __injected?: boolean }).__injected === true;
  });
  expect(injected).toBe(false);
  expect(violations).toHaveLength(1);
});

test('under the policy, the inline script still sets the theme before the app', async ({
  page,
}) => {
  await remember(page, { theme: 'dark' });
  expect(await beforeTheApp(page)).toMatchObject({ dark: true, bar: '#0a0a0a' });
});
