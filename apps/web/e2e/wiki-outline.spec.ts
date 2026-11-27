/**
 * The outline of a wiki's page, on the in-memory backend: beside the page in
 * a wide window, following it and marking the heading being read; behind a
 * button in the bar at 390 pixels; following the headings as they are
 * written, by the reader and by others; and changing nothing in the body.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import {
  bodyHeading,
  caretIn,
  currentItem,
  headingBelowTop,
  longBody,
  outline,
  outlineBeside,
  outlineItem as item,
  outlineItems,
  pageColumn,
  pageWith,
} from './outline.support.ts';
import { openCardWithBody } from './embeds.support.ts';
import { FIRST_BOARD } from './shell.support.ts';
import { letSomebodyElseIn, pageBody, typeAsSomebodyElse } from './wiki.support.ts';

test.use({ locale: 'en-US', reducedMotion: 'reduce' });

/** The page's body as somebody else's copy of the room has it, once it has `waitFor`. */
async function bodyElsewhere(
  page: Page,
  place: { wikiId: string; pageId: string },
  waitFor: string,
) {
  return typeAsSomebodyElse(page, place, waitFor, '');
}

test.describe('in a wide window', () => {
  test.use({ viewport: { width: 1600, height: 900 } });

  test('the outline is beside the page, stays as it scrolls, marks the heading read, and brings the one chosen below the head', async ({
    page,
  }, info) => {
    test.skip(info.project.name !== 'desktop', 'one wide window');
    const place = await pageWith(page, 'Guide', longBody(24));
    await expect(outlineItems(page)).toHaveCount(24);
    const beside = outlineBeside(page);
    await expect(beside).toBeVisible();
    await letSomebodyElseIn(page, place.wikiId);
    const before = await bodyElsewhere(page, place, 'Section 24');

    // At the right of the page, which keeps its width; each level a step further in.
    const view = await page.getByTestId('page-view').boundingBox();
    const side = await beside.boundingBox();
    expect(view?.width).toBe(768);
    expect(side?.x).toBeGreaterThanOrEqual((view?.x ?? 0) + (view?.width ?? 0) - 1);
    const insets = await outlineItems(page).evaluateAll((items) =>
      items.slice(0, 4).map((one) => parseFloat(getComputedStyle(one).paddingInlineStart)),
    );
    expect(insets[0]).toBeLessThan(insets[1] ?? 0);
    expect(insets[1]).toBeLessThan(insets[2] ?? 0);
    expect(insets[3]).toBe(insets[1]);

    // The reader scrolls: the outline stays where it was, the heading read is marked.
    await pageColumn(page).evaluate((column) => {
      const target = [...column.querySelectorAll('h1, h2, h3')].find(
        (heading) => heading.textContent === 'Section 10',
      );
      column.scrollTop +=
        (target?.getBoundingClientRect().top ?? 0) - column.getBoundingClientRect().top - 8;
    });
    await expect(currentItem(page)).toHaveText('Section 10');
    expect((await beside.boundingBox())?.y).toBe(side?.y);
    await expect(beside).toBeInViewport();

    // Chosen: its heading under the head of the page, marked, the caret at its start.
    await item(page, 'Section 18').click();
    await expect
      .poll(async () => Math.abs((await headingBelowTop(page, 'Section 18')) - 16))
      .toBeLessThanOrEqual(1);
    await expect(currentItem(page)).toHaveText('Section 18');
    await expect.poll(() => caretIn(page)).toEqual([true, 'Section 18']);
    await page.waitForTimeout(100);
    expect(await caretIn(page)).toEqual([true, 'Section 18']);

    // The last cannot come up to the head of the page, and is the one marked all the same.
    await item(page, 'Section 24').click();
    await expect(currentItem(page)).toHaveText('Section 24');
    await expect(bodyHeading(page, 'Section 24')).toBeInViewport();
    // Until the reader moves the page themselves.
    const column = await pageColumn(page).boundingBox();
    await page.mouse.move((view?.x ?? 0) + 100, (column?.y ?? 0) + 200);
    await page.mouse.wheel(0, -2500);
    await expect(currentItem(page)).not.toHaveText('Section 24');

    // Nothing of that went into the body. (That the outline's being there writes nothing
    // is measured from before there was one: in the unit test and on the real server.)
    expect(await bodyElsewhere(page, place, 'Section 24')).toBe(before);
  });

  test.describe('where the reader has not asked for less motion', () => {
    test.use({ reducedMotion: 'no-preference' });

    test('a heading chosen comes up smoothly to its place below the head, the caret in it', async ({
      page,
    }, info) => {
      test.skip(info.project.name !== 'desktop', 'one wide window');
      await pageWith(page, 'Guide', longBody(24));
      await expect(outlineBeside(page)).toBeVisible();
      await pageColumn(page).evaluate((column) => {
        column.scrollTop = 0;
        const seen = new Set<number>();
        column.addEventListener('scroll', () => seen.add(Math.round(column.scrollTop)));
        (window as unknown as { scrolledThrough: Set<number> }).scrolledThrough = seen;
      });
      await item(page, 'Section 18').click();
      await expect
        .poll(async () => Math.abs((await headingBelowTop(page, 'Section 18')) - 16))
        .toBeLessThanOrEqual(1);
      await expect.poll(() => caretIn(page)).toEqual([true, 'Section 18']);
      // Through the places between, not at once; and it stays there.
      expect(
        await page.evaluate(
          () => (window as unknown as { scrolledThrough: Set<number> }).scrolledThrough.size,
        ),
      ).toBeGreaterThan(2);
      await page.waitForTimeout(300);
      expect(Math.abs((await headingBelowTop(page, 'Section 18')) - 16)).toBeLessThanOrEqual(1);
      await expect(currentItem(page)).toHaveText('Section 18');
    });
  });

  test('folded away, it is behind a button in the bar, the page and its editor as they were; the button puts it back', async ({
    page,
  }, info) => {
    test.skip(info.project.name !== 'desktop', 'one wide window');
    await pageWith(page, 'Guide', longBody(6));
    await expect(outlineBeside(page)).toBeVisible();
    const editor = pageBody(page);
    await editor.evaluate((element) => {
      (element as HTMLElement & { kept?: boolean }).kept = true;
    });
    await page.getByTestId('fold-page-outline').click();
    await expect(outlineBeside(page)).toBeHidden();
    const button = page.getByRole('button', { name: 'Show the contents beside the page' });
    await expect(button).toBeFocused();
    expect(await page.evaluate(() => localStorage.getItem('fvoci:wiki-outline'))).toBe('folded');
    expect((await page.getByTestId('page-view').boundingBox())?.width).toBe(768);
    expect(await editor.evaluate((element) => (element as { kept?: boolean }).kept)).toBe(true);
    await button.press('Enter');
    await expect(outlineBeside(page)).toBeVisible();
    await expect(page.getByTestId('fold-page-outline')).toBeFocused();
    expect(await page.evaluate(() => localStorage.getItem('fvoci:wiki-outline'))).toBe('beside');

    // The list is one stop of the Tab key, at the heading read; the arrows go along it.
    await page.keyboard.press('Tab');
    await expect(currentItem(page)).toBeFocused();
    await page.keyboard.press('Home');
    await expect(outlineItems(page).first()).toBeFocused();
    await page.keyboard.press('End');
    await expect(outlineItems(page).last()).toBeFocused();
    await page.keyboard.press('ArrowUp');
    await expect(outlineItems(page).nth(4)).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(outlineBeside(page).locator(':focus')).toHaveCount(0);
    await expect(page.getByTestId('page-view').locator(':focus')).toHaveCount(1);
  });

  test('a list left out behind the button is shut when the outline went beside the page and came back', async ({
    page,
  }, info) => {
    test.skip(info.project.name !== 'desktop', 'one window, resized');
    await page.setViewportSize({ width: 1280, height: 800 });
    await pageWith(page, 'Guide', longBody(6));
    const button = page.getByTestId('open-page-outline');
    await button.click();
    await expect(outlineItems(page)).toHaveCount(6);
    await page.setViewportSize({ width: 1600, height: 900 });
    await expect(outlineBeside(page)).toBeVisible();
    await expect(button).toHaveCount(0);
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(button).toBeVisible();
    await expect(button).toHaveAttribute('aria-expanded', 'false');
    await expect(outline(page)).toHaveCount(0);
  });

  test('it follows the headings as they are written, by the reader and by others; a page with fewer than two has none', async ({
    page,
  }, info) => {
    test.skip(info.project.name !== 'desktop', 'one wide window');
    const place = await pageWith(page, 'Notes', '');
    const editor = pageBody(page);
    await editor.click();
    await page.keyboard.type('# First');
    await page.keyboard.press('Enter');
    await page.keyboard.type('Some words.');
    await page.keyboard.press('Enter');
    await expect(outline(page)).toHaveCount(0);
    await editor.evaluate((element) => {
      (element as HTMLElement & { kept?: boolean }).kept = true;
    });
    await page.keyboard.type('## Second');
    await expect(outlineItems(page)).toHaveText(['First', 'Second']);
    // The editor kept the focus and was not made anew for the outline.
    await expect(editor).toBeFocused();
    expect(await editor.evaluate((element) => (element as { kept?: boolean }).kept)).toBe(true);

    // Somebody else adds to the first heading.
    await letSomebodyElseIn(page, place.wikiId);
    await typeAsSomebodyElse(page, place, 'Second', ' and more');
    await expect(outlineItems(page)).toHaveText(['First and more', 'Second']);

    // The reader takes the second heading back out: one is left, and no outline.
    for (let left = 'Second'.length; left > 0; left -= 1) await page.keyboard.press('Backspace');
    await page.keyboard.press('Backspace');
    await expect(outline(page)).toHaveCount(0);
    await expect(editor).toBeFocused();
  });
});

