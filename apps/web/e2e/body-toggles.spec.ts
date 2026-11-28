/**
 * Toggles and callouts in a body, on the backend in memory: each is made
 * from the slash menu; a toggle is written in with Enter, opens and closes by
 * its chevron, and is left with Enter on an empty line; a callout's icon
 * changes its tone; Markdown pasted with `<details>` and an alert is a toggle
 * and a callout; and at 390 pixels across the chevron, the icon and the
 * tones are a finger's square and the page does not move sideways. Where a
 * body is only read, a toggle opens for its reader too, in the editor that
 * only reads and as the page's own `<details>` in a comment, and a callout's
 * icon is no button.
 *
 * `FVOCI_SCREENS=1` also takes the pictures of both, wide and narrow, light
 * and dark, into `target/screens/toggle-body/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { openCardWithBody, type Dev } from './embeds.support.ts';
import { lineBoundary } from './keyboard.support.ts';
import { screensDir } from './screens.ts';
import { hideSidebar, openApp } from './shell.support.ts';
import { paste, slash } from './tables.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

/** A new wiki's first page, named, its body ready to be written. */
async function newPage(page: Page): Promise<Locator> {
  await openApp(page);
  await newWiki(page, 'Notes');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Questions');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  return pageBody(page);
}

/** A toggle of a question and its answer, a callout of each of two tones, as Markdown. */
const MARKDOWN = [
  '<details>',
  '<summary>Why is the sky blue?</summary>',
  '',
  'Blue light is scattered the most.',
  '',
  '</details>',
  '',
  '> [!TIP]',
  '> Press the icon to change the kind.',
  '',
  '> [!CAUTION]',
  '> This cannot be undone.',
].join('\n');

/** The width and height of what a locator finds. */
async function sizeOf(found: Locator): Promise<[number, number]> {
  const box = await found.boundingBox();
  return [Math.round(box?.width ?? 0), Math.round(box?.height ?? 0)];
}

test('a toggle and a callout are made from the slash menu, opened, closed and given a tone', async ({
  page,
}) => {
  const body = await newPage(page);
  await body.click();
  await slash(page, 'toggle', 'Toggle');
  const toggle = body.locator('[data-details]');
  await expect(toggle).toHaveAttribute('data-open', '');
  await page.keyboard.type('Why?');
  // Enter goes on in what the toggle hides.
  await page.keyboard.press('Enter');
  await page.keyboard.type('Because.');
  const hidden = toggle.locator('.doc-details-content');
  await expect(toggle.locator('.doc-details-summary')).toHaveText('Why?');
  await expect(hidden).toHaveText('Because.');

  await toggle.getByRole('button', { name: 'Collapse' }).click();
  await expect(toggle).not.toHaveAttribute('data-open');
  await expect(hidden).toBeHidden();
  const expand = toggle.getByRole('button', { name: 'Expand' });
  await expect(expand).toHaveAttribute('aria-expanded', 'false');
  await expand.click();
  await expect(hidden).toBeVisible();
  await expect(toggle.getByRole('button', { name: 'Collapse' })).toHaveAttribute(
    'aria-expanded',
    'true',
  );

  // Enter twice at the end of what it hides leaves it, on a line of the document.
  await hidden.getByText('Because.').click();
  await lineBoundary(page, 'end');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await slash(page, 'callout', 'Callout');
  const callout = body.locator('[data-callout]');
  await expect(callout).toHaveAttribute('data-tone', 'info');
  await page.keyboard.type('Mind the gap');
  await expect(callout.locator('.doc-callout-content')).toHaveText('Mind the gap');
  await expect(hidden.locator('p')).toHaveCount(1);

  await callout.getByRole('button', { name: 'Callout: Note. Change its kind' }).click();
  const tones = callout.getByRole('group', { name: 'Kinds of callout' });
  await expect(tones.getByRole('button')).toHaveCount(4);
  await tones.getByRole('button', { name: 'Warning' }).click();
  await expect(callout).toHaveAttribute('data-tone', 'warn');
  await expect(tones).toBeHidden();
  await expect(
    callout.getByRole('button', { name: 'Callout: Warning. Change its kind' }),
  ).toBeVisible();
  // The caret is back in the body: typing goes on where it was.
  await page.keyboard.type('!');
  await expect(callout.locator('.doc-callout-content')).toHaveText('Mind the gap!');
  await expect(page.getByTestId('page-body').locator('[data-unreadable]')).toHaveCount(0);
});

test('Markdown pasted with a toggle and alerts is a toggle, closed, and callouts', async ({
  page,
}) => {
  const body = await newPage(page);
  await body.click();
  await paste(body, { 'text/plain': MARKDOWN });
  const toggle = body.locator('[data-details]');
  await expect(toggle.locator('.doc-details-summary')).toHaveText('Why is the sky blue?');
  await expect(toggle.locator('.doc-details-content')).toBeHidden();
  await toggle.getByRole('button', { name: 'Expand' }).click();
  await expect(toggle.locator('.doc-details-content')).toHaveText(
    'Blue light is scattered the most.',
  );
  await expect(body.locator('[data-callout]')).toHaveCount(2);
  await expect(body.locator('[data-callout]').first()).toHaveAttribute('data-tone', 'tip');
  await expect(body.locator('[data-callout]').last()).toHaveAttribute('data-tone', 'danger');
  await expect(body.locator('[data-callout]').last()).toContainText('This cannot be undone.');
  await expect(page.getByTestId('page-body').locator('[data-unreadable]')).toHaveCount(0);
});

