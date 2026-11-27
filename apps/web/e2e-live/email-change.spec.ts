/**
 * Changing the account's address in the settings, on the real server: asked
 * with the password, the address waits for the link mailed to the new one,
 * which a browser that is not signed in opens and confirms by its button;
 * then the address is the new one, the old one is told so with a link back,
 * and the account signs in by the new one only. The link back, opened and
 * pressed where nobody is signed in, gives the old address back, signs the
 * account out where it is open, and leads to a reset link for that address.
 */
import { expect, test, type Browser, type Page } from '@playwright/test';

import { screensDir } from '../e2e/screens.ts';
import { apiClient, inbox, person, register, signInOnPage, watchErrors } from './support.ts';

test.use({ locale: 'en-US' });

/** Pictures the page, wide and at 390 pixels, light and dark, when `FVOCI_SCREENS` is set. */
async function picture(page: Page, name: string): Promise<void> {
  if (!process.env.FVOCI_SCREENS) return;
  const wide = page.viewportSize();
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    const path = (width: string) => `${screensDir('email-links')}/${name}-${width}-${scheme}.png`;
    await page.screenshot({ path: path('wide'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({ path: path('390'), fullPage: true });
    if (wide) await page.setViewportSize(wide);
  }
  await page.emulateMedia({ colorScheme: null });
}

test('the address is changed by the link mailed to the new one, opened where nobody is signed in', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('mover');
  const moved = `${who.username}@home.example.test`;
  await register(base, who);

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, who);
  await page.getByTestId('open-settings').click();
  const section = page.getByTestId('settings-dialog').getByTestId('email-change');
  const address = section.getByTestId('email-change-address');
  await expect(address).toHaveText(who.email);
  await expect(section.getByTestId('email-change-revert-hint')).toContainText('7 days');
  await section.getByRole('button', { name: 'Change address' }).click();
  const password = section.getByLabel('Current password');
  const ask = section.getByRole('button', { name: 'Change address' });
  await section.getByLabel('New email address').fill(moved);

  // Not without the account's password.
  await password.fill('not the password');
  await ask.click();
  await expect(section.getByRole('alert')).toHaveText('The current password is wrong.');
  await expect(password).toBeFocused();
  await expect(section.getByTestId('email-change-pending')).toHaveCount(0);

  await password.fill(who.password);
  await ask.click();
  const pending = section.getByTestId('email-change-pending');
  await expect(pending).toContainText(moved);
  await expect(address).toHaveText(who.email);
  await expect(section.getByLabel('New email address')).toHaveCount(0);

  // The link goes to the new address, and a newer one ends it.
  const linksTo = async (recipient: string): Promise<string[]> =>
    (await inbox(base))
      .filter((mail) => mail.recipient === recipient)
      .flatMap((mail) => mail.decoded.split(/\r?\n/))
      .filter((line) => line.startsWith(base) && line.includes('/confirm-email#'))
      .map((line) => line.trim());
  await expect.poll(async () => (await linksTo(moved)).length).toBe(1);
  await pending.getByRole('button', { name: 'Send the link again' }).click();
  await expect(section.getByRole('status')).toContainText(
    'Asked for again. A new link ends the one before it.',
  );
  await expect.poll(async () => (await linksTo(moved)).length).toBe(2);
  const [first, second] = await linksTo(moved);
  if (!first || !second) throw new Error('the links were not mailed');
  expect(new URL(second).hash).toHaveLength(44);

  // In a browser that is not signed in: the first link changes nothing, the second does,
  // and either only by its button: opening it, as a mail scanner would, changes nothing.
  const elsewhere = await browser.newContext({ locale: 'en-US' });
  const visitor = await elsewhere.newPage();
  const visitorErrors = watchErrors(visitor);
  const confirmButton = visitor.getByRole('button', { name: 'Confirm' });
  await visitor.goto(first);
  await confirmButton.click();
  await expect(visitor.getByRole('alert')).toContainText('This link changes nothing.');
  await visitor.goto(second);
  await expect(confirmButton).toBeVisible();
  await expect(visitor.getByText('Opening the link changed nothing.')).toBeVisible();
  await picture(visitor, 'confirm-before');
  const anon = await apiClient(base);
  const login = (email: string, password = who.password) =>
    anon.post('/api/v1/login', { data: { email, password } });
  expect((await login(moved)).status(), 'opened, not pressed').toBe(401);
  await confirmButton.click();
  await expect(visitor.getByTestId('confirm-email-done')).toContainText('Your address is changed.');
  await picture(visitor, 'confirm-after');
  expect(new URL(visitor.url()).hash).toBe('');
  // Used once: opened again, it changes nothing.
  await visitor.goto(second);
  await confirmButton.click();
  await expect(visitor.getByRole('alert')).toContainText('This link changes nothing.');
  // Nobody was signed in by it.
  await visitor.getByRole('link', { name: 'Continue' }).click();
  await expect(visitor.getByTestId('sign-in-form')).toBeVisible();
  expect(visitorErrors).toEqual([]);
  await elsewhere.close();

  // The settings read it anew as they open: the new address, and nothing waiting.
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('settings-dialog')).toHaveCount(0);
  await page.getByTestId('open-settings').click();
  await expect(address).toHaveText(moved);
  await expect(pending).toHaveCount(0);

  // The old address is told, without the new one; and the account is the new address's.
  await expect
    .poll(
      async () =>
        (await inbox(base))
          .filter((mail) => mail.recipient === who.email)
          .filter((mail) => !mail.decoded.includes('/verify-email#')).length,
    )
    .toBe(1);
  const told = (await inbox(base)).filter((mail) => mail.recipient === who.email).at(-1);
  expect(told?.decoded).toContain(who.username);
  expect(told?.decoded).not.toContain('home.example.test');
  expect((await login(who.email)).status()).toBe(401);
  expect((await login(moved)).status()).toBe(204);
  const back = told?.decoded
    .split(/\r?\n/)
    .find((line) => line.startsWith(base) && line.includes('/revert-email#'))
    ?.trim();
  if (!back) throw new Error('the notice has no link back');
  expect(new URL(back).hash).toHaveLength(44);

  await takenBack(browser, page, back, who.email);
  // Back at the old address; the password the account had signs it in no more.
  expect((await login(who.email)).status()).toBe(401);
  expect((await login(moved)).status()).toBe(401);
  const after = (await inbox(base)).filter((mail) => mail.recipient === moved).at(-1);
  expect(after?.decoded).toContain(who.username);
  expect(after?.decoded).not.toContain(who.email);
  expect(after?.decoded).not.toContain('/revert-email#');
  await recovered(browser, base, who.email);
  expect((await login(who.email, 'A recovered password 1')).status()).toBe(204);
  await anon.dispose();
  expect(errors).toEqual([]);
});

