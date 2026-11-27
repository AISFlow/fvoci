// The manager, the page at the site's `/`, under the headers the site is
// served with: it frames a story and logs no error, a violation of the
// policy among them. smoke.spec.ts opens iframe.html alone.
import { expect, test } from '@playwright/test';

import { assertNoBrowserErrors, browserErrors } from './browser-errors.ts';

const STORY = 'activity-audit-list--every-action';

test('the manager frames a story under the policy without errors', async ({ page }, info) => {
  const errors = browserErrors(page);
  const globals =
    info.project.name === 'ko-dark-390' ? 'locale:ko;theme:dark' : 'locale:en;theme:light';
  const response = await page.goto(`?path=/story/${STORY}&globals=${globals}`);
  expect(response?.headers()['content-security-policy']).toContain("frame-ancestors 'self'");
  const frame = page.frameLocator('#storybook-preview-iframe').locator('html');
  await expect(frame).toHaveAttribute('data-story-finished', STORY);
  await expect(frame).toHaveAttribute('data-story-result', 'success');
  await assertNoBrowserErrors(errors, 'manager');
});
