/**
 * The first use of a server, from nobody to two people at work on one board:
 * the first person signs up with the name of their organization and makes a board,
 * and a second comes in by the link of an invitation and edits that board.
 *
 * This is the first thing that happens on the server of a run: the project
 * of these tests comes before every other (`playwright.live.config.ts`),
 * which then find the administrator made here.
 *
 * The signup and invitation screens never say workspace: the words
 * of the screens are looked at, in both languages.
 */
import { expect, test, type Page } from '@playwright/test';

import { boardLink, sidebar } from '../e2e/shell.support.ts';

import {
  ADMIN,
  madeABoard,
  ORG_NAME,
  person,
  placeOf,
  shot,
  signedIn,
  watchErrors,
  type Schemas,
} from './support.ts';

test.describe.configure({ mode: 'serial' });
test.use({ locale: 'en-US' });

/** What a screen must not say on the way in, in either language. */
const SERVER_WORDS = /workspace|워크스페이스/i;

/** The words of the screens that were looked at, each under the name of its screen. */
const seen: { screen: string; words: string }[] = [];

/**
 * Keeps the words of the screen. The sidebar's way to the screen of the
 * organization's administrators is left out: it is named after that screen,
 * which speaks of organizations and workspaces to who manages them, and is no
 * step of the way in.
 */
async function look(page: Page, screen: string): Promise<void> {
  const words = await page.locator('body').innerText();
  const door = page.getByTestId('open-organization');
  const named = (await door.count()) > 0 ? await door.innerText() : '';
  seen.push({ screen, words: named === '' ? words : words.replace(named, '') });
}

test('a registration that is refused says what is wrong beside each field, and makes nothing', async ({
  page,
}) => {
  const errors = watchErrors(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'New here? Create an account' }).click();
  const form = page.getByTestId('register-form');
  const organization = form.getByTestId('register-organization-name');
  const name = form.getByLabel(/^user name$/i);
  const email = form.getByLabel(/email/i);
  const password = form.getByLabel(/^password$/i);
  const submit = form.getByRole('button', { name: 'Create account' });

  // Four fields that the browser's own check lets through and the server
  // refuses: blank names, an address that is too long, a short password.
  await organization.fill(' ');
  await name.fill(' ');
  await email.fill(`${'a'.repeat(250)}@example.test`);
  await password.fill('short');
  await submit.click();
  // All are told at once, each beside its field.
  await expect(form.getByTestId('problem-organizationName')).toHaveText('Fill this in.');
  await expect(form.getByTestId('problem-username')).toHaveText('Fill this in.');
  await expect(form.getByTestId('problem-email')).toHaveText('This is too long.');
  await expect(form.getByTestId('problem-password')).toHaveText('Use at least 8 characters.');
  await expect(form.getByRole('alert')).toHaveCount(4);
  await expect(organization).toHaveAttribute('aria-invalid', 'true');
  // The first field that is wrong is where the reader goes on.
  await expect(organization).toBeFocused();
  await look(page, 'a refused registration');

  await organization.fill('x'.repeat(65));
  await name.fill(ADMIN.username);
  await email.fill(ADMIN.email);
  await password.fill(ADMIN.password);
  await submit.click();
  await expect(form.getByTestId('problem-organizationName')).toHaveText(/up to 64 characters\./);
  await expect(form.getByRole('alert')).toHaveCount(1);
  // Nothing was made: the server still takes its first account.
  await page.reload();
  await expect(page.getByRole('button', { name: 'New here? Create an account' })).toBeVisible();
  expect(errors).toEqual([]);
});