test.describe('at 390 pixels', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('a button in the bar at the head opens the outline; with the keyboard alone a heading is gone to', async ({
    page,
  }, info) => {
    test.skip(info.project.name !== 'desktop', 'the keyboard, in a narrow window');
    await pageWith(page, 'Guide', longBody(12));
    const button = page.getByTestId('open-page-outline');
    await expect(button).toBeVisible();
    await expect(outlineBeside(page)).toHaveCount(0);
    await expect(outline(page)).toHaveCount(0);
    // In the bar with the button of the pages, above the page.
    await pageColumn(page).evaluate((column) => {
      column.scrollTop = 0;
    });
    const [pages, opener, view] = await Promise.all([
      page.getByTestId('open-page-tree').boundingBox(),
      button.boundingBox(),
      page.getByTestId('page-view').boundingBox(),
    ]);
    expect(opener?.y).toBe(pages?.y);
    expect((opener?.y ?? 0) + (opener?.height ?? 0)).toBeLessThanOrEqual(view?.y ?? 0);

    await page.getByTestId('open-page-tree').focus();
    await page.keyboard.press('Tab');
    await expect(button).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(button).toHaveAttribute('aria-expanded', 'true');
    await expect(outlineItems(page)).toHaveCount(12);
    await expect(outlineItems(page).first()).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(outline(page)).toHaveCount(0);
    await expect(button).toBeFocused();

    await page.keyboard.press('Enter');
    for (let step = 0; step < 6; step += 1) await page.keyboard.press('ArrowDown');
    await expect(outlineItems(page).nth(6)).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(outline(page)).toHaveCount(0);
    await expect(button).toHaveAttribute('aria-expanded', 'false');
    // Below the bar, not under it.
    const bar = await button.boundingBox();
    await expect
      .poll(async () => (await bodyHeading(page, 'Section 7').boundingBox())?.y ?? 0)
      .toBeGreaterThanOrEqual((bar?.y ?? 0) + (bar?.height ?? 0));
    await expect.poll(() => headingBelowTop(page, 'Section 7')).toBeLessThan(80);
    await expect.poll(() => caretIn(page)).toEqual([true, 'Section 7']);
  });

  test('under a finger, a heading chosen is shown without the keyboard coming up', async ({
    page,
  }, info) => {
    test.skip(info.project.name !== 'mobile', 'a finger');
    await pageWith(page, 'Guide', longBody(12));
    const button = page.getByTestId('open-page-outline');
    await button.tap();
    await item(page, 'Section 9').tap();
    await expect(outline(page)).toHaveCount(0);
    await expect(bodyHeading(page, 'Section 9')).toBeInViewport();
    await expect(button).toBeFocused();
    await expect(pageBody(page)).not.toBeFocused();
  });
});

