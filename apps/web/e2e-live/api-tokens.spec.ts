/**
 * Personal API tokens on the real server: made in the settings with what it
 * may do and a name of its own, shown once as the command that connects
 * Claude Code, which one press copies; the agent then reads and writes as its
 * owner, and what it writes says whose agent wrote it, in the card's versions
 * and on its comment. Its owner ends it in the settings, and an administrator
 * of its organization takes another one out of it on the organization's
 * screen, each after being asked: from then on each opens nothing.
 */
import { expect, request, test, type APIRequestContext, type Page } from '@playwright/test';

import { clipboardPermissions } from '../e2e/keyboard.support.ts';

import {
  ADMIN,
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';
import { openVersions, rows } from './versions.support.ts';

test.use({ locale: 'en-US' });

/** Makes a token in the settings, `write` or read only, and gives its name and the token itself. */
async function makeToken(
  page: Page,
  base: string,
  scope: 'Read only' | 'Read and write content',
): Promise<{ name: string; token: string }> {
  const engine = page.context().browser()?.browserType().name() ?? 'chromium';
  await page.context().grantPermissions(clipboardPermissions(engine));
  await page.getByTestId('open-settings').click();
  const section = page.getByTestId('settings-dialog').getByTestId('api-tokens');
  await section.getByTestId('api-token-new').click();
  const form = section.getByTestId('api-token-form');
  const name = form.getByLabel('Name');
  await expect(name).toHaveValue(/^Agent · \d{1,2}\/\d{1,2}$/);
  await expect(form).toContainText('Lasts 90 days.');
  await form.getByLabel(scope).check();
  const named = `${await name.inputValue()} ${String(Date.now() % 100_000)}`;
  await name.fill(named);
  await form.getByTestId('api-token-make').click();

  const issued = section.getByTestId('api-token-issued');
  await expect(issued).toBeFocused();
  await expect(section.getByRole('status')).toHaveText(
    'The token is made. Copy it now: it is not shown again.',
  );
  const command = (await issued.getByTestId('api-token-command').textContent()) ?? '';
  expect(command).toMatch(
    new RegExp(
      `^claude mcp add --transport http fvoci ${base}/api/v1/mcp --header "Authorization: Bearer fvoci_pat_[A-Za-z0-9_-]+"$`,
    ),
  );
  await expect(issued).toContainText('The command stays in your shell history.');
  const raw = (await issued.getByTestId('api-token-raw').textContent()) ?? '';
  expect(command).toContain(`Bearer ${raw}"`);
  await issued.getByTestId('api-token-copy').click();
  await expect(issued.getByTestId('api-token-copy')).toHaveText('Copied');
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(command);
  const token = /Bearer (fvoci_pat_[^"]+)"/.exec(command)?.[1] ?? '';
  await issued.getByRole('button', { name: 'Done' }).click();
  await expect(section.getByTestId('api-token-issued')).toHaveCount(0);
  await expect(section.getByTestId('api-token').filter({ hasText: named })).toContainText(scope);
  await page.keyboard.press('Escape');
  return { name: named, token };
}

async function agentOf(base: string, token: string): Promise<APIRequestContext> {
  return request.newContext({
    baseURL: base,
    extraHTTPHeaders: { Authorization: `Bearer ${token}` },
  });
}

test('a token made in the settings works as its owner, says whose agent wrote, and ends at once', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('agentowner');
  await register(base, who);
  const api = await signedIn(base, who);
  const board = await makeBoard(api, { title: 'Agent plans', cards: [{ title: 'Plan' }] });
  const cardId = board.cards.Plan ?? '';
  const errors = watchErrors(page);

  await page.goto('/');
  await signInOnPage(page, who);
  const { name, token } = await makeToken(page, base, 'Read and write content');

  // The agent reads and writes as its owner.
  const agent = await agentOf(base, token);
  const containers = await agent.get(`/api/v1/w/${board.workspaceId}/containers`);
  expect(containers.status()).toBe(200);
  expect(((await containers.json()) as Schemas['Container'][]).map((one) => one.id)).toContain(
    board.boardId,
  );
  const appended = await agent.post(
    `/api/v1/w/${board.workspaceId}/cards/${cardId}/body/markdown`,
    { data: { markdown: 'Written by the agent' } },
  );
  expect(appended.ok(), await appended.text()).toBe(true);
  const commented = await agent.post(
    `/api/v1/w/${board.workspaceId}/boards/${board.boardId}/cards/${cardId}/comments`,
    {
      data: {
        body: {
          type: 'doc',
          content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Done by me' }] }],
        },
      },
    },
  );
  expect(commented.ok(), await commented.text()).toBe(true);

  // What it wrote says whose agent wrote it.
  await page.goto(boardPath(board, board.viewId, cardId));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  const comment = page.getByTestId('card-comments').locator('[data-comment]');
  await expect(comment).toContainText('Done by me');
  await expect(comment.getByTestId('comment-agent')).toHaveText(`Agent · ${name}`);
  await openVersions(page);
  await expect(rows(page).first().getByTestId('body-version-cause')).toHaveText(`Agent · ${name}`);
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');

  // Ended in the settings, it opens nothing.
  await page.getByTestId('open-settings').click();
  const section = page.getByTestId('settings-dialog').getByTestId('api-tokens');
  await expect(section.getByTestId('api-token').filter({ hasText: name })).toContainText(
    'Last used',
  );
  await section.getByRole('button', { name: `End: ${name}` }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'End', exact: true }).click();
  await expect(section.getByRole('status')).toHaveText('The token is ended.');
  await expect(section.getByTestId('api-token').filter({ hasText: name })).toHaveCount(0);
  expect((await agent.get(`/api/v1/w/${board.workspaceId}/containers`)).status()).toBe(401);

  await agent.dispose();
  await api.dispose();
  expect(errors).toEqual([]);
});

test("an administrator sees a member's token on the organization's screen and takes it out", async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const who = person('agentmember');
  await register(base, who);
  const memberApi = await signedIn(base, who);
  const [workspace] = (await (
    await memberApi.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const orgId = workspace?.orgId ?? '';

  const member = await browser.newPage();
  await member.goto('/');
  await signInOnPage(member, who);
  const { name, token } = await makeToken(member, base, 'Read only');
  await member.close();
  const agent = await agentOf(base, token);
  expect((await agent.get(`/api/v1/w/${workspace?.id ?? ''}/containers`)).status()).toBe(200);

  const errors = watchErrors(page);
  await page.goto('/');
  await signInOnPage(page, ADMIN);
  await page.goto(`/org/${orgId}`);
  const tokens = page.getByTestId('org-api-tokens');
  const row = tokens.getByTestId('org-api-token').filter({ hasText: name });
  await expect(row).toContainText(`@${who.username}`);
  await expect(row).toContainText('Read only');
  await row.getByRole('button', { name: `Take out: ${name}` }).click();
  await page
    .getByRole('alertdialog')
    .getByRole('button', { name: 'Take out', exact: true })
    .click();
  await expect(row).toHaveCount(0);
  // It reached this organization alone: taken out of it, it is ended.
  expect((await agent.get(`/api/v1/w/${workspace?.id ?? ''}/containers`)).status()).toBe(401);

  await agent.dispose();
  await memberApi.dispose();
  expect(errors).toEqual([]);
});
