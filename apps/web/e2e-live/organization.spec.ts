/**
 * The organization on the real server: its administrator makes a workspace on
 * the organization screen, joins it, goes there with the switcher at the head
 * of the sidebar and makes a board. A workspace's administrator opens a board
 * for the invited only that they manage without reading, joins it from there
 * and reads it; another member reads in the workspace's records that it was
 * joined. With `FVOCI_SCREENS` set the screens are pictured under
 * `target/screens/org/`, wide and at 390 pixels.
 */
import { expect, test, type Browser, type Page } from '@playwright/test';

import { ON_A_VIEW } from '../e2e/address.ts';
import { screensDir } from '../e2e/screens.ts';
import { boardLink, sidebar } from '../e2e/shell.support.ts';

import {
  ADMIN,
  boardPath,
  invitation,
  madeABoard,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  uniqueName,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

/** Pictures the page, wide and narrow, when asked to. */
async function picture(page: Page, name: string, area = 'org'): Promise<void> {
  if (!process.env.FVOCI_SCREENS) return;
  const wide = page.viewportSize();
  await page.screenshot({ path: `${screensDir(area)}/${name}-wide.png`, fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: `${screensDir(area)}/${name}-390.png`, fullPage: true });
  if (wide) await page.setViewportSize(wide);
}

/** The same join explanation in Korean, with the signed-in account's automatic language. */
async function koreanJoinHint(browser: Browser, page: Page, name: string): Promise<void> {
  const context = await browser.newContext({
    locale: 'ko-KR',
    storageState: await page.context().storageState(),
  });
  const korean = await context.newPage();
  const errors = watchErrors(korean);
  await korean.goto(page.url());
  await expect(korean.locator('html')).toHaveAttribute('lang', 'ko');
  const row = korean.locator(`[data-testid="org-workspace"][data-name="${name}"]`);
  const hint = '참여한 일은 이 워크스페이스의 구성원 모두에게 보입니다.';
  await expect(
    row.getByRole('button', { name: `${name}에 관리자로 참여` }),
  ).toHaveAccessibleDescription(hint);
  await expect(row.getByText(hint, { exact: true })).toBeVisible();
  await expect(korean.getByTestId('org-workspaces').locator(':scope > p')).toHaveCount(0);
  await picture(korean, 'org-join-ko', 'chores');
  expect(errors).toEqual([]);
  await context.close();
}

test('the organization’s administrator makes a workspace, joins it, switches to it and makes a board', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  await invitation(base);
  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, ADMIN);

  await sidebar(page).getByTestId('open-organization').click();
  const screen = page.getByTestId('org-page');
  await expect(screen).toBeVisible();
  const name = uniqueName('Research');
  await screen.getByTestId('new-workspace-name').fill(name);
  await screen.getByRole('button', { name: 'Create', exact: true }).click();
  const row = screen.locator(`[data-testid="org-workspace"][data-name="${name}"]`);
  await expect(row).toContainText('Members: 0');
  const hint = 'Every member of the workspace will see that you joined.';
  const join = row.getByRole('button', { name: `Join ${name} as administrator` });
  await expect(join).toHaveAccessibleDescription(hint);
  await expect(row.getByText(hint, { exact: true })).toBeVisible();
  await expect(screen.getByTestId('org-workspaces').locator(':scope > p')).toHaveCount(0);
  await picture(page, 'org-join-en', 'chores');
  await koreanJoinHint(browser, page, name);
  await join.click();
  await expect(row.getByText(hint, { exact: true })).toHaveCount(0);
  await expect(row).toContainText('you joined as administrator');
  await expect(row.getByRole('button', { name: `Leave ${name}` })).toBeVisible();
  await expect(screen.getByTestId('org-records').locator('[data-action]').first()).toHaveAttribute(
    'data-action',
    'workspace_created',
  );
  await picture(page, 'organization');

  await screen.getByTestId('org-back').click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await sidebar(page).getByTestId('workspace-switcher').click();
  await page.getByRole('menuitem', { name }).click();
  await expect(sidebar(page).getByTestId('workspace-switcher')).toContainText(name);
  await picture(page, 'switcher');

  const made = page.waitForResponse(madeABoard);
  await sidebar(page).getByTestId('add-board').click();
  await page.getByTestId('template-picker').getByTestId('empty-board').click();
  await made;
  await expect(page).toHaveURL(ON_A_VIEW);
  const workspaceId = new URL(page.url()).pathname.split('/')[2];
  const api = await signedIn(base, ADMIN);
  const workspaces = (await (await api.get('/api/v1/users/me/workspaces')).json()) as {
    id: string;
    name: string;
  }[];
  expect(workspaces.find((one) => one.id === workspaceId)?.name).toBe(name);
  expect(errors).toEqual([]);
});

