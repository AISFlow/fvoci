/**
 * Copying a board into another workspace, on the in-memory backend: the
 * reader's own space is the one place to go, so it is chosen already, and one
 * press makes a private copy under the same title, which opens. An
 * organization that lets nothing be copied out of it (`?copyOut=0`) refuses;
 * the dialog says so in one line, announced, and the place outside it can no
 * longer be chosen. Opened again, the dialog tries again.
 */
import { expect, test } from '@playwright/test';

import { ON_A_VIEW, placeOf, WORKSPACE } from './address.ts';
import { boardMenu, chooseFromMenu, FIRST_BOARD, openApp, showSidebar } from './shell.support.ts';

test('a board is copied into the one place there is with one press, under its title', async ({
  page,
}) => {
  await openApp(page);
  await showSidebar(page);
  await chooseFromMenu(page, boardMenu(page, FIRST_BOARD), 'copy-to-workspace');
  const dialog = page.getByTestId('copy-to-workspace-dialog');
  await expect(dialog.getByTestId('copy-to-workspace-only')).toHaveText('Copies into My space');
  await expect(dialog.getByRole('textbox')).toHaveCount(0);
  await expect(dialog.getByTestId('copy-to-workspace-target')).toHaveCount(0);
  await dialog.getByTestId('copy-to-workspace-submit').click();
  await expect(dialog).toBeHidden();
  await expect(page).toHaveURL(ON_A_VIEW);
  expect(placeOf(page.url()).workspaceId).not.toBe(WORKSPACE);
  await expect(page.getByTestId('top-bar').getByTestId('board-title')).toHaveValue(FIRST_BOARD);
});

test('an organization that keeps its boards in says so once it has refused, until the dialog is opened again', async ({
  page,
}) => {
  await openApp(page, '/?copyOut=0');
  await showSidebar(page);
  await chooseFromMenu(page, boardMenu(page, FIRST_BOARD), 'copy-to-workspace');
  const dialog = page.getByTestId('copy-to-workspace-dialog');
  await expect(dialog.getByTestId('copy-to-workspace-kept')).toHaveCount(0);
  await dialog.getByTestId('copy-to-workspace-submit').click();
  const kept = dialog.getByTestId('copy-to-workspace-kept');
  await expect(kept).toHaveText(
    'This organization does not let its containers be copied outside it: they copy into its own workspaces only.',
  );
  await expect(kept).toHaveAttribute('role', 'alert');
  await expect(dialog.getByRole('alert')).toHaveCount(1);
  await expect(dialog.getByTestId('copy-to-workspace-error')).toHaveCount(0);
  await expect(dialog.getByTestId('copy-to-workspace-submit')).toBeDisabled();
  const target = dialog.getByTestId('copy-to-workspace-target');
  await expect(target).toHaveAttribute('aria-describedby', (await kept.getAttribute('id')) ?? '');
  await expect(target.getByRole('option')).toHaveText([
    'My space — outside the organization, cannot copy',
  ]);
  await expect(target.getByRole('option')).toBeDisabled();

  // The switch may be back on: opened again, the dialog lets the copy be tried again.
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toBeHidden();
  await chooseFromMenu(page, boardMenu(page, FIRST_BOARD), 'copy-to-workspace');
  await expect(dialog.getByTestId('copy-to-workspace-kept')).toHaveCount(0);
  await expect(dialog.getByTestId('copy-to-workspace-only')).toHaveText('Copies into My space');
  await dialog.getByTestId('copy-to-workspace-submit').click();
  await expect(dialog.getByTestId('copy-to-workspace-kept')).toHaveAttribute('role', 'alert');
});
