/**
 * An organization that takes single sign-on alone moves to another provider,
 * on the real server with two fake providers beside it.
 *
 * The administrator saves the other provider's settings, which wait aside.
 * From a session made more than five minutes ago the sign-in through them
 * connects nothing; the screen asks to sign in again with the current provider
 * first, comes back, and the sign-in through the new settings then connects
 * the administrator's identity there to their account and puts the settings
 * in use. The provider before is kept for linking.
 *
 * A member the new provider knows under another name signs in through it: no
 * account is made, the sign-in page asks whether they had one, and by their
 * previous sign-in they come in to the account they had, with its board. The
 * records tell the administrator which identity each move connected.
 *
 * It runs with the other tests that close the organization to passwords
 * (`playwright.live.config.ts`), and by its name after `sso-operator.spec.ts`,
 * which finds the operator's release among the records by its words and expects
 * one: the release at the end of this test is one more. It takes the provider
 * and the rule away again, whatever came of it; the member's account, and the
 * sessions it made older, stay on the server, where no other test reads them.
 */
import { expect, test, type Browser, type BrowserContext } from '@playwright/test';

import {
  ADMIN,
  ageSessions,
  boardPath,
  makeBoard,
  operator,
  signedIn,
  uniqueName,
  type Schemas,
} from './support.ts';
import {
  ISSUER,
  NEXT_ISSUER,
  SECRET,
  forgetIdentities,
  pictures,
  providerSignsIn,
  requireOnScreen,
  settle,
} from './sso-support.ts';

// The fake providers' certificates are their own.
test.use({ locale: 'en-US', ignoreHTTPSErrors: true });

/** Who the fake providers sign in when no test has named anybody. */
const USUAL = {
  subject: 'oidc-person',
  email: 'oidc-person@example.test',
  preferredUsername: 'oidc-person',
  name: 'OIDC Person',
};

/** A browser of somebody who is not signed in, whose requests are the app's own. */
function visitor(browser: Browser, base: string): Promise<BrowserContext> {
  return browser.newContext({
    baseURL: base,
    ignoreHTTPSErrors: true,
    locale: 'en-US',
    extraHTTPHeaders: { 'X-Requested-With': 'XMLHttpRequest' },
  });
}

