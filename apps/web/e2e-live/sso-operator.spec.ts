/**
 * Single sign-on from the organization's screen to the operator's release,
 * on the real server with the fake provider beside it
 * (`tools/live-server.ts`): the administrator sets the provider up on the
 * screen, signs in through it and requires it; a member's password session
 * is told so; the person who runs the server releases the organization on
 * the administration socket, and the organization's records name the server
 * operator; the administrator disconnects the sign-in their session was made
 * through, is told beforehand that it signs them out, and is signed out.
 *
 * It runs with the test that closes the organization to passwords, after the
 * others (`playwright.live.config.ts`): the provider and the rule are taken
 * away again at the end, whatever came of the test.
 */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

import {
  ADMIN,
  invitation,
  operator,
  person,
  register,
  signedIn,
  signInOnPage,
  uniqueName,
  type Schemas,
} from './support.ts';
import {
  forgetIdentities,
  pictures,
  providerSignsIn,
  requireOnScreen,
  settle,
} from './sso-support.ts';

// The fake provider's certificate is its own.
test.use({ locale: 'en-US', ignoreHTTPSErrors: true });

/** The operator releases the organization: its records name the server operator. */
async function releasedByTheOperator(page: Page, base: string, orgId: string): Promise<void> {
  expect(await operator(base, `orgs/${orgId}/sso/release`)).toBe(204);
  await page.reload();
  const records = page.getByTestId('org-records');
  await expect(records).toContainText(
    "Server operator released the organization's single sign-on requirement",
  );
  await expect(records).toContainText('required single sign-on for the organization');
  // Read after the release: the switch is off as stored.
  await expect(page.getByTestId('org-sso').getByTestId('sso-required-switch')).not.toBeChecked();
  await records.getByText('Server operator released').scrollIntoViewIfNeeded();
  await pictures(page, 'records');
}

/**
 * Disconnecting the sign-in the page's session was made through says first
 * that it signs out, then does: the session is over at the server too, and
 * the identity `subject` is gone while the password session `api` goes on.
 */
async function disconnectingSignsOut(
  page: Page,
  api: APIRequestContext,
  subject: string,
): Promise<void> {
  await page.goto('/');
  await page.getByTestId('open-settings').click();
  const linked = page.getByTestId('settings-dialog').getByTestId('linked-sign-ins');
  await expect(linked.getByTestId('linked-sign-in')).toHaveCount(1);
  await linked.getByTestId('linked-sign-in').getByRole('button', { name: 'Disconnect' }).click();
  await expect(linked.getByTestId('linked-ends-session')).toContainText(
    'disconnecting it signs you out',
  );
  await pictures(page, 'disconnect');
  await linked.getByLabel('Current password').fill(ADMIN.password);
  await linked.getByRole('button', { name: 'Disconnect it' }).click();
  await expect(page.getByTestId('sign-in-form')).toBeVisible();
  expect((await page.request.get('/api/v1/users/me')).status()).toBe(401);
  const left = (await (
    await api.get('/api/v1/users/me/identities')
  ).json()) as Schemas['LinkedIdentity'][];
  expect(left.map((identity) => identity.subject)).not.toContain(subject);
}

test('an administrator requires single sign-on on the screen, the operator releases it, and disconnecting signs out', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  await invitation(base);
  const member = person('operator-member');
  await register(base, member);
  const api = await signedIn(base, ADMIN);
  const [workspace] = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const orgId = workspace?.orgId ?? '';
  const endpoint = `/api/v1/org/${orgId}/oidc`;
  const subject = uniqueName('operator-admin');
  const memberContext = await browser.newContext({ locale: 'en-US' });

  try {
    await forgetIdentities(api);
    // The member works with their password before the organization takes none.
    const memberPage = await memberContext.newPage();
    await memberPage.goto('/');
    await signInOnPage(memberPage, member);
    await expect(memberPage.getByTestId('app-shell')).toBeVisible();

    await requireOnScreen(page, orgId, subject);
    // The member's password session is told so in place of what they had open.
    await memberPage.reload();
    await expect(memberPage.getByTestId('sso-required')).toBeVisible();
    await expect(memberPage.getByTestId('sign-in-form')).toHaveCount(0);

    await releasedByTheOperator(page, base, orgId);
    await memberPage.reload();
    await expect(memberPage.getByTestId('app-shell')).toBeVisible();
    await expect(memberPage.getByTestId('sso-required')).toHaveCount(0);

    await disconnectingSignsOut(page, api, subject);
  } finally {
    // The other tests' people sign in with passwords: each step is taken
    // whatever came of the one before, and one that failed is told.
    await settle('the member page closes', () => memberContext.close());
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
