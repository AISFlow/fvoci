/**
 * Names on the real server: a display name given at registration or not;
 * in the settings the name of one's own, the parts of the name, which of
 * them is shown, a title and credentials, a refused part told beside its
 * field; and every place that shows a person showing "Shown name (@login)",
 * or the login name alone without one.
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import {
  boardPath,
  invitation,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

async function me(api: APIRequestContext): Promise<Schemas['User']> {
  return (await (await api.get('/api/v1/users/me')).json()) as Schemas['User'];
}

test('a display name is given where the account is made, changed in the settings, and a refused one is told at its field', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const errors = watchErrors(page);
  const who = person('named');
  const token = await invitation(base);

  await page.goto(`/invite/${token}`);
  const form = page.getByTestId('invite-register-form');
  await form.getByLabel(/^user name$/i).fill(who.username);
  // A name that turns the text around: the server's refusal is said beside the field.
  const displayName = form.getByLabel(/^display name$/i);
  await displayName.fill(`Ada${String.fromCharCode(0x202e)}`);
  await form.getByLabel(/email/i).fill(who.email);
  await form.getByLabel(/^password$/i).fill(who.password);
  await form.getByRole('button', { name: /^create account and join$/i }).click();
  await expect(form.getByTestId('problem-displayName')).toHaveText(/turn the text around/);
  await expect(displayName).toBeFocused();
  await displayName.fill('  Ada Lovelace ');
  await form.getByRole('button', { name: /^create account and join$/i }).click();
  await expect(page.getByTestId('app-shell')).toBeVisible();

  const api = await signedIn(base, who);
  expect((await me(api)).displayName).toBe('Ada Lovelace');

  await page.getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');
  await expect(settings.getByTestId('signed-in-as')).toHaveText(
    `Signed in as Ada Lovelace (@${who.username})`,
  );
  const name = settings.locator('[data-setting="name"]');
  const field = name.getByRole('textbox', { name: 'Name of your own', exact: true });
  await expect(field).toHaveValue('Ada Lovelace');
  const save = settings.getByRole('button', { name: 'Save name' });

  // A character that shows nothing: refused by the server, said beside the field.
  await field.fill(`Ada${String.fromCharCode(0x200b)}L`);
  await save.click();
  await expect(name.getByTestId('problem-customName')).toHaveText(/show nothing/);
  await expect(field).toHaveAttribute('aria-invalid', 'true');
  expect((await me(api)).displayName).toBe('Ada Lovelace');
  // Too long, counted as the server counts (UTF-16 units).
  await field.fill('x'.repeat(65));
  await save.click();
  await expect(name.getByTestId('problem-customName')).toHaveText(/at most 64/i);

  await field.fill('Countess');
  await save.click();
  await expect(name.getByTestId('problem-customName')).toHaveCount(0);
  await expect(settings.getByText('Saved.')).toBeVisible();
  await expect.poll(async () => (await me(api)).displayName).toBe('Countess');
  await expect(settings.getByTestId('signed-in-as')).toHaveText(
    `Signed in as Countess (@${who.username})`,
  );

  // Empty is none: the login name alone.
  await field.fill('');
  await save.click();
  await expect.poll(async () => (await me(api)).displayName).toBe('');
  await expect(settings.getByTestId('signed-in-as')).toHaveText(`Signed in as ${who.username}`);

  // Kept by the account: another page reads it from the server.
  await api.patch('/api/v1/users/me', { data: { customName: 'Lady Ada' } });
  await page.reload();
  await page.getByTestId('open-settings').click();
  await expect(field).toHaveValue('Lady Ada');

  // The parts of the name, the one shown, a title and a credential: kept by the server,
  // which makes the name shown of them.
  await name.getByRole('textbox', { name: 'Family name', exact: true }).fill('Lovelace');
  await name.getByRole('textbox', { name: 'Given name', exact: true }).fill('Augusta Ada');
  await name.getByTestId('name-shown-name').check();
  await name.getByRole('combobox', { name: 'Title' }).selectOption({ label: 'Prof.' });
  await name
    .getByRole('combobox', { name: 'Add a degree or qualification' })
    .selectOption({ label: 'PhD' });
  await name.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(name.getByTestId('name-preview')).toHaveText(
    'Others see you as Augusta Ada Lovelace.',
  );
  await name.getByRole('button', { name: 'Save name' }).click();
  await expect(name.getByText('Saved.')).toBeVisible();
  const named = await me(api);
  expect([named.displayName, named.lastName, named.title, named.credentials]).toEqual([
    'Augusta Ada Lovelace',
    'Lovelace',
    'prof',
    ['phd'],
  ]);
  await expect(settings.getByTestId('signed-in-as')).toHaveText(
    `Signed in as Augusta Ada Lovelace (@${who.username})`,
  );
  // A title off the list: refused, and nothing changes.
  const refused = await api.patch('/api/v1/users/me', { data: { title: 'sir' } });
  expect(refused.status()).toBe(400);
  expect((await me(api)).title).toBe('prof');
  await api.dispose();
  expect(errors).toEqual([]);
});

test('people are shown by display name and login name wherever the board names them', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('host');
  const named = person('grace');
  const plain = person('plain');
  for (const who of [owner, named, plain]) await register(base, who);
  const ownerApi = await signedIn(base, owner);
  const namedApi = await signedIn(base, named);
  const plainApi = await signedIn(base, plain);
  await namedApi.patch('/api/v1/users/me', { data: { customName: 'Grace Hopper' } });
  const namedId = (await me(namedApi)).id;
  const graceLabel = `Grace Hopper (@${named.username})`;

  const board = await makeBoard(ownerApi, {
    title: 'Named people',
    cards: [{ title: 'Plan the launch', status: 'To do' }],
    members: [{ who: named, role: 'editor' }],
  });
  const cardId = board.cards['Plan the launch'] ?? '';
  // A person property with Grace in it, a body that mentions her, and her comment.
  const patched = await ownerApi.patch(
    `/api/v1/w/${board.workspaceId}/containers/${board.boardId}`,
    {
      data: {
        updatedCardProperties: [{ id: 'owner', name: 'Owner', type: 'person', options: [] }],
      },
    },
  );
  expect(patched.ok()).toBe(true);
  const card = (await (
    await ownerApi.get(`/api/v1/w/${board.workspaceId}/cards/${cardId}`)
  ).json()) as Schemas['Card'];
  const written = await ownerApi.patch(`/api/v1/w/${board.workspaceId}/cards/${cardId}`, {
    data: {
      readAt: card.updatedAt,
      updatedProperties: { owner: namedId },
      body: {
        type: 'doc',
        content: [
          {
            type: 'paragraph',
            content: [
              { type: 'text', text: 'ask ' },
              { type: 'mention', attrs: { userId: namedId } },
            ],
          },
        ],
      },
    },
  });
  expect(written.ok()).toBe(true);
  const commented = await namedApi.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments`,
    {
      data: {
        body: {
          type: 'doc',
          content: [{ type: 'paragraph', content: [{ type: 'text', text: 'On it' }] }],
        },
      },
    },
  );
  expect(commented.ok()).toBe(true);

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId, cardId));
  await signInOnPage(page, owner);
  const dialog = page.getByTestId('card-dialog');
  await expect(dialog).toBeVisible();
  // The mention, the person property and the comment's author.
  await expect(page.getByTestId('card-body').locator('[data-mention]')).toHaveText([
    `@${graceLabel}`,
  ]);
  await expect(dialog.locator('[data-slot="person-chip"]')).toHaveText([graceLabel]);
  await expect(page.getByTestId('card-comments')).toContainText(graceLabel);
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);

  // The share dialog: members, and people found by either name.
  await page.getByTestId('share-board').click();
  const members = page.getByTestId('share-members');
  await expect(members.locator(`[data-member="${graceLabel}"]`)).toHaveCount(1);
  await expect(members.locator(`[data-member="${owner.username}"]`)).toHaveCount(1);
  const search = page.getByTestId('share-search');
  await plainApi.patch('/api/v1/users/me', { data: { customName: 'Plainly Named' } });
  await search.fill('plainly');
  await expect(page.getByTestId('share-candidates')).toContainText(
    `Plainly Named (@${plain.username})`,
  );
  for (const api of [ownerApi, namedApi, plainApi]) await api.dispose();
  expect(errors).toEqual([]);
});

test("a member's new name and title reach another's open page, and their profile card", async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('chair');
  const named = person('kim');
  for (const who of [owner, named]) await register(base, who);
  const namedApi = await signedIn(base, named);
  const ownerApi = await signedIn(base, owner);
  const board = await makeBoard(ownerApi, {
    title: 'Faculty',
    cards: [{ title: 'Seminar', status: 'To do' }],
    members: [{ who: named, role: 'editor' }],
  });

  const errors = watchErrors(page);
  await page.goto(boardPath(board, board.viewId));
  await signInOnPage(page, owner);
  await page.getByTestId('share-board').click();
  const members = page.getByTestId('share-members');
  await expect(members.getByRole('button', { name: named.username, exact: true })).toBeVisible();

  // Named in another session: the open page is told, and reads them again.
  const changed = await namedApi.patch('/api/v1/users/me', {
    data: {
      lastName: '김',
      firstName: '영환',
      latinFirstName: 'Younghwan',
      latinLastName: 'Kim',
      title: 'prof',
      credentials: ['phd'],
    },
  });
  expect(changed.ok()).toBe(true);
  const opener = members.getByRole('button', { name: `Prof. 김영환, PhD @${named.username}` });
  await expect(opener).toBeVisible();
  await opener.click();
  const card = page.getByRole('dialog', { name: 'Prof. 김영환, PhD' });
  await expect(card.locator('[data-row="latin"]')).toHaveText('Younghwan Kim');
  await expect(card.locator('[data-row="credentials"]')).toHaveText('PhD');
  await page.keyboard.press('Escape');
  await expect(card).toHaveCount(0);
  await expect(opener).toBeFocused();
  // The narrow places name them alone: the member's tag of the list says "김영환 (@login)".
  await expect(members.locator(`[data-member="김영환 (@${named.username})"]`)).toHaveCount(1);
  for (const api of [ownerApi, namedApi]) await api.dispose();
  expect(errors).toEqual([]);
});
