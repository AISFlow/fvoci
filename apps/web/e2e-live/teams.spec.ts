/** Workspace administrators manage groups; their colleagues can read them. */
import path from 'node:path';
import { expect, test } from '@playwright/test';
import { screensDir } from '../e2e/screens.ts';
import { boardLink, unfold } from '../e2e/shell.support.ts';
import {
  ADMIN,
  boardPath,
  makeBoard,
  person,
  register,
  signedIn,
  signInOnPage,
  uniqueName,
  watchErrors,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

test('an administrator creates a team, changes its name and people, and deletes it', async ({
  page,
  baseURL,
  browser,
}) => {
  const base = baseURL ?? '';
  const colleague = person('group-member');
  await register(base, colleague);
  const theirs = await signedIn(base, colleague);
  const member = (await (await theirs.get('/api/v1/users/me')).json()) as Schemas['User'];
  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, ADMIN);
  await page.getByTestId('open-people').click();
  const panel = page.getByTestId('teams-panel');
  await unfold(panel.getByTestId('teams-toggle'));
  const name = uniqueName('Design');
  await panel.getByRole('textbox', { name: 'Group name' }).fill(name);
  await panel.getByRole('button', { name: 'Create group' }).click();
  let row = panel.locator(`[data-team="${name}"]`);
  await row.locator('summary').click();
  await row.getByLabel('Choose a person').selectOption(member.id);
  await row.getByRole('button', { name: 'Add a person' }).click();
  await expect(row.locator(`[data-team-member="${member.id}"]`)).toBeVisible();
  if (process.env.FVOCI_SCREENS === '1') {
    const folder = screensDir('teams');
    await page.screenshot({ path: path.join(folder, 'people-wide.png') });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({ path: path.join(folder, 'people-390.png') });
    await page.setViewportSize({ width: 1280, height: 720 });
  }
  // Ordinary members see the group and its people, without management controls.
  const other = await browser.newContext();
  const otherPage = await other.newPage();
  await otherPage.goto(base);
  await signInOnPage(otherPage, colleague);
  await otherPage.getByTestId('open-people').click();
  const readOnly = otherPage.getByTestId('teams-panel');
  await readOnly.locator(`[data-team="${name}"] summary`).click();
  await expect(readOnly.getByText(colleague.username, { exact: true })).toBeVisible();
  await expect(readOnly.getByRole('button')).toHaveCount(0);
  await other.close();
  // A refused name stays typed, with the reason in one line.
  const box = panel.getByRole('textbox', { name: 'Group name' });
  for (const [typed, told] of [
    [name.toUpperCase(), /Letter case does not make names different/],
    ['x'.repeat(65), /up to 64 characters/],
    ['a\u202eb', /change the direction of text/],
  ] as const) {
    await box.fill(typed);
    await panel.getByRole('button', { name: 'Create group' }).click();
    await expect(panel.getByRole('alert')).toHaveText(told);
    await expect(box).toHaveValue(typed);
  }
  await box.fill('');
  await row.getByRole('button', { name: 'Rename', exact: true }).click();
  const renamed = `${name}-research`;
  await row.getByRole('textbox', { name: `New name for ${name}` }).fill(renamed);
  await row.getByRole('button', { name: 'Save name' }).click();
  row = panel.locator(`[data-team="${renamed}"]`);
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: `Remove ${colleague.username} from group` }).click();
  await expect(row.locator(`[data-team-member="${member.id}"]`)).toHaveCount(0);
  await row.getByRole('button', { name: 'Delete group' }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Delete group' }).click();
  await expect(row).toHaveCount(0);
  expect(errors).toEqual([]);
  await theirs.dispose();
});