test('an administrator moves the organization to another provider, and a member continues with their account', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const api = await signedIn(base, ADMIN);
  const [workspace] = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const orgId = workspace?.orgId ?? '';
  const endpoint = `/api/v1/org/${orgId}/oidc`;
  const admin = uniqueName('move-admin');
  const member = uniqueName('move-member');

  try {
    await forgetIdentities(api);
    await requireOnScreen(page, orgId, admin);
    const section = page.getByTestId('org-sso');
    const records = page.getByTestId('org-records');

    // A member whose account the provider made, with a board of their own.
    await providerSignsIn({
      subject: member,
      email: `${member}@old.example.test`,
      preferredUsername: member,
    });
    const before = await visitor(browser, base);
    const first = await before.newPage();
    await first.goto(`/login?org=${orgId}`);
    await first.getByTestId('oidc-continue').click();
    await expect(first.getByTestId('app-shell')).toBeVisible();
    const memberId = (
      (await (await before.request.get('/api/v1/users/me')).json()) as Schemas['User']
    ).id;
    const board = await makeBoard(before.request, { title: `Board of ${member}` });
    await before.close();

    // The administrator saves the other provider's settings: they wait aside. The
    // session was made through the current provider a moment ago.
    await page.goto(`/org/${orgId}`);
    await section.getByTestId('sso-issuer').fill(NEXT_ISSUER);
    await section.getByTestId('sso-secret').fill(SECRET);
    await section.getByTestId('sso-save').click();
    await expect(section.getByRole('status')).toContainText('Saved as pending');
    await expect(section.getByTestId('sso-pending-issuer')).toHaveText(NEXT_ISSUER);
    await expect(section.getByTestId('sso-pending-binding')).toContainText('just now');
    await expect(section.getByTestId('sso-pending-sign-in-again')).toHaveCount(0);

    // From a session made more than five minutes ago, the sign-in through the new
    // settings connects nothing, and nothing changes.
    ageSessions(base, 6);
    await page.reload();
    await expect(section.getByTestId('sso-pending-binding')).toContainText(
      'sign in through the current provider again first',
    );
    await pictures(page, 'pending-sign-in-again', 'i2c');
    await providerSignsIn({ subject: `${admin}-next`, email: ADMIN.email }, NEXT_ISSUER);
    await section.getByTestId('sso-pending-try').click();
    await expect(section.getByRole('alert')).toContainText(
      'Sign in through the current provider again, then finish the sign-in',
    );
    await expect(section.getByTestId('sso-pending')).toBeVisible();
    await expect(section.getByTestId('sso-issuer')).toHaveValue(ISSUER);

    // Signing in again with the current provider comes back to this page; the
    // sign-in through the new settings then connects the identity and applies them.
    await providerSignsIn({ subject: admin, email: ADMIN.email });
    await section.getByTestId('sso-pending-sign-in-again').click();
    await expect(section.getByTestId('sso-pending-binding')).toContainText('just now');
    expect(new URL(page.url()).pathname).toBe(`/org/${orgId}`);
    await pictures(page, 'pending-signed-in-again', 'i2c');
    await section.getByTestId('sso-pending-try').click();
    await expect(section.getByRole('status')).toContainText('They are now in use');
    await expect(section.getByTestId('sso-pending')).toHaveCount(0);
    await expect(section.getByTestId('sso-issuer')).toHaveValue(NEXT_ISSUER);
    await expect(section.getByTestId('sso-required-switch')).toBeChecked();
    await expect(section.getByTestId('sso-former-issuer')).toHaveText(ISSUER);
    await expect(records).toContainText(
      `connected their identity at the new provider (${NEXT_ISSUER}) to their account`,
    );
    // Which identity it was, by the address the new provider gave.
    await expect(records).toContainText(`Identity connected: ${ADMIN.email}`);
    await pictures(page, 'moved', 'i2c');
    await expect(records).toContainText(`the previous one (${ISSUER}) is kept for linking`);

    // The member, whom the new provider knows under another name and address: no
    // account is made, and the sign-in page asks whether they had one.
    await providerSignsIn(
      { subject: `${member}-next`, email: `${member}@new.example.test` },
      NEXT_ISSUER,
    );
    await providerSignsIn({ subject: member, email: `${member}@old.example.test` });
    const after = await visitor(browser, base);
    const visit = await after.newPage();
    await visit.goto(`/login?org=${orgId}`);
    await visit.getByTestId('oidc-continue').click();
    const choice = visit.getByTestId('sign-in-choice');
    await expect(choice).toBeVisible();
    expect(new URL(visit.url()).searchParams.get('oidc_held')).toBe('1');
    expect((await after.request.get('/api/v1/users/me')).status()).toBe(401);
    await pictures(visit, 'sign-in-choice', 'i2c');

    // The same step for a reader whose browser asks for Korean.
    const korean = await browser.newContext({ baseURL: base, locale: 'ko-KR' });
    const asked = await korean.newPage();
    await asked.goto(`/login?org=${orgId}&oidc_held=1`);
    await expect(asked.getByTestId('sign-in-choice')).toContainText('쓰던 계정이 있나요?');
    await expect(asked.getByTestId('held-carry-over')).toHaveText(
      '이미 계정이 있음: 예전 로그인으로 잇기',
    );
    await pictures(asked, 'sign-in-choice-ko', 'i2c');
    await korean.close();

    // By their previous sign-in they are in the account they had, with its board.
    await choice.getByTestId('held-carry-over').click();
    await expect(visit.getByTestId('app-shell')).toBeVisible();
    const me = (await (await after.request.get('/api/v1/users/me')).json()) as Schemas['User'];
    expect(me.id).toBe(memberId);
    await visit.goto(boardPath(board, board.viewId));
    // A board of members only: it opens for the member who made it, and nobody else.
    await expect(visit.getByTestId('board-page')).toBeVisible();
    await expect(visit.getByText(`Board of ${member}`).first()).toBeVisible();
    await after.close();

    // The administrator sees who continued, and ends the linking period early.
    await page.reload();
    await expect(records).toContainText('confirmed their previous sign-in');
    await expect(records).toContainText(`Identity connected: ${member}@new.example.test`);
    await pictures(page, 'carried-over', 'i2c');
    await section.getByTestId('sso-former-end').click();
    await page.getByTestId('confirm-dialog').getByRole('button', { name: 'End early' }).click();
    await expect(section.getByRole('status')).toContainText('linking period was ended');
    await expect(section.getByTestId('sso-former')).toHaveCount(0);
    await expect(records).toContainText(`ended the linking period of the previous provider`);
  } finally {
    await settle('released', async () => {
      expect(await operator(base, `orgs/${orgId}/sso/release`)).toBe(204);
    });
    await settle('the provider is removed', async () => {
      if ((await api.get(endpoint)).ok()) expect((await api.delete(endpoint)).status()).toBe(204);
    });
    await settle('the identities are removed', () => forgetIdentities(api));
    await settle('the client ends', () => api.dispose());
    await settle('the providers sign in their usual person', async () => {
      await providerSignsIn(USUAL);
      await providerSignsIn(USUAL, NEXT_ISSUER);
    });
  }
});