/**
 * Opens the link back to `email` where nobody is signed in: nothing changes
 * until the button is pressed, and then `page`, where the account is open,
 * is signed out. The page then leads to a reset link for `email`, which it
 * asks for.
 */
async function takenBack(browser: Browser, page: Page, link: string, email: string): Promise<void> {
  const elsewhere = await browser.newContext({ locale: 'en-US' });
  const rescuer = await elsewhere.newPage();
  const errors = watchErrors(rescuer);
  await rescuer.goto(link);
  const button = rescuer.getByRole('button', { name: 'Take the address back' });
  await expect(button).toBeVisible();
  await expect(rescuer.getByText(/Opening the link changed nothing/)).toBeVisible();
  await picture(rescuer, 'revert-before');
  // Opened, not pressed: the account is open where it was.
  await page.reload();
  await expect(page.getByTestId('open-settings')).toBeVisible();
  await button.click();
  await expect(rescuer.getByTestId('revert-email-done')).toContainText(
    `Your account has ${email} again, and is signed out everywhere.`,
  );
  await picture(rescuer, 'revert-after');
  expect(new URL(rescuer.url()).hash).toBe('');
  await expect(page.getByTestId('sign-in-form')).toBeVisible({ timeout: 15_000 });
  await rescuer.getByTestId('revert-email-reset').click();
  await expect(rescuer.getByLabel('Email address')).toHaveValue(email);
  await rescuer.getByRole('button', { name: 'Send reset link' }).click();
  await expect(rescuer.getByRole('status')).toContainText('If this email belongs to an account');
  // Used once.
  await rescuer.goto(link);
  await button.click();
  await expect(rescuer.getByRole('alert')).toContainText('This link changes nothing.');
  expect(errors).toEqual([]);
  await elsewhere.close();
}

/** Sets a new password by the reset link mailed to `email`. */
async function recovered(browser: Browser, base: string, email: string): Promise<void> {
  const elsewhere = await browser.newContext({ locale: 'en-US' });
  const rescuer = await elsewhere.newPage();
  const errors = watchErrors(rescuer);
  const resetLink = async (): Promise<string | undefined> =>
    (await inbox(base))
      .filter((mail) => mail.recipient === email)
      .flatMap((mail) => mail.decoded.split(/\r?\n/))
      .find((line) => line.startsWith(base) && line.includes('/reset-password#'))
      ?.trim();
  await expect.poll(resetLink).toBeTruthy();
  await rescuer.goto((await resetLink()) ?? '');
  await rescuer.getByLabel('New password').fill('A recovered password 1');
  await rescuer.getByRole('button', { name: 'Reset password', exact: true }).click();
  await expect(rescuer.getByRole('status')).toContainText('Your password has been reset');
  expect(errors).toEqual([]);
  await elsewhere.close();
}