test('a team editor opens and edits a board, then loses it when removed from the team', async ({
  page,
  baseURL,
  browser,
}) => {
  const base = baseURL ?? '';
  const colleague = person('team-editor');
  await register(base, colleague);
  const theirs = await signedIn(base, colleague);
  const api = await signedIn(base, ADMIN);
  const member = (await (await theirs.get('/api/v1/users/me')).json()) as Schemas['User'];
  const title = uniqueName('Team board');
  const board = await makeBoard(api, {
    title,
    cards: [{ title: 'Plan together', status: 'To do' }],
  });
  await page.goto('/');
  await signInOnPage(page, ADMIN);
  await page.getByTestId('open-people').click();
  const panel = page.getByTestId('teams-panel');
  await unfold(panel.getByTestId('teams-toggle'));
  const name = uniqueName('Editors');
  await panel.getByRole('textbox', { name: 'Group name' }).fill(name);
  await panel.getByRole('button', { name: 'Create group' }).click();
  const row = panel.locator(`[data-team="${name}"]`);
  await row.locator('summary').click();
  await row.getByLabel('Choose a person').selectOption(member.id);
  await row.getByRole('button', { name: 'Add a person' }).click();
  await expect(row.locator(`[data-team-member="${member.id}"]`)).toBeVisible();
  await page.keyboard.press('Escape');
  const other = await browser.newContext({ locale: 'en-US', baseURL: base });
  const editor = await other.newPage();
  await editor.goto('/');
  await signInOnPage(editor, colleague);
  await expect(boardLink(editor, title)).toHaveCount(0);
  await page.goto(boardPath(board, board.viewId));
  await page.getByTestId('share-board').click();
  const groups = page.getByTestId('board-teams');
  await unfold(groups.getByTestId('board-teams-toggle'));
  await groups.getByLabel('Choose a group').selectOption({ label: name });
  await groups.getByRole('button', { name: 'Add a group' }).click();
  const assigned = groups.locator(`[data-board-team="${name}"]`);
  await expect(assigned).toBeVisible();
  await assigned.locator('summary').click();
  await expect(assigned.getByText(colleague.username, { exact: true })).toBeVisible();
  await expect(assigned.getByLabel(`Role of ${name}`)).toHaveValue('editor');
  await expect(page.getByTestId('share-members').locator('li')).toHaveCount(1);
  if (process.env.FVOCI_SCREENS === '1') {
    const folder = screensDir('teams');
    await groups.scrollIntoViewIfNeeded();
    await page.screenshot({ path: path.join(folder, 'share-wide.png'), animations: 'disabled' });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect
      .poll(async () => (await page.getByTestId('share-dialog').boundingBox())?.width ?? 1000)
      .toBeLessThanOrEqual(390);
    await groups.scrollIntoViewIfNeeded();
    await page.screenshot({ path: path.join(folder, 'share-390.png'), animations: 'disabled' });
    await page.setViewportSize({ width: 1280, height: 720 });
  }
  await expect(boardLink(editor, title)).toBeVisible();
  await boardLink(editor, title).click();
  await expect(editor.getByTestId('board-title')).toHaveValue(title);
  await editor.locator('[data-card]').getByText('Plan together', { exact: true }).click();
  await editor.getByTestId('card-title').fill('Edited through the team');
  await expect
    .poll(
      async () =>
        (
          (await (
            await api.get(`/api/v1/w/${board.workspaceId}/cards/${board.cards['Plan together']}`)
          ).json()) as Schemas['Card']
        ).title,
    )
    .toBe('Edited through the team');
  await page.keyboard.press('Escape');
  await page.getByTestId('open-people').click();
  await unfold(panel.getByTestId('teams-toggle'));
  await row.locator('summary').click();
  await row.getByRole('button', { name: `Remove ${colleague.username} from group` }).click();
  await expect(row.locator(`[data-team-member="${member.id}"]`)).toHaveCount(0);
  await expect(boardLink(editor, title)).toHaveCount(0);
  await expect
    .poll(async () =>
      (await theirs.get(`/api/v1/w/${board.workspaceId}/containers/${board.boardId}`)).status(),
    )
    .toBe(404);
  await other.close();
  await theirs.dispose();
  await api.dispose();
});
