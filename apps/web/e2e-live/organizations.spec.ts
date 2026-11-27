/** A second organization uses the whole real app with PostgreSQL row policies active. */
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';

import { addressOf } from '../e2e/address.ts';
import { boardLink, sidebar } from '../e2e/shell.support.ts';

import {
  archiveRoundTrip,
  fileUnderCategory,
  firstOrganization,
  invitedColleague,
  observe,
  restoreBoard,
  startOrganization,
} from './organizations.support.ts';
import {
  apiClient,
  makeBoard,
  memberLink,
  person,
  placeOf,
  signedIn,
  signInOnPage,
  watchErrors,
  type Person,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

test('a second organization goes from signup through the full app to a shared live board', async ({
  page,
  browser,
  baseURL,
}, info) => {
  test.slow();
  const base = baseURL ?? '';
  const forbidden = await firstOrganization(base);
  const owner = person('second-founder');
  const observation = observe(page);
  const errors = watchErrors(page);
  await startOrganization(page, owner);
  await observation.screen();
  const api = await signedIn(base, owner);
  const { workspaceId } = placeOf(page.url());
  const listed = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  // The workspace of the organization, and after it the account's own space.
  expect(listed.map((item) => item.kind === 'personal')).toEqual([false, true]);
  expect(listed[0]?.id).toBe(workspaceId);

  await sidebar(page).getByTestId('add-board').click();
  await observation.screen();
  await page
    .getByTestId('template-picker')
    .getByRole('button', { name: 'Use this template: Issue tracker' })
    .click();
  await expect(boardLink(page, 'Issue tracker')).toHaveAttribute('aria-current', 'page');
  const title = 'Second planning board';
  await page.getByTestId('top-bar').getByTestId('board-title').fill(title);
  await page.getByTestId('top-bar').getByTestId('board-title').press('Enter');
  await expect(boardLink(page, title)).toBeVisible();
  const address = page.url();
  const { boardId } = placeOf(address);
  await fileUnderCategory(page, api, workspaceId, title);
  await observation.screen();

  // Made in the organization's space: open to it from the start.
  await page.getByTestId('share-board').click();
  await expect(page.getByTestId('share-dialog').getByTestId('share-open')).toBeChecked();
  await page.keyboard.press('Escape');
  await page
    .getByRole('button', { name: /^New card in / })
    .first()
    .click();
  await page.getByTestId('card-title').fill('Second launch draft');
  await page.goBack();
  await expect(page.getByText('Second launch draft', { exact: true })).toBeVisible();

  await page.getByTestId('open-people').click();
  const people = page.getByTestId('people-dialog');
  await expect(people).toContainText(owner.username);
  await people.getByTestId('people-make-link').click();
  await expect(people.getByTestId('people-link')).toHaveValue(/\/invite\/[\w-]{43}$/);
  const invitation = await people.getByTestId('people-link').inputValue();
  await observation.screen();
  await page.keyboard.press('Escape');

  await restoreBoard(page, title, () => observation.screen());
  await archiveRoundTrip(page, api, workspaceId, title, info, forbidden);
  await observation.screen();

  const colleague = await invitedColleague(browser, base, invitation, address);
  const other = colleague.page;
  await other.getByText('Second launch draft', { exact: true }).click();
  await other.getByTestId('card-title').fill('Second launch ready');
  await expect(page.getByText('Second launch ready', { exact: true })).toBeVisible();
  await expect
    .poll(() =>
      observation.frames.some(
        (frame) => frame.includes('Second launch ready') && frame.includes(boardId),
      ),
    )
    .toBe(true);
  await colleague.observation.screen();
  await observation.screen();
  await observation.assertIsolated(forbidden);
  await colleague.observation.assertIsolated(forbidden);
  expect([...errors, ...colleague.errors]).toEqual([]);
  await api.dispose();
  await colleague.context.close();
});

test('an account made on its own starts in its own space and starts organizations from the switcher up to the limit', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const errors = watchErrors(page);
  const owner = person('alone');
  await page.goto('/');
  await page.getByRole('button', { name: 'New here? Create an account' }).click();
  const form = page.getByTestId('register-form');
  await expect(form.getByTestId('register-solo')).toBeChecked();
  await expect(form.getByTestId('register-organization-name')).toHaveCount(0);
  await form.getByLabel(/^user name$/i).fill(owner.username);
  await form.getByLabel(/email/i).fill(owner.email);
  await form.getByLabel(/^password$/i).fill(owner.password);
  await form.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  // It starts in its own space, on its own copy of the welcome board of an own space.
  await expect(boardLink(page, 'Welcome to your personal space')).toHaveAttribute(
    'aria-current',
    'page',
  );
  const switcher = sidebar(page).getByTestId('workspace-switcher');
  await expect(switcher).toHaveAccessibleName('Switch workspace: My space');
  const api = await signedIn(base, owner);
  const listed = async () =>
    (await (await api.get('/api/v1/users/me/workspaces')).json()) as Schemas['UserWorkspace'][];
  expect((await listed()).map((item) => item.kind)).toEqual(['personal']);

  const start = async (name: string): Promise<void> => {
    await switcher.click();
    await page.locator('[data-action="new-organization"]').click();
    await page.getByTestId('new-organization-name').fill(name);
    await page.getByTestId('new-organization-submit').click();
  };
  await start('Design studio');
  await expect(switcher).toHaveAccessibleName('Switch workspace: Design studio');
  expect((await listed()).map((item) => item.kind === 'personal')).toEqual([false, true]);
  for (const name of ['Research', 'Support']) {
    expect((await api.post('/api/v1/org', { data: { name } })).status()).toBe(201);
  }
  // A fourth is refused, with the number the server allows.
  await start('One more');
  await expect(page.getByTestId('new-organization-problem')).toContainText('3');
  await expect(page.getByTestId('new-organization-dialog')).toBeVisible();
  expect(errors).toEqual([]);
  await api.dispose();
});

