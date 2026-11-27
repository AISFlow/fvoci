/**
 * The reader's own space on the real server: the switcher puts it first;
 * entered, it starts empty; what is made there is the reader's alone, shared
 * by its public link only, and kept; deleting the account says that it goes
 * with it. An account that signs in through its organization's provider only
 * is asked, once it makes something there, to set a password, and sets it,
 * signing in through the provider again first where its session is too old;
 * set, the session that set it shows it and stays in My space, which goes on
 * hearing its changes; another sign-in through the provider is asked for it in
 * place of the board, and the same board opens, where the page is, to the
 * session that gives it.
 *
 * With `FVOCI_SCREENS` set, the screens are pictured light and dark, wide and
 * 390 pixels wide, under `target/screens/own-space/`.
 */
import {
  expect,
  test,
  type Browser,
  type Locator,
  type Page,
  type Response,
} from '@playwright/test';
import path from 'node:path';

import { screensDir } from '../e2e/screens.ts';
import { showSidebar, sidebar } from '../e2e/shell.support.ts';

import { CLIENT_ID, ISSUER, SECRET, providerSignsIn } from './sso-support.ts';
import {
  ADMIN,
  ageSessions,
  person,
  register,
  signedIn,
  signInOnPage,
  uniqueName,
  watchErrors,
  type Schemas,
} from './support.ts';

// The fake provider's certificate is its own.
test.use({ locale: 'en-US', ignoreHTTPSErrors: true });

const switcher = (page: Page): Locator => sidebar(page).getByTestId('workspace-switcher');
const workspacesMenu = (page: Page): Locator =>
  page.getByRole('menu', { name: 'Switch workspace', exact: true });
const notice = (page: Page): Locator => page.getByTestId('password-notice');

/** Opens the switcher. */
async function openSwitcher(page: Page): Promise<void> {
  await showSidebar(page);
  await switcher(page).click();
  await expect(workspacesMenu(page)).toBeVisible();
}

/** Goes to the reader's own space through the switcher. */
async function toOwnSpace(page: Page): Promise<void> {
  await openSwitcher(page);
  await workspacesMenu(page).getByRole('menuitem', { name: 'My space', exact: true }).click();
  await expect(page.getByTestId('workspace-start')).toBeVisible();
}

/** Makes an empty board from the own space's first screen. */
async function boardFromStart(page: Page): Promise<void> {
  await page.getByTestId('workspace-start-add').click();
  await page.getByTestId('template-picker').getByTestId('empty-board').click();
  await expect(page.getByTestId('board-page')).toBeVisible();
}

/**
 * Pictures the page's address light and dark, wide and 390 pixels wide, with
 * nothing wider than the window: `show` brings up what is pictured on the page
 * loaded again.
 */
async function pictures(page: Page, name: string, show: (seen: Page) => Promise<void>) {
  if (!process.env.FVOCI_SCREENS) return;
  const address = page.url();
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme });
    for (const [label, width, height] of [
      ['wide', 1280, 800],
      ['390', 390, 844],
    ] as const) {
      await page.setViewportSize({ width, height });
      await page.goto(address);
      await show(page);
      const file = path.join(screensDir('own-space'), `${name}-${label}-${colorScheme}.png`);
      await page.screenshot({ path: file, animations: 'disabled' });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
        width,
      );
    }
  }
  await page.emulateMedia({ colorScheme: null });
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto(address);
}