test('somebody starts an organization, and a second person comes in by its link and edits the same board', async ({
  page,
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const errors = watchErrors(page);

  // The form to start with, in Korean too.
  const korean = await browser.newContext({ locale: 'ko-KR' });
  const koreanPage = await korean.newPage();
  await koreanPage.goto(base);
  await koreanPage.getByRole('button', { name: '처음인가요? 계정 만들기' }).click();
  await expect(koreanPage.getByLabel('팀 이름')).toBeVisible();
  // The server's first account starts its organization: no choice, and the page says why.
  await expect(koreanPage.getByTestId('register-first')).toBeVisible();
  await expect(koreanPage.getByTestId('register-use')).toHaveCount(0);
  await look(koreanPage, 'the form that starts an organization, in Korean');

  // Signing up: the name of the organization, and an account.
  await page.goto('/');
  await page.getByRole('button', { name: 'New here? Create an account' }).click();
  const form = page.getByTestId('register-form');
  await form.getByTestId('register-organization-name').fill(ORG_NAME);
  await form.getByLabel(/^user name$/i).fill(ADMIN.username);
  await form.getByLabel(/email/i).fill(ADMIN.email);
  await form.getByLabel(/^password$/i).fill(ADMIN.password);
  await look(page, 'the form that starts an organization');
  await shot(page, 'sign-up');
  await form.getByRole('button', { name: 'Create account' }).click();

  // Signed in at once, on a copy of the welcome board of their own, made once.
  await expect(page.getByTestId('app-shell')).toBeVisible();
  const welcome = boardLink(page, 'Getting started with fvoci');
  await expect(welcome).toHaveAttribute('aria-current', 'page');
  await page.reload();
  await expect(welcome).toHaveCount(1);
  // The server has its organization now: the page that signs in offers no other.
  await koreanPage.reload();
  await expect(koreanPage.getByTestId('invitation-needed')).toBeVisible();
  await look(koreanPage, 'the sign-in page of a server with its organization, in Korean');

  // The first board, made in the organization's space: open to it from the start.
  const made = page.waitForResponse(madeABoard);
  await sidebar(page).getByTestId('add-board').click();
  await page.getByTestId('template-picker').getByTestId('empty-board').click();
  await made;
  const title = page.getByTestId('top-bar').getByTestId('board-title');
  await title.fill('Launch plan');
  await title.press('Enter');
  await expect(boardLink(page, 'Launch plan')).toBeVisible();
  await page.getByTestId('share-board').click();
  const share = page.getByTestId('share-dialog');
  await expect(share.getByTestId('share-open')).toBeChecked();
  await look(page, 'the first board, shared with the workspace');
  await page.keyboard.press('Escape');
  // Its first view is grouped by its "Status", without the column of cards that have none.
  await page.getByRole('button', { name: 'New card in To do' }).click();
  await page.getByTestId('card-title').fill('Book the venue');
  await page.goBack();
  await expect(page.getByText('Book the venue')).toBeVisible();
  const { workspaceId, boardId } = placeOf(page.url());

  // The link of an invitation, made where the people of the workspace are.
  await page.getByTestId('open-people').click();
  const people = page.getByTestId('people-dialog');
  await people.getByTestId('people-make-link').click();
  const field = people.getByTestId('people-link');
  await expect(field).toHaveValue(/\/invite\/[\w-]{43}$/);
  const address = await field.inputValue();
  await look(page, 'the people of the workspace, with a link');
  await page.keyboard.press('Escape');
  const api = await signedIn(base, ADMIN);

  // A second person, in a browser of their own, in Korean.
  await koreanPage.goto(address);
  await expect(koreanPage.getByTestId('invite-summary')).toHaveText(
    `${ADMIN.username} 님이 ${ORG_NAME}에 초대했습니다`,
  );
  await look(koreanPage, 'the page of an invitation, in Korean');
  await shot(koreanPage, 'invite-member-ko');
  const second = person('second');
  const account = koreanPage.getByTestId('invite-register-form');
  await account.getByLabel('사용자 이름').fill(second.username);
  await account.getByLabel('이메일 주소').fill(second.email);
  await account.getByLabel('비밀번호').fill(second.password);
  await account.getByRole('button', { name: '계정 만들고 참여' }).click();
  await expect(koreanPage.getByTestId('app-shell')).toBeVisible();
  await look(koreanPage, 'the organization as a newcomer finds it, in Korean');

  // The same board: found, and joined by the first edit on it.
  await koreanPage.goto(page.url());
  await expect(koreanPage.getByTestId('board-page')).toBeVisible();
  await expect(koreanPage.getByTestId('board-join')).toHaveCount(0);
  await look(koreanPage, 'the board, in Korean');
  await koreanPage.getByText('Book the venue').click();
  await koreanPage.getByTestId('card-title').fill('Book the venue by Friday');
  await koreanPage.goBack();
  // The first person sees the change arrive.
  await expect(page.getByText('Book the venue by Friday')).toBeVisible();
  const cards = (await (
    await api.get(`/api/v1/w/${workspaceId}/boards/${boardId}/cards`)
  ).json()) as Schemas['Card'][];
  expect(cards.map((card) => card.title)).toEqual(['Book the venue by Friday']);

  // The page of an invitation in English, to somebody who is signed in.
  await page.goto(address);
  await expect(page.getByTestId('invite-summary')).toHaveText(
    `${ADMIN.username} invited you to ${ORG_NAME}`,
  );
  await look(page, 'the page of an invitation');
  await shot(page, 'invite-signed-in');

  for (const { screen, words } of seen) {
    expect(words, `${screen} says what the server calls it`).not.toMatch(SERVER_WORDS);
    expect(words.trim(), `${screen} says something`).not.toBe('');
  }
  // The tests that follow start in a workspace without boards for everybody: this
  // one is its members' alone from here on.
  const closed = await api.patch(`/api/v1/w/${workspaceId}/containers/${boardId}`, {
    data: { visibility: 'private' },
  });
  expect(closed.ok()).toBe(true);
  await api.dispose();
  await korean.close();
  expect(errors).toEqual([]);
});
