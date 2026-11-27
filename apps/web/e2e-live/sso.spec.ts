/**
 * An organization that takes no password, on the real server with the fake
 * provider of its tests beside it (`tools/live-server.ts`): a member connects
 * its single sign-on to their account in the settings; the organization then
 * begins to take no password; the member, still signed in with theirs, is
 * told so in place of the board, goes through the single sign-on, and is
 * back on the board they asked for, in the same account. Their own space,
 * which takes the account's password, then opens to that session once it
 * gives the password, and the two are gone between without signing in again.
 *
 * The server of these tests has one organization, which every test's people
 * are in: the provider and the rule are taken away again at the end, whatever
 * came of the test.
 */
import { expect, request, test, type APIRequestContext, type Page } from '@playwright/test';

import { boardLink, showSidebar, sidebar } from '../e2e/shell.support.ts';

import {
  ADMIN,
  boardPath,
  invitation,
  makeBoard,
  person,
  register,
  signInOnPage,
  uniqueName,
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
const PROVIDER = { issuer: ISSUER, clientId: CLIENT_ID };

/** A client of the API that takes the fake provider's certificate, signed in as nobody yet. */
async function trustingClient(baseURL: string): Promise<APIRequestContext> {
  return request.newContext({
    baseURL,
    ignoreHTTPSErrors: true,
    extraHTTPHeaders: { 'X-Requested-With': 'XMLHttpRequest' },
  });
}

/** Who the provider signs in from now on. */
async function providerSignsIn(who: {
  subject: string;
  email: string;
  preferredUsername?: string;
  name?: string;
}): Promise<void> {
  const provider = await request.newContext({ ignoreHTTPSErrors: true });
  expect((await provider.post(`${ISSUER}/person`, { data: who })).status()).toBe(204);
  await provider.dispose();
}

/** Goes through the provider as a browser does, in `api`: started here, and back to the server. */
async function throughProvider(
  api: APIRequestContext,
  orgId: string,
  linkPassword?: string,
): Promise<string> {
  const started = await api.post(`/api/v1/auth/oidc/start?org=${orgId}`, {
    data: linkPassword === undefined ? undefined : { linkPassword },
  });
  expect(started.ok(), await started.text()).toBe(true);
  const { authorizationUrl } = (await started.json()) as Schemas['OidcStart'];
  const back = await api.get(authorizationUrl);
  expect(back.ok(), back.url()).toBe(true);
  return back.url();
}

/** The reader's workspaces, as the server lists them to the page's session. */
async function listed(page: Page): Promise<Schemas['UserWorkspace'][]> {
  const answer = await page.request.get('/api/v1/users/me/workspaces');
  expect(answer.ok(), await answer.text()).toBe(true);
  return (await answer.json()) as Schemas['UserWorkspace'][];
}

/** Goes to a workspace of the reader's through the switcher. */
async function switchTo(page: Page, workspaceId: string): Promise<void> {
  await showSidebar(page);
  await sidebar(page).getByTestId('workspace-switcher').click();
  const menu = page.getByRole('menu', { name: 'Switch workspace', exact: true });
  await menu.locator(`[data-action="${workspaceId}"]`).click();
}

/**
 * The session single sign-on made, of an account with a password, in the
 * account's own space: asked for the password there, it gives it and the
 * space opens where the page is; from then on the organization that takes no
 * password and the own space are both open to it, to and fro and after the
 * page is loaded again, and nobody signs in again.
 */
async function betweenOwnSpaceAndOrganization(
  page: Page,
  password: string,
  organizationId: string,
): Promise<void> {
  const own = (await listed(page)).find((one) => one.kind === 'personal');
  expect(own?.passwordRequired, 'closed to the session single sign-on made').toBe(true);
  const ownId = own?.id ?? '';
  await page.evaluate(() => {
    (window as unknown as { stayed?: boolean }).stayed = true;
  });
  await switchTo(page, ownId);
  const locked = page.getByTestId('own-space-locked');
  await expect(locked).toBeVisible();
  await page.getByTestId('own-space-locked-password').fill('not the password at all');
  await locked.getByRole('button', { name: 'Open My space' }).click();
  await expect(page.getByTestId('own-space-locked-problem')).toHaveText(
    'The current password is wrong.',
  );
  await page.getByTestId('own-space-locked-password').fill(password);
  await locked.getByRole('button', { name: 'Open My space' }).click();
  await expect(page.getByTestId('workspace-start')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { stayed?: boolean }).stayed)).toBe(true);
  for (const workspace of await listed(page)) {
    expect([workspace.passwordRequired, workspace.ssoRequired], workspace.id).toEqual([
      false,
      false,
    ]);
  }

  // To the organization and back, and once more on a page loaded again.
  for (const reload of [false, true]) {
    await switchTo(page, organizationId);
    await expect(boardLink(page, 'Behind single sign-on')).toBeVisible();
    if (reload) await page.reload();
    await expect(page.getByTestId('sso-required')).toHaveCount(0);
    await expect(page.getByTestId('sign-in-form')).toHaveCount(0);
    await switchTo(page, ownId);
    if (reload) await page.reload();
    await expect(page.getByTestId('workspace-start')).toBeVisible();
    await expect(locked).toHaveCount(0);
  }
}

