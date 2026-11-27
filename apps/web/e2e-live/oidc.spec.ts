/**
 * Signing in through the organization's provider on the real server: the
 * fake provider of the server's tests runs beside it (`tools/live-server.ts`),
 * the administrator sets it as the organization's, and somebody who has no
 * account yet signs in with the button of the sign-in page and opens a board.
 */
import { expect, test } from '@playwright/test';

import {
  ORG_NAME,
  ADMIN,
  boardPath,
  invitation,
  makeBoard,
  signedIn,
  signInOnPage,
  watchErrors,
  type Schemas,
} from './support.ts';

// The fake provider's certificate is its own.
test.use({ locale: 'en-US', ignoreHTTPSErrors: true });

const PORT = Number(process.env.FVOCI_LIVE_PORT ?? 18510);
/** Where `tools/live-server.ts` runs the fake provider, and its client. */
const ISSUER = `https://127.0.0.1:${PORT + 1}`;
const CLIENT_ID = 'fvoci-test-client';
const CLIENT_SECRET = 'fvoci test secret: not one to keep';

test('a newcomer signs in through the provider and opens a board', async ({ page, baseURL }) => {
  const base = baseURL ?? '';
  await invitation(base);
  const admin = await signedIn(base, ADMIN);
  const [workspace] = (await (
    await admin.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const orgId = workspace?.orgId ?? '';
  const set = await admin.put(`/api/v1/org/${orgId}/oidc`, {
    data: { issuer: ISSUER, clientId: CLIENT_ID, clientSecret: CLIENT_SECRET },
  });
  expect(set.ok(), await set.text()).toBe(true);
  const provider = (await set.json()) as Schemas['OidcProvider'];
  expect(provider.hasClientSecret).toBe(true);
  expect(JSON.stringify(provider)).not.toContain(CLIENT_SECRET);
  const board = await makeBoard(admin, { title: 'Through the provider', visibility: 'open' });

  const errors = watchErrors(page);
  await page.goto('/');
  const button = page.getByTestId('oidc-continue');
  // Named after the organization, which has the name its first user gave it.
  await expect(button).toHaveText(`Continue with ${ORG_NAME}`);
  await button.click();
  // Signed in: the app, wherever it opens, and not the sign-in page.
  await expect(page.getByTestId('app-shell')).toBeVisible();
  expect(new URL(page.url()).pathname).not.toBe('/login');

  await page.goto(boardPath(board, board.viewId));
  await expect(page.getByTestId('board-page')).toBeVisible();
  // Open to them, and theirs to write in: the first write joins it.
  await expect(page.getByTestId('top-bar').getByTestId('board-title')).toHaveValue(
    'Through the provider',
  );
  expect(errors).toEqual([]);
  // The other files' sign-in pages are as they were: no provider.
  expect((await admin.delete(`/api/v1/org/${orgId}/oidc`)).status()).toBe(204);
  await admin.dispose();
});

test('a sign-in through the provider that went wrong says why, and a password still signs in there', async ({
  page,
  baseURL,
}) => {
  await invitation(baseURL ?? '');
  const errors = watchErrors(page);
  // A return from a provider that no sign-in of this browser started: the server
  // sends it to the sign-in page with the name of what went wrong.
  await page.goto(
    `/api/v1/auth/oidc/callback/${crypto.randomUUID()}?state=never-started&code=anything`,
  );
  expect(new URL(page.url()).pathname).toBe('/login');
  expect(new URL(page.url()).searchParams.get('oidc_error')).toBe('oidc-state-invalid');
  await expect(page.getByTestId('sign-in-form').getByRole('alert')).toHaveText(
    'That sign-in did not come back as it was started. Start it again.',
  );
  await page.goto('/login?oidc_error=oidc-email-taken');
  await expect(page.getByTestId('sign-in-form').getByRole('alert')).toHaveText(
    'An account with your email address exists already. Sign in to it with its password, then connect single sign-on to it in the settings, under Connected sign-ins.',
  );
  // Signed in with the password from there, the app opens, not "page not found".
  await signInOnPage(page, ADMIN);
  await expect(page.getByText('There is nothing at this address.')).toHaveCount(0);
  expect(new URL(page.url()).pathname).not.toBe('/login');
  expect(errors).toEqual([]);
});
