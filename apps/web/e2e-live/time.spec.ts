/**
 * Real-server end-to-end tests for personal time tracking (`time-t2-t2`):
 * 1. Start timer on a board card -> visible in TopBar pill (`active-timer-pill`).
 * 2. Switch to a course unit -> start timer there -> previous timer auto-stops with toast.
 * 3. Stop timer -> open "My time" (`/me/time`) -> check Day/Week/Month summary,
 *    manual entry (+ `endedAt < startedAt` validation), edit, delete, and DB persistence on reload.
 * 4. Privacy invariants: timer buttons are completely hidden on public shared board & course links
 *    (`/shared/...`) and on card templates.
 */
import { expect, test } from '@playwright/test';

import { showSidebar, sidebar } from '../e2e/shell.support.ts';

import {
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

test.use({ locale: 'en-US', timezoneId: 'UTC' });

function sharedPath(containerId: string, token: string, subId?: string, cardId?: string): string {
  const parts = [containerId, subId, cardId].filter((part) => part !== undefined);
  return `/shared/${parts.map(encodeURIComponent).join('/')}?r=${encodeURIComponent(token)}`;
}

test('card timer -> course unit auto-switch -> My time CRUD & persistence -> shared privacy', async ({
  browser,
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const errors = watchErrors(page);

  const owner = person('time-owner');
  await register(base, owner);
  const api = await signedIn(base, owner);

  const cardTitle = uniqueName('Sprint Task');
  const made = await makeBoard(api, {
    title: uniqueName('Time Board'),
    cards: [{ title: cardTitle, status: 'To Do' }],
  });
  const cardId = made.cards[cardTitle];
  if (!cardId) throw new Error('expected created card');

  // Create a card template on the same board to verify template privacy invariant
  const templateCard = (await (
    await api.post(`/api/v1/w/${made.workspaceId}/boards/${made.boardId}/cards`, {
      data: { title: 'Card Template', isTemplate: true },
    })
  ).json()) as Schemas['Card'];

  // Create a course with 1 chapter and 1 unit
  const course = (await (
    await api.post(`/api/v1/w/${made.workspaceId}/courses`, {
      data: { title: uniqueName('Time Course'), visibility: 'private' },
    })
  ).json()) as Schemas['Container'];
  await api.post(`/api/v1/w/${made.workspaceId}/courses/${course.id}/units`, {
    data: { kind: 'chapter', title: 'Chapter 1' },
  });
  const unitTitle = uniqueName('Lesson 1');
  const unit = (await (
    await api.post(`/api/v1/w/${made.workspaceId}/courses/${course.id}/units`, {
      data: { kind: 'unit', title: unitTitle },
    })
  ).json()) as Schemas['Unit'];

  // Enable public sharing on both the board and the course for privacy verification
  const boardShare = (await (
    await api.post(`/api/v1/w/${made.workspaceId}/containers/${made.boardId}/public-link`, {
      data: { enabled: true },
    })
  ).json()) as Schemas['PublicLink'];
  const courseShare = (await (
    await api.post(`/api/v1/w/${made.workspaceId}/containers/${course.id}/public-link`, {
      data: { enabled: true },
    })
  ).json()) as Schemas['PublicLink'];

  // Sign in on page and navigate directly to the card
  await page.goto('/');
  await signInOnPage(page, owner);
  await page.goto(boardPath(made, made.viewId, cardId));
  await expect(page.getByTestId('card-dialog')).toBeVisible();

  // 1. Start timer on card -> verify TopBar pill appears
  await page.getByTestId('target-timer-start').click();
  await expect(page.getByTestId('target-timer-stop')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('active-timer-pill')).toBeVisible();
  await expect(page.getByTestId('active-timer-title')).toContainText(cardTitle);

  // Verify template card hides timer buttons completely
  await page.goto(boardPath(made, made.viewId, templateCard.id));
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await expect(page.getByTestId('target-timer-start')).toHaveCount(0);
  await expect(page.getByTestId('target-timer-stop')).toHaveCount(0);

  // 2. Navigate to course unit and start timer -> auto-stops previous card timer with toast
  await page.goto(`/w/${made.workspaceId}/courses/${course.id}/${unit.id}`);
  await expect(page.getByTestId('unit-view')).toBeVisible();
  await page.getByTestId('target-timer-start').click();
  await expect(page.getByTestId('time-switch-toast')).toBeVisible();
  await expect(page.getByTestId('active-timer-title')).toContainText(unitTitle);

  // Stop the unit timer from the TopBar pill
  await page.getByTestId('active-timer-stop').click();
  await expect(page.getByTestId('active-timer-pill')).toBeHidden();

  // 3. Open "My time" screen from sidebar
  await showSidebar(page);
  await sidebar(page).getByTestId('open-my-time').click();
  await expect(page).toHaveURL(/\/me\/time$/);
  await expect(page.getByTestId('my-time-screen')).toBeVisible();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(2);

  // Check Day / Week / Month views
  await page.getByTestId('time-period-week').click();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(2);
  await page.getByTestId('time-period-month').click();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(2);
  await page.getByTestId('time-period-day').click();

  // Add manual entry (with inline validation check first)
  await page.getByTestId('time-add-button').click();
  await expect(page.getByTestId('time-entry-dialog')).toBeVisible();
  const day = (await page.getByTestId('time-form-ended-at').inputValue()).slice(0, 10);
  await page.getByTestId('time-form-kind-study').click();
  await page.getByTestId('time-form-started-at').fill(`${day}T14:00`);
  await page.getByTestId('time-form-ended-at').fill(`${day}T13:00`);
  await page.getByTestId('time-form-submit').click();
  await expect(page.getByTestId('time-form-error')).toBeVisible();

  await page.getByTestId('time-form-ended-at').fill(`${day}T15:00`);
  await page.getByTestId('time-form-note').fill('Deep study block');
  await page.getByTestId('time-form-submit').click();
  await expect(page.getByTestId('time-entry-dialog')).toBeHidden();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(3);
  await expect(page.getByTestId('time-summary-total')).toContainText('1h');

  // Reload to verify DB persistence
  await page.reload();
  await expect(page.getByTestId('my-time-screen')).toBeVisible();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(3);

  // Filter by study, edit note, then delete
  await page.getByTestId('time-filter-study').click();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(1);
  await page.getByTestId('time-entry-edit').first().click();
  await page.getByTestId('time-form-note').fill('Edited study block');
  await page.getByTestId('time-form-submit').click();
  await expect(page.getByTestId('time-entry-note')).toHaveText('Edited study block');

  await page.getByTestId('time-entry-delete').first().click();
  await page.getByRole('button', { name: /^delete$/i }).click();
  await expect(page.getByTestId('time-empty-state')).toBeVisible();

  // 4. Privacy invariants on public shared links (unauthenticated browser context)
  const anonContext = await browser.newContext({ locale: 'en-US' });
  const anonPage = await anonContext.newPage();
  const anonErrors = watchErrors(anonPage);

  if (!boardShare.token || !courseShare.token) throw new Error('expected share tokens');

  await anonPage.goto(sharedPath(made.boardId, boardShare.token, made.viewId, cardId));
  await expect(anonPage.getByTestId('card-dialog')).toBeVisible();
  await expect(anonPage.getByTestId('target-timer-start')).toHaveCount(0);
  await expect(anonPage.getByTestId('target-timer-stop')).toHaveCount(0);
  await expect(anonPage.getByTestId('active-timer-pill')).toHaveCount(0);

  await anonPage.goto(sharedPath(course.id, courseShare.token, unit.id));
  await expect(anonPage.getByTestId('shared-course')).toBeVisible();
  await expect(anonPage.getByTestId('target-timer-start')).toHaveCount(0);
  await expect(anonPage.getByTestId('target-timer-stop')).toHaveCount(0);
  await expect(anonPage.getByTestId('active-timer-pill')).toHaveCount(0);

  expect(anonErrors).toEqual([]);
  await anonContext.close();
  await api.dispose();
  expect(errors).toEqual([]);
});