test('an organization started on a Korean screen begins on the welcome board in Korean', async ({
  browser,
  baseURL,
}) => {
  const context = await browser.newContext({ locale: 'ko-KR', baseURL: baseURL ?? '' });
  const page = await context.newPage();
  const errors = watchErrors(page);
  const owner = person('korean-founder');
  await page.goto('/');
  await page.getByRole('button', { name: '처음인가요? 계정 만들기' }).click();
  const form = page.getByTestId('register-form');
  await form.getByLabel('팀과 함께 쓰기').check();
  await form.getByLabel('팀 이름', { exact: true }).fill('한국어 조직');
  await form.getByLabel('사용자 이름', { exact: true }).fill(owner.username);
  await form.getByLabel('이메일 주소', { exact: true }).fill(owner.email);
  await form.getByLabel('비밀번호', { exact: true }).fill(owner.password);
  await form.getByRole('button', { name: '계정 만들기' }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  // The account's settings leave the language to the device: the browser's is taken.
  await expect(boardLink(page, 'fvoci 시작하기')).toHaveAttribute('aria-current', 'page');
  await expect(page.getByText('카드를 열어 보세요', { exact: true })).toBeVisible();
  expect(errors).toEqual([]);
  await context.close();
});

/** `who` registers, starting the organization `name`. */
async function founded(base: string, who: Person, name: string): Promise<void> {
  const anybody = await apiClient(base);
  const registered = await anybody.post('/api/v1/register', {
    data: { ...who, organizationName: name },
  });
  expect(registered.status()).toBe(201);
  await anybody.dispose();
}

/** An account made by a link of the organization's workspace `workspaceId`, as a newcomer's. */
async function joinedBy(base: string, api: APIRequestContext, workspaceId: string) {
  const newcomer = person('leaver');
  const token = (await memberLink(api, workspaceId)).token ?? '';
  const anybody = await apiClient(base);
  const accepted = await anybody.post('/api/v1/invitations/accept', {
    data: { token, account: newcomer },
  });
  expect(accepted.ok()).toBe(true);
  await anybody.dispose();
  return newcomer;
}

/** `who` signed in on a page of its own, at `address`. */
async function openedAt(page: Page, who: Person, address: string): Promise<void> {
  await page.goto(address);
  await signInOnPage(page, who);
}

/** Asks to leave the organization `name` in the settings, said in its dialog first. */
async function leaveInSettings(page: Page, name: string) {
  await page.getByTestId('open-settings').click();
  const section = page.getByTestId('settings-dialog').getByTestId('organization-leaving');
  await section.getByRole('button', { name: `Leave ${name}` }).click();
  const dialog = page.getByTestId('organization-leave-dialog');
  await expect(dialog).toContainText(`Leave ${name}?`);
  await expect(dialog).toContainText('you wrote stay');
  await dialog.getByRole('button', { name: 'Leave', exact: true }).click();
  return { section, dialog };
}

test('a member who leaves an organization finds it no more, and its only administrator may not', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  // Never the server's first account, whose refusal is that of the server's last administrator.
  await founded(base, person('before-leaving'), 'Earlier studio');
  const founder = person('leaving-founder');
  await founded(base, founder, 'Leaving studio');
  const api = await signedIn(base, founder);
  const [workspace] = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const workspaceId = workspace?.id ?? '';
  const name = workspace?.name ?? '';
  const leaver = await joinedBy(base, api, workspaceId);
  const board = await makeBoard(api, {
    title: 'Shared plans',
    workspaceId,
    members: [{ who: leaver, role: 'editor' }],
  });
  const address = addressOf({ workspaceId, boardId: board.boardId, viewId: board.viewId });
  const errors = watchErrors(page);
  await openedAt(page, leaver, address);
  // Another tab of the leaver's, on the same board, hears its cards.
  const other = await page.context().newPage();
  const heard: string[] = [];
  other.on('websocket', (socket) =>
    socket.on('framereceived', ({ payload }) => heard.push(String(payload))),
  );
  await other.goto(address);
  const card = (title: string) =>
    api.post(`/api/v1/w/${workspaceId}/boards/${board.boardId}/cards`, { data: { title } });
  await card('Heard before leaving');
  await expect(other.getByText('Heard before leaving', { exact: true })).toBeVisible();

  // The founder is its only administrator, beside a member: refused, with what to do first.
  const theirs = await browser.newContext({ locale: 'en-US', baseURL: base });
  const founderPage = await theirs.newPage();
  await openedAt(founderPage, founder, address);
  const refused = await leaveInSettings(founderPage, name);
  await expect(refused.dialog.getByRole('alert')).toContainText(
    'Make another member an administrator',
  );
  await refused.dialog.getByRole('button', { name: 'Cancel' }).click();
  expect((await api.get('/api/v1/users/me/workspaces')).ok()).toBe(true);

  const left = await leaveInSettings(page, name);
  await expect(left.dialog).toBeHidden();
  await expect(left.section.getByRole('status')).toHaveText(`You left ${name}.`);
  await expect(left.section.getByRole('button', { name: `Leave ${name}` })).toHaveCount(0);
  await page.keyboard.press('Escape');
  // The page went from the organization's board to the leaver's own space.
  await expect(sidebar(page).getByTestId('workspace-switcher')).toHaveAccessibleName(
    'Switch workspace: My space',
  );
  const theirApi = await signedIn(base, leaver);
  const listed = (await (
    await theirApi.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  expect(listed.map((item) => item.kind)).toEqual(['personal']);
  await page.goto(address);
  await expect(page.getByTestId('not-found')).toBeVisible();

  // The other tab was told, and hears the workspace no more: a card made now
  // reaches the founder's page, not it.
  await expect
    .poll(() => heard.some((frame) => frame.includes('workspaceUnsubscribed')))
    .toBe(true);
  await card('Made after leaving');
  await expect(founderPage.getByText('Made after leaving', { exact: true })).toBeVisible();
  expect(heard.filter((frame) => frame.includes('Made after leaving'))).toEqual([]);
  expect(errors).toEqual([]);
  await Promise.all([api.dispose(), theirApi.dispose(), theirs.close()]);
});

test('an organization administrator removes a member and a zero-workspace member still sees and leaves the renamed organization', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  await founded(base, person('before-remove'), 'Earlier org');
  const founder = person('remove-founder');
  await founded(base, founder, 'Remove studio');
  const api = await signedIn(base, founder);
  const [workspace] = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const workspaceId = workspace?.id ?? '';
  const orgId = workspace?.orgId ?? '';
  // Rename the organization distinctly from its workspace name.
  expect((await api.patch(`/api/v1/org/${orgId}`, { data: { name: 'Renamed Corp' } })).ok()).toBe(
    true,
  );

  const removed = await joinedBy(base, api, workspaceId);
  const stranded = await joinedBy(base, api, workspaceId);
  const strandedApi = await signedIn(base, stranded);
  const strandedMe = (await (await strandedApi.get('/api/v1/users/me')).json()) as { id: string };

  // Remove `stranded` from the workspace only: zero workspaces left, still in the org.
  expect((await api.delete(`/api/v1/w/${workspaceId}/members/${strandedMe.id}`)).status()).toBe(
    204,
  );

  // `stranded` sees the real organization name in Account settings and can leave it.
  const strandedCtx = await browser.newContext({ locale: 'en-US', baseURL: base });
  const strandedPage = await strandedCtx.newPage();
  const strandedErrors = watchErrors(strandedPage);
  await openedAt(strandedPage, stranded, '/');
  const left = await leaveInSettings(strandedPage, 'Renamed Corp');
  await expect(left.dialog).toBeHidden();
  await expect(left.section.getByRole('status')).toHaveText('You left Renamed Corp.');
  expect(strandedErrors).toEqual([]);
  await strandedApi.dispose();
  await strandedCtx.close();

  // `removed` opens a live board tab, then founder removes them from the organization screen.
  const board = await makeBoard(api, {
    title: 'Active work',
    workspaceId,
    members: [{ who: removed, role: 'editor' }],
  });
  const address = addressOf({ workspaceId, boardId: board.boardId, viewId: board.viewId });
  const memberCtx = await browser.newContext({ locale: 'en-US', baseURL: base });
  const memberPage = await memberCtx.newPage();
  const memberFrames: string[] = [];
  memberPage.on('websocket', (socket) =>
    socket.on('framereceived', ({ payload }) => memberFrames.push(String(payload))),
  );
  await openedAt(memberPage, removed, address);
  await expect(memberPage.getByTestId('board-page')).toBeVisible();

  const errors = watchErrors(page);
  await openedAt(page, founder, '/');
  await page.goto(`/org/${orgId}`);
  const orgScreen = page.getByTestId('org-page');
  await expect(orgScreen).toBeVisible();
  const ownRow = orgScreen.locator(`[data-username="${founder.username}"]`);
  await expect(ownRow.getByRole('button', { name: /Remove/ })).toHaveCount(0);

  const memberRow = orgScreen.locator(`[data-username="${removed.username}"]`);
  await memberRow.getByRole('button', { name: /Remove/ }).click();
  const confirm = page.getByRole('alertdialog');
  await expect(confirm).toContainText(`@${removed.username}`);
  await confirm.getByRole('button', { name: 'Remove', exact: true }).click();
  await expect(memberRow).toHaveCount(0);
  await expect(
    orgScreen.getByTestId('org-records').locator('[data-action="org_member_removed"]').first(),
  ).toBeVisible();

  await expect
    .poll(() => memberFrames.some((frame) => frame.includes('workspaceUnsubscribed')))
    .toBe(true);
  await memberPage.goto(address);
  await expect(memberPage.getByTestId('not-found')).toBeVisible();
  expect(errors).toEqual([]);
  await Promise.all([api.dispose(), memberCtx.close()]);
});
