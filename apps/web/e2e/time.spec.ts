/**
 * Memory-backend browser E2E for personal time tracking:
 * - Starting/stopping a timer from a card detail, top-bar pill display & navigation
 * - Automatic switch when starting a timer on another target + toast notice
 * - "My time" screen (`/me/time`): Day/Week/Month views, summary, kind filter,
 *   manual entry with `endedAt < startedAt` inline validation, edit, and delete
 */
import { expect, test } from '@playwright/test';

import { openApp, showSidebar, sidebar } from './shell.support.ts';

test.use({ locale: 'en-US', timezoneId: 'UTC' });

test('card timer, single-active auto-switch, and My time CRUD', async ({ page }) => {
  await openApp(page);

  // Open the first card on the seeded board
  const cards = page.locator('[data-card]');
  await expect(cards.first()).toBeVisible();
  await cards.first().click();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  const titleField = page.getByTestId('card-title');
  const firstCardTitle = (await titleField.inputValue().catch(() => titleField.innerText())).trim();

  // Start timer from card header
  await page.getByTestId('target-timer-start').click();
  await expect(page.getByTestId('target-timer-stop')).toBeVisible();

  // Close card and verify global TopBar pill shows the running timer
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  await expect(page.getByTestId('active-timer-pill')).toBeVisible();
  if (firstCardTitle) {
    await expect(page.getByTestId('active-timer-title')).toContainText(firstCardTitle);
  }
  await expect(page.getByTestId('active-timer-elapsed')).toHaveText(/^\d{2}:\d{2}:\d{2}$/);

  // Open second card and start timer -> previous auto-stops with toast
  await cards.nth(1).click();
  await expect(page.getByTestId('card-dialog')).toBeVisible();
  await page.getByTestId('target-timer-start').click();
  await expect(page.getByTestId('time-switch-toast')).toBeVisible();
  await expect(page.getByTestId('target-timer-stop')).toBeVisible();

  // Close card and stop via TopBar pill
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('card-dialog')).toBeHidden();
  await page.getByTestId('active-timer-stop').click();
  await expect(page.getByTestId('active-timer-pill')).toBeHidden();

  // Navigate to "My time" via sidebar
  await showSidebar(page);
  await sidebar(page).getByTestId('open-my-time').click();
  await expect(page).toHaveURL(/\/me\/time$/);
  await expect(page.getByTestId('my-time-screen')).toBeVisible();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(2);

  // Period switching: Day -> Week -> Month -> Day
  await page.getByTestId('time-period-week').click();
  await expect(page.getByTestId('time-period-week')).toHaveAttribute('aria-pressed', 'true');
  await page.getByTestId('time-period-month').click();
  await expect(page.getByTestId('time-period-month')).toHaveAttribute('aria-pressed', 'true');
  await page.getByTestId('time-period-day').click();
  await expect(page.getByTestId('time-period-day')).toHaveAttribute('aria-pressed', 'true');

  // Manual entry: first verify endedAt < startedAt inline validation
  await page.getByTestId('time-add-button').click();
  await expect(page.getByTestId('time-entry-dialog')).toBeVisible();
  const day = (await page.getByTestId('time-form-ended-at').inputValue()).slice(0, 10);
  await page.getByTestId('time-form-kind-study').click();
  await page.getByTestId('time-form-started-at').fill(`${day}T10:30`);
  await page.getByTestId('time-form-ended-at').fill(`${day}T09:30`);
  await page.getByTestId('time-form-submit').click();
  await expect(page.getByTestId('time-form-error')).toBeVisible();

  // Fix end time and submit manual entry (1h 30m)
  await page.getByTestId('time-form-ended-at').fill(`${day}T12:00`);
  await page.getByTestId('time-form-note').fill('Algorithms practice');
  await page.getByTestId('time-form-submit').click();
  await expect(page.getByTestId('time-entry-dialog')).toBeHidden();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(3);
  await expect(page.getByTestId('time-summary-total')).toContainText('1h 30m');

  // Filter by study -> only the manual study entry is shown
  await page.getByTestId('time-filter-study').click();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(1);
  await expect(page.getByTestId('time-entry-note')).toHaveText('Algorithms practice');

  // Edit the manual entry (change kind to lecture and update note)
  await page.getByTestId('time-entry-edit').first().click();
  await expect(page.getByTestId('time-entry-dialog')).toBeVisible();
  await page.getByTestId('time-form-kind-lecture').click();
  await page.getByTestId('time-form-note').fill('Recorded lecture review');
  await page.getByTestId('time-form-submit').click();
  await expect(page.getByTestId('time-entry-dialog')).toBeHidden();

  // Filter by lecture -> updated entry appears
  await page.getByTestId('time-filter-lecture').click();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(1);
  await expect(page.getByTestId('time-entry-note')).toHaveText('Recorded lecture review');

  // Delete entry with confirmation
  await page.getByTestId('time-entry-delete').first().click();
  await page.getByRole('button', { name: /^delete$/i }).click();
  await expect(page.getByTestId('time-empty-state')).toBeVisible();

  // Back to All -> original 2 timer entries remain
  await page.getByTestId('time-filter-all').click();
  await expect(page.getByTestId('time-entry-row')).toHaveCount(2);
});