/** Installs in the page a count of the outline's changes and a clock from each key to the frame after it. */
async function watchTyping(page: Page): Promise<void> {
  await page.evaluate(() => {
    const watch = { timing: false, changes: 0, times: [] as number[] };
    (window as unknown as { typing: typeof watch }).typing = watch;
    const nav = document.querySelector('[data-testid="page-outline-beside"]');
    if (nav) {
      new MutationObserver((records) => {
        if (watch.timing) watch.changes += records.length;
      }).observe(nav, { subtree: true, childList: true, characterData: true, attributes: true });
    }
    document.addEventListener(
      'keydown',
      () => {
        if (!watch.timing) return;
        const start = performance.now();
        requestAnimationFrame(() =>
          requestAnimationFrame(() => watch.times.push(performance.now() - start)),
        );
      },
      { capture: true },
    );
  });
}

/** Types `typed` at the end of `place`, giving the time of each key and the outline's changes meanwhile. */
async function timedTyping(page: Page, place: Locator, typed: string) {
  await place.click();
  await page.keyboard.press('End');
  await page.waitForTimeout(100);
  const typing = () =>
    page.evaluate(
      () =>
        (window as unknown as { typing: { timing: boolean; changes: number; times: number[] } })
          .typing,
    );
  await page.evaluate(() => {
    const watch = (
      window as unknown as { typing: { timing: boolean; changes: number; times: number[] } }
    ).typing;
    [watch.timing, watch.changes, watch.times] = [true, 0, []];
  });
  await page.keyboard.type(typed, { delay: 60 });
  await page.waitForTimeout(200);
  const seen = await typing();
  await page.evaluate(() => {
    (window as unknown as { typing: { timing: boolean } }).typing.timing = false;
  });
  return { times: seen.times, changes: seen.changes };
}

