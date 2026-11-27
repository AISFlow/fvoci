/**
 * What the live tests of single sign-on share: the fake provider beside the
 * server (`tools/live-server.ts`), who it signs in, the pictures of the
 * screens, and the administrator requiring it on the organization's screen.
 */
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, request, type APIRequestContext, type Locator, type Page } from '@playwright/test';

import { ADMIN, signInOnPage, type Schemas } from './support.ts';

export const PORT = Number(process.env.FVOCI_LIVE_PORT ?? 18510);
export const ISSUER = `https://127.0.0.1:${PORT + 1}`;
/** The second fake provider, which an organization moves to: the same client and secrets. */
export const NEXT_ISSUER = `https://127.0.0.1:${PORT + 5}`;
export const CLIENT_ID = 'fvoci-test-client';
export const SECRET = 'fvoci test secret: not one to keep';
/** The provider's other secret, which it takes as well (a secret rotated). */
export const NEXT_SECRET = 'fvoci test secret: the next one';

/** Who the provider (the first one, or the one at `issuer`) signs in from now on. */
export async function providerSignsIn(
  who: { subject: string; email: string; preferredUsername?: string; name?: string },
  issuer: string = ISSUER,
): Promise<void> {
  const provider = await request.newContext({ ignoreHTTPSErrors: true });
  expect((await provider.post(`${issuer}/person`, { data: who })).status()).toBe(204);
  await provider.dispose();
}

/**
 * The page at both widths, under `target/screens/<folder>/` (`sso` unless named),
 * with nothing wider than the window.
 */
export async function pictures(page: Page, name: string, folder = 'sso'): Promise<void> {
  const directory = path.resolve('../../target/screens', folder);
  await mkdir(directory, { recursive: true });
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: path.join(directory, `${name}-${width}.png`), fullPage: true });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      width,
    );
  }
  await page.setViewportSize({ width: 1280, height: 900 });
}

/** Removes the identities `api`'s account has, as one left by an earlier test would make two. */
export async function forgetIdentities(api: APIRequestContext): Promise<void> {
  const tied = (await (
    await api.get('/api/v1/users/me/identities')
  ).json()) as Schemas['LinkedIdentity'][];
  for (const { issuer, subject } of tied) {
    const removed = await api.delete('/api/v1/users/me/identities', {
      data: { issuer, subject, password: ADMIN.password },
    });
    expect(removed.status(), await removed.text()).toBe(204);
  }
}

/** Opens the form of the organization's single sign-on, which is one line until it is set up. */
export async function openSsoForm(section: Locator): Promise<void> {
  const setUp = section.getByTestId('sso-set-up');
  const issuer = section.getByTestId('sso-issuer');
  await expect(issuer.or(setUp)).toBeVisible();
  if (!(await setUp.isVisible())) return;
  await setUp.click();
  // Setting up takes the reader to the first field of the form.
  await expect(issuer).toBeFocused();
}

/**
 * The administrator puts the provider in on the organization's screen,
 * connects their account to it as `subject`, signs in through it from the
 * section, and requires it: the provider cannot be removed meanwhile.
 */
export async function requireOnScreen(page: Page, orgId: string, subject: string): Promise<void> {
  await page.goto('/');
  await signInOnPage(page, ADMIN);
  await page.goto(`/org/${orgId}`);
  const section = page.getByTestId('org-sso');
  await openSsoForm(section);
  await section.getByTestId('sso-issuer').fill(ISSUER);
  await section.getByTestId('sso-client').fill(CLIENT_ID);
  await section.getByTestId('sso-secret').fill(SECRET);
  await section.getByTestId('sso-save').click();
  await expect(section.getByRole('status')).toContainText('Saved.');
  await expect(section.getByTestId('sso-required-switch')).toBeDisabled();

  await providerSignsIn({ subject, email: ADMIN.email });
  const started = await page.request.post(`/api/v1/auth/oidc/start?org=${orgId}`, {
    headers: { 'X-Requested-With': 'XMLHttpRequest' },
    data: { linkPassword: ADMIN.password },
  });
  expect(started.ok(), await started.text()).toBe(true);
  const { authorizationUrl } = (await started.json()) as Schemas['OidcStart'];
  expect((await page.request.get(authorizationUrl)).url()).toContain('oidc_linked=1');
  await section.getByTestId('sso-sign-in').click();
  await expect(section.getByTestId('sso-required-switch')).toBeEnabled();

  await section.getByTestId('sso-required-switch').click();
  await section.getByTestId('sso-save').click();
  // The save is answered, and what it stored is read back.
  await expect(section.getByTestId('sso-save')).toBeEnabled();
  await expect(section.getByTestId('sso-required-switch')).toBeChecked();
  await expect(section.getByTestId('sso-delete')).toBeDisabled();
}

/** Runs a step of the cleanup, and tells of its failure without stopping the others. */
export async function settle(what: string, step: () => Promise<unknown>): Promise<void> {
  try {
    await step();
  } catch (error) {
    expect.soft(error, what).toBeUndefined();
  }
}