test('what takes no password is opened through its single sign-on, back on the board asked for', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  await invitation(base);
  const member = person('sso-member');
  await register(base, member);
  const admin = await trustingClient(base);
  const login = await admin.post('/api/v1/login', {
    data: { username: ADMIN.username, password: ADMIN.password },
  });
  expect(login.ok(), await login.text()).toBe(true);
  const [workspace] = (await (
    await admin.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const orgId = workspace?.orgId ?? '';
  const oidc = `/api/v1/org/${orgId}/oidc`;
  const set = await admin.put(oidc, { data: { ...PROVIDER, clientSecret: CLIENT_SECRET } });
  expect(set.ok(), await set.text()).toBe(true);
  const board = await makeBoard(admin, {
    title: 'Behind single sign-on',
    members: [{ who: member, role: 'editor' }],
    cards: [{ title: 'Kept behind the wall' }],
  });
  const address = boardPath(board, board.viewId);
  const memberThere = { subject: uniqueName('member-there'), email: member.email };
  const adminThere = { subject: uniqueName('admin-there'), email: ADMIN.email };
  /** The administrator's session made through the provider, once there is one. */
  let adminThrough: APIRequestContext | undefined;

  try {
    const errors = watchErrors(page);
    await page.goto(address);
    await signInOnPage(page, member);
    await expect(page.getByTestId('board-page')).toBeVisible();

    // The member connects the organization's single sign-on to their account.
    await providerSignsIn(memberThere);
    await page.getByTestId('open-settings').click();
    const linked = page.getByTestId('settings-dialog').getByTestId('linked-sign-ins');
    await expect(linked.getByText('Nothing is connected yet.')).toBeVisible();
    await linked.getByTestId('linked-connect').click();
    await linked.getByLabel('Current password').fill(member.password);
    await linked.getByRole('button', { name: 'Continue to single sign-on' }).click();
    // To the provider and back: the settings say it is connected, on the board they left.
    await expect(linked.getByRole('status')).toHaveText(
      'Connected. Single sign-on now signs in to this account.',
    );
    await expect(linked.getByTestId('linked-sign-in')).toContainText(`127.0.0.1:${PORT + 1}`);
    expect(new URL(page.url()).pathname).toBe(address);
    expect(new URL(page.url()).search).toBe('');
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('board-page')).toBeVisible();

    // The administrator does the same, signs in through it, and switches passwords off.
    await providerSignsIn(adminThere);
    expect(await throughProvider(admin, orgId, ADMIN.password)).toContain('oidc_linked=1');
    adminThrough = await trustingClient(base);
    await throughProvider(adminThrough, orgId);
    const unproven = await admin.put(oidc, { data: { ...PROVIDER, ssoRequired: true } });
    expect(unproven.status()).toBe(409);
    const required = await adminThrough.put(oidc, { data: { ...PROVIDER, ssoRequired: true } });
    expect(required.ok(), await required.text()).toBe(true);

    // The member's password session: told so in place of the board, and not signed out.
    await providerSignsIn(memberThere);
    await page.reload();
    const closed = page.getByTestId('sso-required');
    await expect(closed.getByRole('heading')).toContainText('signs in with single sign-on');
    await expect(page.getByTestId('board-page')).toHaveCount(0);
    await expect(page.getByTestId('sign-in-form')).toHaveCount(0);
    await expect(page.getByTestId('app-shell')).toBeVisible();

    // Through the single sign-on: the same account, on the board asked for.
    await closed.getByTestId('sso-continue').click();
    await expect(page.getByTestId('board-page')).toBeVisible();
    expect(new URL(page.url()).pathname).toBe(address);
    await expect(page.getByText('Kept behind the wall')).toBeVisible();
    await page.getByTestId('open-settings').click();
    await expect(page.getByTestId('signed-in-as')).toContainText(member.username);
    await page.keyboard.press('Escape');
    await betweenOwnSpaceAndOrganization(page, member.password, workspace?.id ?? '');
    expect(errors).toEqual([]);
  } finally {
    // The other tests' people sign in with passwords, and to pages without a
    // provider: each step is taken whatever came of the one before, and one
    // that failed is told, as the files after this one would fail for it.
    if (adminThrough) {
      const released = await adminThrough.put(oidc, { data: { ...PROVIDER, ssoRequired: false } });
      expect.soft(released.ok(), `passwords are taken again: ${released.status()}`).toBe(true);
      await adminThrough.dispose();
    }
    const removed = await admin.delete(oidc);
    expect.soft(removed.status(), 'the provider is removed').toBe(204);
    await admin.dispose();
    await providerSignsIn({
      subject: 'oidc-person',
      email: 'oidc-person@example.test',
      preferredUsername: 'oidc-person',
      name: 'OIDC Person',
    });
  }
});