const median = (values: number[]): number => {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)] ?? 0;
};

test('a keystroke on a page of 300 headings: the outline is not drawn again for typing in a paragraph', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'measured once');
  test.setTimeout(120_000);
  await page.setViewportSize({ width: 1600, height: 900 });
  // Short paragraphs: the whole body is one paste, which has to be small enough to send.
  const words = 'A short paragraph of the section.';
  await pageWith(page, 'Long', longBody(300, words));
  await expect(outlineItems(page)).toHaveCount(300);
  await watchTyping(page);
  const below = (heading: string): Locator =>
    bodyHeading(page, heading).locator('xpath=following-sibling::p[1]');
  const inParagraph = await timedTyping(page, below('Section 150'), 'abcdefghijklmnopqrst');
  expect(inParagraph.times).toHaveLength(20);
  // Typing in a paragraph changes nothing in the outline, not even which heading is marked.
  expect(inParagraph.changes).toBe(0);
  const inHeading = await timedTyping(page, bodyHeading(page, 'Section 151'), ' xyz');
  await expect(item(page, 'Section 151 xyz')).toBeVisible();
  expect(inHeading.changes).toBeGreaterThan(0);

  // The same body on a card, which has no outline: what the keystroke costs without it.
  const doc = {
    type: 'doc',
    content: Array.from({ length: 300 }, (_, at) => [
      {
        type: 'heading',
        attrs: { level: [1, 2, 3, 2][at % 4] },
        content: [{ type: 'text', text: `Section ${at + 1}` }],
      },
      { type: 'paragraph', content: [{ type: 'text', text: words }] },
    ]).flat(),
  };
  const card = await openCardWithBody(page, FIRST_BOARD, doc);
  await expect(card.locator('h2').filter({ hasText: /^Section 150$/ })).toBeAttached();
  await watchTyping(page);
  const onCard = await timedTyping(
    page,
    card
      .locator('h2')
      .filter({ hasText: /^Section 150$/ })
      .locator('xpath=following-sibling::p[1]'),
    'abcdefghijklmnopqrst',
  );
  const said = (values: number[]): string =>
    `median ${median(values).toFixed(1)}, most ${Math.max(...values).toFixed(1)}`;
  info.annotations.push({
    type: 'keystroke to the frame after it, ms (300 headings)',
    description: `page, paragraph: ${said(inParagraph.times)}; page, heading: ${said(inHeading.times)}; card without outline, paragraph: ${said(onCard.times)}`,
  });
});
