/** Full-app fixtures and observations for the isolated, open PostgreSQL server. */
import { readFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';

import {
  expect,
  type APIRequestContext,
  type Browser,
  type Page,
  type TestInfo,
} from '@playwright/test';

import { boardLink, boardMenu, category, chooseFromMenu, sidebar } from '../e2e/shell.support.ts';

import { apiClient, person, placeOf, watchErrors, type Person, type Schemas } from './support.ts';

export async function firstOrganization(base: string): Promise<string[]> {
  const owner = person('sealed-first');
  const api = await apiClient(base);
  const registered = await api.post('/api/v1/register', {
    data: { ...owner, organizationName: 'Sealed first organization' },
  });
  expect(registered.status()).toBe(201);
  expect((await api.post('/api/v1/login', { data: owner })).ok()).toBe(true);
  const workspaces = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  // The workspace of the organization, and after it the account's own space.
  expect(workspaces.map((workspace) => workspace.kind === 'personal')).toEqual([false, true]);
  const workspace = workspaces[0]?.id ?? '';
  const organization = workspaces[0]?.orgId ?? '';
  expect(organization).not.toBe('');
  const user = (await (await api.get('/api/v1/users/me')).json()) as Schemas['User'];
  const forbidden = [
    owner.username,
    owner.email,
    user.id,
    workspace,
    organization,
    'Sealed first organization',
  ];
  for (const kind of ['board', 'template', 'discarded']) {
    const title = `Sealed first ${kind}`;
    const made = await api.post(`/api/v1/w/${workspace}/boards`, {
      data: { title, visibility: 'open', isTemplate: kind === 'template' },
    });
    expect(made.status()).toBe(201);
    const board = (await made.json()) as Schemas['Container'];
    forbidden.push(title, board.id);
    if (kind === 'discarded')
      expect((await api.delete(`/api/v1/w/${workspace}/containers/${board.id}`)).ok()).toBe(true);
  }
  await api.dispose();
  return forbidden;
}

export async function startOrganization(page: Page, owner: Person): Promise<void> {
  await page.goto('/');
  await page.getByRole('button', { name: 'New here? Create an account' }).click();
  const form = page.getByTestId('register-form');
  // After the server's first account, one starts on its own unless it chooses otherwise.
  await expect(form.getByTestId('register-solo')).toBeChecked();
  await form.getByTestId('register-together').check();
  await form.getByTestId('register-organization-name').fill('Second organization');
  await form.getByLabel(/^user name$/i).fill(owner.username);
  await form.getByLabel(/email/i).fill(owner.email);
  await form.getByLabel(/^password$/i).fill(owner.password);
  await form.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  await expect(boardLink(page, 'Getting started with fvoci')).toHaveAttribute(
    'aria-current',
    'page',
  );
}

export function observe(page: Page) {
  const bodies: Promise<string>[] = [];
  const frames: string[] = [];
  const screens: string[] = [];
  page.on('response', (response) => {
    if (
      response.url().includes('/api/v1/') &&
      response.headers()['content-type']?.includes('json')
    ) {
      bodies.push(
        response
          .text()
          .catch(
            (error: unknown) =>
              `<unreadable response> ${response.status()} ${response.url()}: ${String(error)}`,
          ),
      );
    }
  });
  page.on('websocket', (socket) =>
    socket.on('framereceived', ({ payload }) => frames.push(String(payload))),
  );
  return {
    frames,
    async screen() {
      screens.push(await page.locator('body').innerText());
    },
    async assertIsolated(forbidden: string[]) {
      for (const contents of [...(await Promise.all(bodies)), ...frames, ...screens]) {
        expect(contents).not.toContain('<unreadable response>');
        for (const secret of forbidden) expect(contents).not.toContain(secret);
      }
    },
  };
}

export async function fileUnderCategory(
  page: Page,
  api: APIRequestContext,
  workspace: string,
  title: string,
) {
  await sidebar(page).getByTestId('new-category').click();
  const dialog = page.getByTestId('category-dialog');
  await dialog.getByLabel('Category name').fill('Second projects');
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(dialog).toBeHidden();
  const categories = (await (
    await api.get(`/api/v1/w/${workspace}/categories`)
  ).json()) as Schemas['CategoryContainers'][];
  const id = categories.find((item) => item.name === 'Second projects')?.id;
  expect(id).toBeTruthy();
  await chooseFromMenu(page, boardMenu(page, title), `move-to-${id}`);
  await expect(
    category(page, 'Second projects').getByRole('link', { name: title, exact: true }),
  ).toBeVisible();
  // The search asks every workspace the account is listed, its own space too: the
  // answer looked at is this workspace's.
  const searching = page.waitForResponse((response) =>
    response.url().includes(`/w/${workspace}/boards/search?`),
  );
  await sidebar(page).getByTestId('sidebar-search').fill('Second planning');
  const found = await searching;
  expect(found.ok()).toBe(true);
  expect(await found.text()).toContain(title);
  await expect(boardLink(page, title)).toBeVisible();
  await sidebar(page).getByTestId('sidebar-search').fill('');
}

export async function archiveRoundTrip(
  page: Page,
  api: APIRequestContext,
  workspace: string,
  title: string,
  info: TestInfo,
  forbidden: string[],
) {
  const saving = page.waitForEvent('download');
  await chooseFromMenu(page, boardMenu(page, title), 'export');
  const archive = info.outputPath('second.fvoci');
  await (await saving).saveAs(archive);
  expect((await readFile(archive)).subarray(0, 2).toString()).toBe('PK');
  await sidebar(page).getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');
  const savingAll = page.waitForEvent('download');
  await settings.getByTestId('export-workspace').click();
  const all = info.outputPath('second-workspace.fvoci');
  await (await savingAll).saveAs(all);
  expect((await readFile(all)).subarray(0, 2).toString()).toBe('PK');
  for (const file of [archive, all]) {
    const contents = execFileSync('unzip', ['-p', file], {
      maxBuffer: 20 * 1024 * 1024,
    }).toString();
    const names = execFileSync('unzip', ['-Z1', file]).toString();
    expect(contents).toContain(title);
    for (const secret of forbidden) expect(contents + names).not.toContain(secret);
  }
  await settings.getByTestId('import-archive').setInputFiles(archive);
  await expect(
    page.getByTestId('transfers').locator('[data-transfer="import"][data-finished]'),
  ).toContainText('second.fvoci is imported.');
  const boards = (await (
    await api.get(`/api/v1/w/${workspace}/boards`)
  ).json()) as Schemas['Container'][];
  expect(boards.filter((board) => board.title === title)).toHaveLength(2);
  const copied = boards.find(
    (board) => board.title === title && board.id !== placeOf(page.url()).boardId,
  );
  expect(copied).toBeDefined();
  const cards = (await (
    await api.get(`/api/v1/w/${workspace}/boards/${copied?.id ?? ''}/cards`)
  ).json()) as Schemas['Card'][];
  expect(cards.some((card) => card.title === 'Second launch draft')).toBe(true);
  await page.keyboard.press('Escape');
}

export async function invitedColleague(
  browser: Browser,
  base: string,
  invitation: string,
  address: string,
) {
  const context = await browser.newContext({ locale: 'en-US', baseURL: base });
  const page = await context.newPage();
  const observation = observe(page);
  const errors = watchErrors(page);
  await page.goto(invitation);
  const newcomer = person('second-colleague');
  const form = page.getByTestId('invite-register-form');
  await form.getByLabel(/user name/i).fill(newcomer.username);
  await form.getByLabel(/email/i).fill(newcomer.email);
  await form.getByLabel(/password/i).fill(newcomer.password);
  await form.getByRole('button', { name: 'Create account and join' }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();
  // Somebody else's, open to them: among the open ones to join, at the sidebar's foot.
  const waiting = sidebar(page).getByTestId('open-to-join');
  await waiting.locator('button[aria-expanded="false"]').click();
  await waiting.locator(`a[href*="${placeOf(address).boardId}"]`).click();
  await expect(page.getByText('Second launch draft', { exact: true })).toBeVisible();
  return { page, context, observation, errors };
}

export async function restoreBoard(page: Page, title: string, observeTrash: () => Promise<void>) {
  await chooseFromMenu(page, boardMenu(page, title), 'delete');
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(boardLink(page, title)).toHaveCount(0);
  await page.getByTestId('open-trash').click();
  const discarded = page.locator(`[data-trash-item="${title}"]`);
  await expect(discarded).toBeVisible();
  await observeTrash();
  await discarded.getByRole('button', { name: `Restore ${title}` }).click();
  await expect(discarded).toHaveCount(0);
  await page.keyboard.press('Escape');
  await boardLink(page, title).click();
  await expect(boardLink(page, title)).toHaveAttribute('aria-current', 'page');
}