test('an invited member works alone in their own space, shares by link, and is told it goes with the account', async ({
  page,
  baseURL,
}) => {
  // Pictured, each screen is loaded four times more.
  test.slow(Boolean(process.env.FVOCI_SCREENS));
  const base = baseURL ?? '';
  const who = person('own-space');
  await register(base, who);
  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, who);
  // The app opens where the member works with others; the own space comes first in the list.
  await openSwitcher(page);
  const items = workspacesMenu(page).getByRole('menuitem');
  await expect(items.first()).toHaveAccessibleName('My space');
  await expect(items.nth(1)).toHaveAccessibleName(/, current$/u);
  await pictures(page, 'switcher', openSwitcher);
  await page.keyboard.press('Escape');

  await toOwnSpace(page);
  const start = page.getByTestId('workspace-start');
  await expect(start.getByRole('heading', { name: 'My space' })).toBeVisible();
  await expect(start).toContainText('yours alone');
  await expect(page.getByTestId('open-people')).toHaveCount(0);
  await pictures(page, 'own-space-empty', (seen) =>
    expect(seen.getByTestId('workspace-start')).toBeVisible(),
  );

  await boardFromStart(page);
  const boardAddress = page.url();
  await expect(page.getByTestId('board-people')).toHaveCount(0);
  // Something is made here: the account, which has a password, is told once where its reset mail goes.
  const told = page.getByTestId('email-notice');
  await expect(told).toContainText('If that is a company address, change it to a personal one');
  await told.getByTestId('email-notice-dismiss').click();
  await expect(told).toHaveCount(0);
  // Sharing is a button of the bar from two columns, and a line of its "…" on a phone.
  const openShare = async (seen: Page): Promise<void> => {
    const button = seen.getByTestId('share-board');
    if (await button.isVisible()) await button.click();
    else {
      await seen.getByTestId('bar-menu').click();
      await seen.getByRole('menuitem', { name: 'Share' }).click();
    }
    await expect(seen.getByTestId('share-public-section')).toBeVisible();
  };
  await openShare(page);
  const dialog = page.getByTestId('share-dialog');
  await expect(dialog.getByTestId('share-open')).toHaveCount(0);
  await expect(dialog.getByTestId('share-members')).toHaveCount(0);
  await dialog.getByTestId('share-public').click();
  await expect(dialog.getByTestId('share-link')).toHaveValue(/\/shared\/[0-9a-f-]{36}\?r=[^&]+$/u);
  await pictures(page, 'share-dialog', openShare);

  // Kept: loaded again, the board is there, in the own space.
  await page.reload();
  await expect(page.getByTestId('board-page')).toBeVisible();
  expect(page.url()).toBe(boardAddress);
  await expect(page.getByTestId('email-notice')).toHaveCount(0);
  await showSidebar(page);
  await expect(switcher(page)).toHaveAccessibleName('Switch workspace: My space');

  // Nobody else sees it: the administrator has no such workspace, nor the board where they work.
  const ownId = new URL(boardAddress).pathname.split('/')[2] ?? '';
  const admin = await signedIn(base, ADMIN);
  const theirs = (await (
    await admin.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  expect(theirs.map((one) => one.id)).not.toContain(ownId);
  expect((await admin.get(`/api/v1/w/${ownId}/boards`)).status()).toBe(404);
  await admin.dispose();

  const openDeletion = async (seen: Page): Promise<void> => {
    // At a phone's width the settings are in the drawer.
    await showSidebar(seen);
    await seen.getByTestId('open-settings').click();
    const section = seen.getByTestId('settings-dialog').getByTestId('account-deletion');
    await section.getByRole('button', { name: 'Delete account' }).click();
    const own = section.getByTestId('account-deletion-own-space');
    await expect(own).toBeVisible();
    await own.scrollIntoViewIfNeeded();
  };
  await openDeletion(page);
  await expect(page.getByTestId('account-deletion-own-space')).toContainText(
    'My space is deleted with your account, with everything in it: 1 board',
  );
  // It is no organization that has nobody else.
  await expect(page.getByTestId('account-deletion-organizations')).toHaveCount(0);
  await pictures(page, 'account-deletion', openDeletion);
  expect(errors).toEqual([]);
});

/** Signs in through the provider in a new browser as a new person, and goes to their own space. */
async function newcomer(browser: Browser, prefix: string) {
  const subject = uniqueName(prefix);
  await providerSignsIn({ subject, email: `${subject}@example.test`, preferredUsername: subject });
  const context = await browser.newContext({ locale: 'en-US', ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto('/');
  await page.getByTestId('oidc-continue').click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  // Where they work with others, nothing is asked.
  await expect(notice(page)).toHaveCount(0);
  await toOwnSpace(page);
  return { page, context, errors, subject };
}

/** Signs the person `subject` in through the provider once more, in a new browser. */
async function again(browser: Browser, subject: string) {
  await providerSignsIn({ subject, email: `${subject}@example.test`, preferredUsername: subject });
  const context = await browser.newContext({ locale: 'en-US', ignoreHTTPSErrors: true });
  const page = await context.newPage();
  const errors = watchErrors(page);
  await page.goto('/');
  await page.getByTestId('oidc-continue').click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  return { page, context, errors };
}

/** Waits for the page loaded again to have read the state the account keeps. */
async function reloadRead(page: Page): Promise<void> {
  const read = page.waitForResponse(
    (answer) => answer.url().endsWith('/api/v1/users/me/client-state') && answer.ok(),
  );
  await page.reload();
  await read;
  await expect(page.getByTestId('board-page')).toBeVisible();
}

test('an account of single sign-on only is asked once in its own space to set a password, and sets it', async ({
  browser,
  baseURL,
}) => {
  test.slow();
  const base = baseURL ?? '';
  const admin = await signedIn(base, ADMIN);
  const [shared] = (await (
    await admin.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const endpoint = `/api/v1/org/${shared?.orgId ?? ''}/oidc`;
  const set = await admin.put(endpoint, {
    data: { issuer: ISSUER, clientId: CLIENT_ID, clientSecret: SECRET },
  });
  expect(set.ok(), await set.text()).toBe(true);
  try {
    // Nothing is asked before something is made; "Don't show again" is kept with the account.
    const first = await newcomer(browser, 'notice-dismissed');
    await expect(notice(first.page)).toHaveCount(0);
    await boardFromStart(first.page);
    await expect(notice(first.page)).toContainText('nothing else opens My space');
    await first.page.getByTestId('password-notice-dismiss').click();
    await expect(notice(first.page)).toHaveCount(0);
    await reloadRead(first.page);
    await expect(notice(first.page)).toHaveCount(0);
    expect(first.errors).toEqual([]);
    await first.context.close();

    const second = await newcomer(browser, 'notice-set');
    const { page } = second;
    await boardFromStart(page);
    await expect(notice(page)).toBeVisible();
    await pictures(page, 'password-notice', (seen) => expect(notice(seen)).toBeVisible());
    // A session older than five minutes may not give the account its password.
    ageSessions(base, 6);
    await page.getByTestId('password-notice-field').fill('a long password of mine');
    await notice(page).getByRole('button', { name: 'Set password' }).click();
    await expect(page.getByTestId('password-notice-problem')).toHaveText(
      'Sign in through single sign-on again, then set it within five minutes.',
    );
    const boardAddress = page.url();
    const returned = page.waitForResponse((answer) =>
      answer.url().includes('/api/v1/auth/oidc/callback/'),
    );
    await page.getByTestId('password-notice-provider').click();
    await returned;
    // Through the provider and back to the same board, where the notice asks again.
    await expect(page.getByTestId('board-page')).toBeVisible();
    expect(page.url()).toBe(boardAddress);
    await page.getByTestId('password-notice-field').fill('a long password of mine');
    // Whether the locked space is shown at any moment from here on, also between two looks.
    await page.evaluate(() => {
      const seen = window as unknown as { locked?: boolean };
      new MutationObserver(() => {
        if (document.querySelector('[data-testid="own-space-locked"]')) seen.locked = true;
      }).observe(document.body, { childList: true, subtree: true });
    });
    // Read before the password is set: the session's cookie changes twice on the way, and
    // a reading of the test's own sent between the two is refused.
    const me = (await (await page.request.get('/api/v1/users/me')).json()) as Schemas['Me'];
    await notice(page).getByRole('button', { name: 'Set password' }).click();
    // Set, My space takes only a session that has shown the password, and this one shows
    // the one it was just given: the notice says it is set and by what name to sign in, the
    // board is where it was, and nothing asks for the password, not for a moment either.
    await expect(page.getByTestId('password-notice-done')).toContainText(
      `Your password is set. Your user name is ${me.username}.`,
    );
    await expect(page.getByTestId('board-page')).toBeVisible();
    await expect(page.getByTestId('own-space-locked')).toHaveCount(0);
    expect(page.url()).toBe(boardAddress);
    await expect
      .poll(async () => {
        const read = await page.request.get('/api/v1/users/me');
        return ((await read.json()) as Schemas['Me']).hasPassword;
      })
      .toBe(true);
    // Closed, nothing takes its place: the line about the address is for another visit.
    await notice(page).getByRole('button', { name: 'Close' }).click();
    await expect(notice(page)).toHaveCount(0);
    await expect(page.getByTestId('email-notice')).toHaveCount(0);
    const [, , ownId = '', , boardId = ''] = new URL(boardAddress).pathname.split('/');
    await expect
      .poll(async () => {
        const listed = (await (
          await page.request.get('/api/v1/users/me/workspaces')
        ).json()) as Schemas['UserWorkspace'][];
        return listed.find((one) => one.id === ownId)?.passwordRequired;
      })
      .toBe(false);
    // The page's connection is the one it had, and still hears the space: a change made
    // elsewhere in the session comes without loading the page again.
    await page.evaluate(() => {
      (window as unknown as { stayed?: boolean }).stayed = true;
    });
    const renamed = await page.request.patch(`/api/v1/w/${ownId}/containers/${boardId}`, {
      data: { title: 'Renamed elsewhere' },
      // As the app sends every request that changes something.
      headers: { 'X-Requested-With': 'XMLHttpRequest' },
    });
    expect(renamed.ok(), await renamed.text()).toBe(true);
    await expect(page.getByTestId('top-bar').getByTestId('board-title')).toHaveValue(
      'Renamed elsewhere',
    );
    expect(await page.evaluate(() => (window as unknown as { stayed?: boolean }).stayed)).toBe(
      true,
    );
    expect(
      await page.evaluate(() => (window as unknown as { locked?: boolean }).locked),
      'the locked space was never shown',
    ).toBeUndefined();
    expect(second.errors).toEqual([]);
    await second.context.close();

    // Another sign-in through the provider has shown nothing: where the board was, the
    // space asks for the password.
    const third = await again(browser, second.subject);
    const { page: later } = third;
    await later.goto(boardAddress);
    const locked = later.getByTestId('own-space-locked');
    await expect(locked).toBeVisible();
    await expect(later.getByTestId('own-space-locked-password')).toBeFocused();
    await expect(later.getByTestId('own-space-locked-user')).toHaveText(
      `User name: ${me.username}`,
    );
    const refused = await later.request.get(`/api/v1/w/${ownId}/boards`);
    expect(refused.status()).toBe(401);
    expect(((await refused.json()) as { reason?: string }).reason).toBe(
      'password_sign_in_required',
    );
    // The sidebar names it, has no people of it to manage, and the switcher marks how it opens.
    await openSwitcher(later);
    await expect(switcher(later)).toContainText('My space');
    await expect(later.getByTestId('open-people')).toHaveCount(0);
    await expect(workspacesMenu(later).locator(`[data-action="${ownId}"]`)).toHaveAttribute(
      'aria-label',
      'My space, opens with your password, current',
    );
    await pictures(later, 'switcher-locked', openSwitcher);
    await later.keyboard.press('Escape');
    // The session goes on where it opens: the workspace people share.
    expect((await later.request.get('/api/v1/users/me')).ok()).toBe(true);
    await pictures(later, 'own-space-locked', (seen) =>
      expect(seen.getByTestId('own-space-locked')).toBeVisible(),
    );
    // Deleting the account: My space goes with it, what is in it not counted for this session.
    await showSidebar(later);
    await later.getByTestId('open-settings').click();
    const deletion = later.getByTestId('settings-dialog').getByTestId('account-deletion');
    await deletion.getByRole('button', { name: 'Delete account' }).click();
    await expect(deletion.getByTestId('account-deletion-own-space')).toHaveText(
      'My space is deleted with your account, with everything in it.',
    );
    // Loaded again, the list says it is closed: nothing under it is asked, the guidance stands.
    const askedOfIt: string[] = [];
    const listen = (response: Response): void => {
      if (response.url().includes(`/api/v1/w/${ownId}`)) askedOfIt.push(response.url());
    };
    later.on('response', listen);
    await later.reload();
    await expect(locked).toBeVisible();
    await expect(later.getByTestId('workspace-switcher')).toBeAttached();
    later.off('response', listen);
    expect(askedOfIt).toEqual([]);
    // A wrong password is said; the right one opens the same board again, where the page
    // is: the session is the one the provider made, and the page is not loaded again.
    await later.evaluate(() => {
      (window as unknown as { stayed?: boolean }).stayed = true;
    });
    const session = (await (await later.request.get('/api/v1/users/me')).json()) as Schemas['Me'];
    await later.getByTestId('own-space-locked-password').fill('not my password at all');
    await locked.getByRole('button', { name: 'Open My space' }).click();
    await expect(later.getByTestId('own-space-locked-problem')).toHaveText(
      'The current password is wrong.',
    );
    await later.getByTestId('own-space-locked-password').fill('a long password of mine');
    await locked.getByRole('button', { name: 'Open My space' }).click();
    await expect(later.getByTestId('board-page')).toBeVisible();
    expect(later.url()).toBe(boardAddress);
    expect(await later.evaluate(() => (window as unknown as { stayed?: boolean }).stayed)).toBe(
      true,
    );
    expect(((await (await later.request.get('/api/v1/users/me')).json()) as Schemas['Me']).id).toBe(
      session.id,
    );
    const listed = (await (
      await later.request.get('/api/v1/users/me/workspaces')
    ).json()) as Schemas['UserWorkspace'][];
    expect(listed.find((one) => one.id === ownId)?.passwordRequired).toBe(false);
    expect(third.errors).toEqual([]);
    await third.context.close();
  } finally {
    await providerSignsIn({
      subject: 'oidc-person',
      email: 'oidc-person@example.test',
      preferredUsername: 'oidc-person',
      name: 'OIDC Person',
    });
    expect((await admin.delete(endpoint)).status()).toBe(204);
    await admin.dispose();
  }
});