for (const entry of ['sidebar', 'address'] as const) {
  test(`a board one manages without reading opens from the ${entry} and is joined from its page`, async ({
    page,
    browser,
    baseURL,
  }) => {
    const base = baseURL ?? '';
    await invitation(base);
    const owner = person('owner');
    const colleague = person('colleague');
    await register(base, owner);
    await register(base, colleague);
    const title = uniqueName(`Salaries from ${entry}`);
    const board = await makeBoard(await signedIn(base, owner), {
      title,
      visibility: 'private',
      cards: [{ title: 'Raise for everyone', status: 'To do' }],
    });
    // The sidebar links the board itself; an address a member shares names a view of it.
    const address = boardPath(board, board.viewId);
    const onBoard = new RegExp(`${boardPath(board)}(/[^/]+)?$`);

    const errors = watchErrors(page);
    await page.goto(entry === 'sidebar' ? `/w/${board.workspaceId}` : address);
    await signInOnPage(page, ADMIN);
    if (entry === 'sidebar') await boardLink(page, title).click();
    const managed = page.getByTestId('board-managed');
    await expect(managed).toBeVisible();
    await expect(page).toHaveURL(entry === 'sidebar' ? onBoard : new RegExp(`${address}$`));
    await expect(managed.getByRole('heading')).toHaveText(title);
    await expect(managed).toContainText('every member of the workspace will see that you joined');
    await expect(page.getByTestId('board-page')).toHaveCount(0);
    await expect(page.getByText('Raise for everyone')).toHaveCount(0);
    await picture(page, `managed-board-${entry}`);
    const joinedFrom = Date.now();
    await managed.getByRole('button', { name: 'Join', exact: true }).click();
    await expect(page.getByTestId('board-page')).toBeVisible();
    await expect(page.getByText('Raise for everyone')).toBeVisible();
    await expect(page).toHaveURL(entry === 'sidebar' ? onBoard : new RegExp(`${address}$`));
    expect(errors).toEqual([]);

    const other = await browser.newContext({ locale: 'en-US' });
    const theirs = await other.newPage();
    await theirs.goto(`/w/${board.workspaceId}`);
    await signInOnPage(theirs, colleague);
    await sidebar(theirs).getByTestId('open-settings').click();
    await theirs.getByTestId('open-records').click();
    const records = theirs.getByTestId('workspace-records');
    const joins = records.locator('[data-action="self_added_to_container"]');
    await expect(joins.first()).toBeVisible();
    // Other tests join boards of this workspace too: this join is the one made since the click
    // (the server and the browser share a clock; a second spares a record kept in seconds).
    const ours = await joins.evaluateAll(
      (rows, since) =>
        rows.findIndex(
          (row) => Date.parse(row.querySelector('time')?.getAttribute('datetime') ?? '') >= since,
        ),
      joinedFrom - 1000,
    );
    expect(ours).toBeGreaterThanOrEqual(0);
    const joined = joins.nth(ours);
    await expect(joined).toContainText(`@${ADMIN.username}`);
    // The colleague does not read the board: its title is not told.
    await expect(joined).toContainText('a member of a board');
    await expect(records).not.toContainText(title);
    await picture(theirs, `records-${entry}`);
    await other.close();
  });
}
