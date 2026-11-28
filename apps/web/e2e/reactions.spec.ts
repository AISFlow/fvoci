/**
 * Reactions, end to end, on the in-memory backend: the reader answers a
 * comment and a page with an emoji from the quick row, sees it pressed, and
 * takes it off; what somebody else does arrives by itself, named. At 390
 * pixels under a finger a long press on a comment opens the emoji as a sheet,
 * and every target is at least 44 pixels; on one being written it is the
 * browser's.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { openApp } from './shell.support.ts';
import { letSomebodyElseIn, nameThePage, newWiki, wikiPlaceOf } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

/** What `fvociDev.someoneElse()` offers these tests (src/app/backend.ts). */
interface Elsewhere {
  me(): Promise<{ id: string; displayName?: string; username: string }>;
  reactions: {
    add(workspaceId: string, containerId: string, blockId: string, emoji: string): Promise<void>;
  };
}
interface Dev {
  someoneElse(): Elsewhere;
}

interface Place {
  wikiId: string;
  pageId: string;
}

const comments = (page: Page): Locator => page.getByTestId('card-comments');
const commentRows = (page: Page): Locator => comments(page).locator('[data-comment]');
const composer = (page: Page): Locator => page.getByTestId('comment-composer');

/** A new wiki of the reader with one page, "Notes", shown, and a comment on it. */
async function aCommentedPage(page: Page): Promise<Place> {
  await openApp(page);
  await newWiki(page, 'Handbook');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Notes');
  await composer(page).getByTestId('comment-field').fill('Shall we?');
  await composer(page).getByTestId('comment-send').click();
  await expect(commentRows(page).last()).toContainText('Shall we?');
  return wikiPlaceOf(page.url());
}

/** Somebody else, on the wiki, answers `blockId` with `emoji`; their name. */
async function reactedElsewhere(
  page: Page,
  place: Place,
  blockId: string,
  emoji: string,
): Promise<string> {
  return page.evaluate(
    async ({ at, block, chosen }) => {
      const other = (window as unknown as { fvociDev: Dev }).fvociDev.someoneElse();
      const workspaceId = location.pathname.split('/')[2] ?? '';
      await other.reactions.add(workspaceId, at.wikiId, block, chosen);
      const me = await other.me();
      return me.displayName ?? me.username;
    },
    { at: place, block: blockId, chosen: emoji },
  );
}

test('a comment and a page are answered with an emoji, taken off, and followed', async ({
  page,
}) => {
  const place = await aCommentedPage(page);
  const row = commentRows(page).last();
  const commentId = (await row.getAttribute('data-comment')) ?? '';
  const onComment = row.getByTestId('reactions');
  await letSomebodyElseIn(page, place.wikiId);

  await onComment.locator('[data-action="react"]').click();
  await page
    .getByRole('group', { name: 'Quick reactions' })
    .getByRole('button', { name: '👍' })
    .click();
  const thumbs = onComment.locator('[data-reaction="👍"]');
  await expect(thumbs).toHaveAttribute('aria-pressed', 'true');
  await expect(thumbs).toHaveAccessibleName(/^👍 1: you/);

  const name = await reactedElsewhere(page, place, commentId, '👍');
  // The reader's own shows at once, before it is saved, and the other's may be saved
  // first: who was saved first is named first, the other with their handle, maybe.
  const them = String.raw`${name}(?: \(@[^)]+\))?`;
  await expect(thumbs).toHaveAccessibleName(
    new RegExp(`^👍 2: (?:you and ${them}|${them} and you)`),
  );
  await thumbs.click();
  await expect(thumbs).toHaveAttribute('aria-pressed', 'false');
  await expect(thumbs).toHaveAccessibleName(new RegExp(`^👍 1: ${name}`));

  await reactedElsewhere(page, place, place.pageId, '🎉');
  const onPage = page.locator(`[data-testid="reactions"][data-block="${place.pageId}"]`);
  await expect(onPage.locator('[data-reaction="🎉"]')).toHaveAccessibleName(
    new RegExp(`^🎉 1: ${name}`),
  );
});

test('at 390 pixels a long press opens the emoji as a sheet, and the targets take a finger', async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== 'mobile', 'a finger on a phone');
  await page.setViewportSize({ width: 390, height: 844 });
  await aCommentedPage(page);
  const row = commentRows(page).last();
  await row.scrollIntoViewIfNeeded();
  const box = await row.boundingBox();
  if (!box) throw new Error('the comment is not shown');
  const at = { clientX: box.x + 20, clientY: box.y + 10, pointerType: 'touch', isPrimary: true };
  await row.dispatchEvent('pointerdown', { ...at, pointerId: 7, button: 0 });
  await page.waitForTimeout(700);
  await row.dispatchEvent('pointerup', { ...at, pointerId: 7, button: 0 });

  const sheet = page.getByRole('dialog');
  await expect(sheet).toBeVisible();
  const quick = sheet.getByRole('group', { name: 'Quick reactions' });
  for (const button of await quick.getByRole('button').all()) {
    const size = await button.boundingBox();
    expect(Math.min(size?.width ?? 0, size?.height ?? 0)).toBeGreaterThanOrEqual(44);
  }
  await quick.getByRole('button', { name: '🚀' }).click();
  await expect(sheet).toBeHidden();
  const chip = row.locator('[data-reaction="🚀"]');
  await expect(chip).toHaveAttribute('aria-pressed', 'true');
  for (const target of [chip, row.locator('[data-action="react"]')]) {
    const size = await target.boundingBox();
    expect(Math.min(size?.width ?? 0, size?.height ?? 0)).toBeGreaterThanOrEqual(44);
  }

  // While the comment is being written a long press is the browser's: its menu, and no sheet.
  await row.locator('[data-action="edit"]').click();
  await expect(row.getByTestId('comment-edit')).toBeVisible();
  await row.dispatchEvent('pointerdown', { ...at, pointerId: 8, button: 0 });
  const menuLeft = await row.evaluate((element) =>
    element.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })),
  );
  expect(menuLeft).toBe(true);
  await page.waitForTimeout(700);
  await row.dispatchEvent('pointerup', { ...at, pointerId: 8, button: 0 });
  await expect(page.getByRole('dialog')).toBeHidden();
});
