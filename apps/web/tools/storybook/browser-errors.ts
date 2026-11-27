import { setTimeout as observe } from 'node:timers/promises';
import { expect, type Page } from '@playwright/test';

/** Observe delayed effects after rendering settles; this is an error-detection window,
 * not a retry that makes an assertion pass. Errors remain recorded throughout it. */
const OBSERVATION_MS = 2000;

export function browserErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));
  return errors;
}

export async function assertNoBrowserErrors(errors: string[], label: string): Promise<void> {
  await observe(OBSERVATION_MS);
  expect(errors, `${label}: browser errors`).toEqual([]);
}
