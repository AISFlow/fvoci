/**
 * The people of a workspace on the real server, as its administrator manages them
 * from the sidebar: the link that lets members in (made, shown once,
 * revoked), what a member is, and taking one out.
 */
import { expect, test } from '@playwright/test';

import {
  ADMIN,
  apiClient,
  person,
  register,
  shot,
  signedIn,
  signInOnPage,
  watchErrors,
  type Schemas,
} from './support.ts';

test.use({ locale: 'en-US' });

test('an administrator makes and revokes the link for members, changes what one is, and takes one out', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const colleague = person('colleague');
  await register(base, colleague);
  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, ADMIN);

  await page.getByTestId('open-people').click();
  const dialog = page.getByTestId('people-dialog');
  const row = dialog.locator(`[data-member="${colleague.username}"]`);
  await expect(row).toBeVisible();

  // A link: its address is shown this once, and works for somebody without an account.
  await dialog.getByTestId('people-make-link').click();
  const field = dialog.getByTestId('people-link');
  await expect(field).toHaveValue(/\/invite\/[\w-]{43}$/);
  const link = await field.inputValue();
  await shot(page, 'people');
  const token = link.slice(link.lastIndexOf('/') + 1);
  const anonymous = await apiClient(base);
  const look = () => anonymous.post('/api/v1/invitations/preview', { data: { token } });
  expect((await look()).status()).toBe(200);
  // Revoked, it lets nobody in. The newest link is the first of the list.
  await dialog
    .getByTestId('people-links')
    .getByRole('button', { name: 'Revoke link' })
    .first()
    .click();
  await expect.poll(async () => (await look()).status()).toBe(404);
  await expect(field).toHaveCount(0);

  // What a member is: an admin, and a member again.
  const role = row.getByLabel(`Role of ${colleague.username}`);
  await role.selectOption('admin');
  const theirs = await signedIn(base, colleague);
  const roleOfColleague = async (): Promise<string | undefined> => {
    const workspaces = (await (
      await theirs.get('/api/v1/users/me/workspaces')
    ).json()) as Schemas['UserWorkspace'][];
    // In the workspace they were let into: their own space, which they keep whatever
    // becomes of them here, is listed after it.
    return workspaces.find((workspace) => workspace.kind !== 'personal')?.role;
  };
  await expect.poll(roleOfColleague).toBe('admin');
  await expect(role).toHaveValue('admin');
  await role.selectOption('member');
  await expect.poll(roleOfColleague).toBe('member');

  // The administrator who is signed in is not taken out by themselves.
  const own = dialog.locator(`[data-member="${ADMIN.username}"]`);
  await expect(own.getByRole('button', { name: `Remove ${ADMIN.username}` })).toBeDisabled();
  await expect(own.getByLabel(`Role of ${ADMIN.username}`)).toBeDisabled();

  // Taken out, the colleague has no workspace to open.
  await row.getByRole('button', { name: `Remove ${colleague.username}` }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Remove' }).click();
  await expect(row).toHaveCount(0);
  await expect.poll(roleOfColleague).toBeUndefined();
  await expect(dialog.getByTestId('people-problem')).toHaveCount(0);

  await Promise.all([anonymous.dispose(), theirs.dispose()]);
  expect(errors).toEqual([]);
});
