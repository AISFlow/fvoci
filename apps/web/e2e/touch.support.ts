/**
 * A touch screen for the tests of fingers: touch events sent through the
 * browser's protocol, as a screen sends them. Only Chromium takes them that
 * way. Playwright's WebKit can tap but not press, hold and move a finger, so
 * a test of a finger's drag or swipe is skipped there, with the reason, and
 * keeps running in the Chromium projects.
 */
import { test, type CDPSession, type Page } from '@playwright/test';

import { engineOf } from './keyboard.support.ts';

export async function touchScreen(page: Page): Promise<CDPSession> {
  const engine = engineOf(page);
  test.skip(
    engine !== 'chromium',
    `a finger that holds and moves needs Chromium's touch events; ${engine ?? 'this browser'} can only tap`,
  );
  return page.context().newCDPSession(page);
}