test('at 390 pixels across the chevron, the icon and the tones are a finger wide, and the page stays put', async ({
  page,
}, testInfo) => {
  const body = await newPage(page);
  await body.click();
  await paste(body, { 'text/plain': MARKDOWN });
  await expect(body.locator('[data-callout]')).toHaveCount(2);
  await page.setViewportSize({ width: 390, height: 844 });
  await hideSidebar(page);

  const chevron = body.locator('.doc-details-button');
  const icon = body.locator('.doc-callout-icon').first();
  for (const control of [chevron, icon]) {
    const [width, height] = await sizeOf(control);
    expect(width).toBeGreaterThanOrEqual(44);
    expect(height).toBeGreaterThanOrEqual(44);
  }
  await chevron.click();
  await expect(body.locator('.doc-details-content')).toBeVisible();
  await icon.click();
  const tones = body.locator('[data-callout]').first().locator('.doc-callout-tone');
  await expect(tones).toHaveCount(4);
  for (const tone of await tones.all()) {
    const [width, height] = await sizeOf(tone);
    expect(width).toBeGreaterThanOrEqual(44);
    expect(height).toBeGreaterThanOrEqual(44);
    const box = await tone.boundingBox();
    expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(390);
  }
  await expect
    .poll(() =>
      page.evaluate(() => {
        const root = document.scrollingElement ?? document.documentElement;
        return root.scrollWidth <= root.clientWidth;
      }),
    )
    .toBe(true);

  if (process.env.FVOCI_SCREENS !== '1' || testInfo.project.name !== 'desktop') return;
  const dir = screensDir('toggle-body');
  for (const scheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: scheme, reducedMotion: 'reduce' });
    for (const [name, width] of [
      ['narrow', 390],
      ['wide', 1280],
    ] as const) {
      await page.setViewportSize({ width, height: 844 });
      if (name === 'narrow') await hideSidebar(page);
      await page.screenshot({ path: path.join(dir, `toggle-${name}-${scheme}.png`) });
    }
  }
});

/** A body of a question folded away and a warning, as the app stores it. */
const READ_BODY = {
  type: 'doc',
  content: [
    {
      type: 'details',
      content: [
        { type: 'detailsSummary', content: [{ type: 'text', text: 'A question' }] },
        {
          type: 'detailsContent',
          content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Its answer' }] }],
        },
      ],
    },
    {
      type: 'callout',
      attrs: { tone: 'warn' },
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Mind it' }] }],
    },
  ],
};

/** Has someone else comment `written` on the card open, the first of its board by title. */
async function commentWith(page: Page, written: object): Promise<void> {
  await page.evaluate(async (body) => {
    const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await dev.backend.workspaces();
    const boards = await dev.backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title.startsWith('Study notes'));
    const cards = await dev.backend.cards(board?.id ?? '');
    const [card] = [...cards].sort((one, other) => one.title.localeCompare(other.title));
    const other = dev.someoneElse() as unknown as {
      createComment(boardId: string, cardId: string, body: object): Promise<unknown>;
    };
    await other.createComment(board?.id ?? '', card?.id ?? '', body);
  }, written);
}

test('where a body is only read, a toggle still opens for its reader, and a callout keeps its tone', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Study notes', READ_BODY);
  await expect(body).toHaveAttribute('data-editor', 'reading');
  const answer = body.getByText('Its answer');
  await expect(answer).toBeHidden();
  await body.getByRole('button', { name: 'Expand' }).click();
  await expect(answer).toBeVisible();
  // The icon is a picture of the tone, not a button that would open nothing.
  await expect(body.getByRole('button', { name: /Callout/ })).toHaveCount(0);
  const icon = body.getByRole('img', { name: 'Warning' });
  await expect(icon).toBeVisible();
  await icon.click();
  await expect(body.locator('.doc-callout-tones')).toBeHidden();
  await expect(body.locator('[data-callout]')).toHaveAttribute('data-tone', 'warn');

  // In a comment, the page's own toggle: closed, and open once its summary is pressed.
  await commentWith(page, READ_BODY);
  const comment = page.getByTestId('card-comments').locator('[data-comment]').last();
  const details = comment.locator('details');
  await expect(details).toHaveCount(1);
  await expect(details).not.toHaveAttribute('open');
  await expect(comment.getByText('Its answer')).toBeHidden();
  await details.locator('summary').click();
  await expect(details).toHaveAttribute('open', '');
  await expect(comment.getByText('Its answer')).toBeVisible();
  await expect(comment.getByRole('img', { name: 'Warning' })).toBeVisible();
});

test('at 390 pixels across, the summary of a toggle in a comment is a finger tall, and its gutter opens it', async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await openCardWithBody(page, 'Study notes', READ_BODY);
  await commentWith(page, READ_BODY);
  const comment = page.getByTestId('card-comments').locator('[data-comment]').last();
  const details = comment.locator('details');
  const summary = details.locator('summary');
  await summary.scrollIntoViewIfNeeded();
  const box = await summary.boundingBox();
  const outer = await details.boundingBox();
  expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
  // The summary takes the whole width of the toggle, the chevron's gutter too.
  expect(Math.abs((box?.x ?? 0) - (outer?.x ?? 0))).toBeLessThanOrEqual(1);
  await expect(details).not.toHaveAttribute('open');
  // A press near the toggle's start, beside the chevron, opens it.
  await summary.click({ position: { x: 6, y: (box?.height ?? 44) / 2 } });
  await expect(details).toHaveAttribute('open', '');
  await summary.click({ position: { x: 36, y: (box?.height ?? 44) / 2 } });
  await expect(details).not.toHaveAttribute('open');
  const width = await page.evaluate(() => document.documentElement.scrollWidth);
  expect(width).toBeLessThanOrEqual(390);
});
