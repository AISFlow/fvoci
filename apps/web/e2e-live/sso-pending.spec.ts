/**
 * Another client secret saved while the organization takes single sign-on
 * alone, on the real server with the fake provider beside it: the screen
 * keeps it aside as pending settings, people go on signing in through the
 * provider as it is, a sign-in through a wrong secret changes nothing, and
 * one through the right secret (the provider's rotated one) puts it in use.
 *
 * It runs with the other tests that close the organization to passwords,
 * after the rest (`playwright.live.config.ts`), and takes the provider and
 * the rule away again at the end, whatever came of it.
 */
import { expect, request, test, type Page } from '@playwright/test';

import { ADMIN, operator, signedIn, uniqueName, type Schemas } from './support.ts';
import {
  NEXT_SECRET,
  forgetIdentities,
  pictures,
  providerSignsIn,
  requireOnScreen,
  settle,
} from './sso-support.ts';

// The fake provider's certificate is its own.
test.use({ locale: 'en-US', ignoreHTTPSErrors: true });

/** A secret the fake provider does not take. */
const WRONG_SECRET = 'a secret the provider never issued';

/** Saves `secret` on the screen; while the organization takes no password it waits aside. */
async function saveSecret(page: Page, secret: string): Promise<void> {
  const section = page.getByTestId('org-sso');
  await section.getByRole('button', { name: 'Change secret' }).click();
  await section.getByTestId('sso-secret').fill(secret);
  await section.getByTestId('sso-save').click();
  await expect(section.getByRole('status')).toContainText('Saved as pending');
  await expect(section.getByTestId('sso-pending')).toBeVisible();
}

/** Whether a new browser signs in through the provider as it is stored. */
async function signsInAsBefore(base: string, orgId: string): Promise<boolean> {
  const browser = await request.newContext({ baseURL: base, ignoreHTTPSErrors: true });
  try {
    const started = await browser.post(`/api/v1/auth/oidc/start?org=${orgId}`, {
      headers: { 'X-Requested-With': 'XMLHttpRequest' },
    });
    if (!started.ok()) return false;
    const { authorizationUrl } = (await started.json()) as Schemas['OidcStart'];
    const landed = await browser.get(authorizationUrl);
    return !landed.url().includes('oidc_error') && (await browser.get('/api/v1/users/me')).ok();
  } finally {
    await browser.dispose();
  }
}

test('a secret saved while single sign-on is required waits aside until a sign-in through it succeeds', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const api = await signedIn(base, ADMIN);
  const [workspace] = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const orgId = workspace?.orgId ?? '';
  const endpoint = `/api/v1/org/${orgId}/oidc`;
  const subject = uniqueName('pending-admin');

  try {
    await forgetIdentities(api);
    await requireOnScreen(page, orgId, subject);
    const section = page.getByTestId('org-sso');

    // A wrong secret: kept aside, and nobody is locked out by it.
    await saveSecret(page, WRONG_SECRET);
    await expect(section.getByTestId('sso-pending-client')).toHaveText('fvoci-test-client');
    expect(await page.content()).not.toContain(WRONG_SECRET);
    await pictures(page, 'pending');
    expect(await signsInAsBefore(base, orgId)).toBe(true);
    await section.getByTestId('sso-pending-try').click();
    await expect(section.getByRole('alert')).toContainText(
      'did not accept the client ID or secret',
    );
    expect(new URL(page.url()).search).toBe('');
    await expect(section.getByTestId('sso-pending')).toBeVisible();
    await expect(section.getByTestId('sso-required-switch')).toBeChecked();
    expect(await signsInAsBefore(base, orgId)).toBe(true);

    // The provider's next secret: tried, and put in use.
    await saveSecret(page, NEXT_SECRET);
    await section.getByTestId('sso-pending-try').click();
    await expect(section.getByRole('status')).toContainText('They are now in use');
    await expect(section.getByTestId('sso-pending')).toHaveCount(0);
    await expect(section.getByTestId('sso-required-switch')).toBeChecked();
    await expect(section.getByTestId('sso-required-switch')).toBeEnabled();
    await pictures(page, 'pending-applied');
    const records = page.getByTestId('org-records');
    await expect(records).toContainText(
      'signed in with the pending sign-in settings, which are now in use',
    );
    await expect(records).toContainText('saved new sign-in settings as pending');
    expect(await signsInAsBefore(base, orgId)).toBe(true);
  } finally {
    await settle('released', async () => {
      expect(await operator(base, `orgs/${orgId}/sso/release`)).toBe(204);
    });
    await settle('the provider is removed', async () => {
      if ((await api.get(endpoint)).ok()) expect((await api.delete(endpoint)).status()).toBe(204);
    });
    await settle('the identities are removed', () => forgetIdentities(api));
    await settle('the client ends', () => api.dispose());
    await settle('the provider signs in its usual person', () =>
      providerSignsIn({
        subject: 'oidc-person',
        email: 'oidc-person@example.test',
        preferredUsername: 'oidc-person',
        name: 'OIDC Person',
      }),
    );
  }
});
