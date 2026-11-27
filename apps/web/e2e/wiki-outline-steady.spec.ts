/**
 * The outline of a wiki's page keeps the page still and its mark true, on
 * the in-memory backend: typing in the heading chosen keeps it the one
 * marked; the outline coming or going as a second heading is typed or taken
 * out moves neither the page nor its caret, at the widths where it is behind
 * the button and where it is beside the page; a page switch brings the next
 * page's outline with its body; the mark follows when the column grows or
 * what is above the body grows; and on a touch screen a heading chosen with
 * the keyboard gets the caret. With the scrollbars shown, as where they take
 * room: a page's layout must not depend on whether it overflows.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import {
  caretIn,
  currentItem,
  headingBelowTop,
  longBody,
  outlineBeside,
  outlineItem,
  outlineItems,
  pageColumn,
  pageWith,
  pasteIntoBody,
} from './outline.support.ts';
import { letSomebodyElseIn, pageBody, pageBySomebodyElse, treeItem } from './wiki.support.ts';

test.use({
  locale: 'en-US',
  reducedMotion: 'reduce',
  launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] },
});

const WIDE = { width: 1600, height: 900 };

test('typing in the heading chosen keeps it the one marked; a key that moves the page lets go of it', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'one wide window');
  await page.setViewportSize(WIDE);
  await pageWith(page, 'Guide', longBody(24));
  // The last heading cannot come up to the reading line: only the choice marks it.
  await outlineItem(page, 'Section 24').click();
  await expect(currentItem(page)).toHaveText('Section 24');
  await expect.poll(() => caretIn(page)).toEqual([true, 'Section 24']);
  await page.keyboard.press('End');
  await page.keyboard.type(' and more');
  // Read again for the words typed, and still the one marked.
  await expect(outlineItem(page, 'Section 24 and more')).toBeVisible();
  await expect(currentItem(page)).toHaveText('Section 24 and more');
  await page.keyboard.press('Enter');
  await page.keyboard.type('A line below it.');
  await expect(pageBody(page)).toContainText('A line below it.');
  await expect(currentItem(page)).toHaveText('Section 24 and more');

  // A key that moves the page: the mark is the reading line's again.
  await page.keyboard.press('PageUp');
  await expect(currentItem(page)).not.toHaveText('Section 24 and more');
});

test.describe('where the reader has not asked for less motion', () => {
  test.use({ reducedMotion: 'no-preference' });

  // A key with a modifier held is one key: Shift, Control, Command alone move nothing.
  for (const key of ['End', 'Shift+End'] as const) {
    test(`${key} pressed while the heading chosen is still coming up does not let go of it`, async ({
      page,
    }, info) => {
      test.skip(info.project.name !== 'desktop', 'one wide window');
      await page.setViewportSize(WIDE);
      await pageWith(page, 'Guide', longBody(24));
      await pageColumn(page).evaluate((column) => {
        column.scrollTop = 0;
      });
      await outlineItem(page, 'Section 24').click();
      await page.keyboard.press(key);
      // The scroll comes to rest (the key may have cut it short).
      await expect.poll(() => scrollAtRest(page)).toBe(true);
      await expect(currentItem(page)).toHaveText('Section 24');
      if (key !== 'End') return;
      await page.keyboard.type(' and more');
      await expect(outlineItem(page, 'Section 24 and more')).toBeVisible();
      await expect(currentItem(page)).toHaveText('Section 24 and more');
    });
  }

  for (const then of ['the wheel', 'a second key'] as const) {
    test(`once a key cut the scroll to the heading chosen short, ${then} lets go of it`, async ({
      page,
    }, info) => {
      test.skip(info.project.name !== 'desktop', 'one wide window');
      await page.setViewportSize(WIDE);
      await pageWith(page, 'Guide', longBody(24));
      await pageColumn(page).evaluate((column) => {
        column.scrollTop = 0;
      });
      await outlineItem(page, 'Section 20').focus();
      // End goes with Enter, not a round trip after it: on a busy machine the scroll to
      // the heading could be over by then, with nothing left to cut short.
      await Promise.all([page.keyboard.press('Enter'), page.keyboard.press('End')]);
      await expect.poll(() => scrollAtRest(page)).toBe(true);
      // Cut short: brought all the way up, the heading would rest near the top, well within 100 px.
      const restsAt = await headingBelowTop(page, 'Section 20');
      expect(restsAt, 'End cut the scroll short').toBeGreaterThan(100);
      await expect(currentItem(page)).toHaveText('Section 20');
      // Short of it, the reader moves on towards it, and is reading what is under the line.
      if (then === 'the wheel') {
        const column = await pageColumn(page).boundingBox();
        await page.mouse.move((column?.x ?? 0) + 200, (column?.y ?? 0) + 300);
        await page.mouse.wheel(0, 200);
      } else {
        // Depending on where End stopped the smooth scroll, Section 20 can rest a few lines
        // above the bottom edge of the column; step down until the caret actually scrolls it.
        const before = await pageColumn(page).evaluate((column) => column.scrollTop);
        let scrolled = false;
        for (let step = 0; step < 30 && !scrolled; step += 1) {
          await page.keyboard.press('ArrowDown');
          scrolled = await pageColumn(page).evaluate(
            (column, from) => column.scrollTop > from,
            before,
          );
        }
        expect(scrolled).toBe(true);
      }
      await expect.poll(() => scrollAtRest(page)).toBe(true);
      await expect(currentItem(page)).not.toHaveText('Section 20');
    });
  }
});

/** Whether the page's column has not scrolled over two frames. */
async function scrollAtRest(page: Page): Promise<boolean> {
  return pageColumn(page).evaluate(
    (column) =>
      new Promise<boolean>((done) => {
        const from = column.scrollTop;
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            done(column.scrollTop === from);
          }),
        );
      }),
  );
}

/** Where the block of the caret and the block above it are on the screen; none for a block not drawn. */
async function placesOnScreen(page: Page): Promise<number[]> {
  return page.evaluate(() => {
    const anchor = getSelection()?.anchorNode;
    const block = (anchor instanceof Element ? anchor : anchor?.parentElement)?.closest(
      'p, h1, h2, h3',
    );
    return [block, block?.previousElementSibling].flatMap((element) => {
      const box = element?.getBoundingClientRect();
      if (!box || box.height === 0) return [];
      return [Math.round(box.left * 10) / 10, Math.round(box.top * 10) / 10];
    });
  });
}

/** Whether the caret is at the end of the last block of the body. */
async function caretAtTheEnd(page: Page): Promise<boolean> {
  return pageBody(page).evaluate((editor) => {
    const selection = getSelection();
    const at = selection?.anchorNode;
    return (
      at !== null &&
      at !== undefined &&
      editor.lastElementChild?.contains(at) === true &&
      selection?.anchorOffset ===
        (at.nodeType === Node.TEXT_NODE ? (at.textContent ?? '').length : at.childNodes.length)
    );
  });
}

/** The outline, beside the page or behind its button: there is one. */
const anOutline = (page: Page): Locator =>
  page.locator('[data-testid="page-outline-beside"], [data-testid="open-page-outline"]');

/** Widths with the app's sidebar and the page tree open, and where the outline is at each. */
const MIDDLE = [
  { width: 1280, height: 800, where: 'behind the button', shown: 'open-page-outline' },
  { width: 1440, height: 900, where: 'beside a narrowed page', shown: 'page-outline-beside' },
  { width: 1600, height: 900, where: 'beside a page moved left', shown: 'page-outline-beside' },
];

for (const { width, height, where, shown } of MIDDLE) {
  for (const long of [false, true]) {
    test(`at ${String(width)} pixels (${where}), ${long ? 'scrolled down a long page' : 'at the top of a short page'}, the outline coming and going moves neither the page nor the caret`, async ({
      page,
    }, info) => {
      test.skip(info.project.name !== 'desktop', 'one window per width');
      await page.setViewportSize({ width, height });
      const words = long ? '<p>Words of the page, one line after another.</p>'.repeat(40) : '';
      await pageWith(page, 'Notes', `<h1>First</h1><p>Some words.</p>${words}`);
      await pageBody(page).click();
      // At the end of the last block, before anything is typed (the key again if a redraw took it).
      await expect(async () => {
        await page.keyboard.press('ControlOrMeta+End');
        expect(await caretAtTheEnd(page)).toBe(true);
      }).toPass();
      await page.keyboard.press('Enter');
      await page.keyboard.type('## ');
      await expect(pageBody(page).locator('h2')).toHaveCount(1);
      await expect(anOutline(page)).toHaveCount(0);
      if (long)
        expect(await pageColumn(page).evaluate((column) => column.scrollTop)).toBeGreaterThan(0);
      const before = await placesOnScreen(page);
      expect(before).toHaveLength(4);

      await page.keyboard.type('S');
      await expect(page.getByTestId(shown)).toBeVisible();
      expect(await placesOnScreen(page)).toEqual(before);
      await page.keyboard.press('Backspace');
      await expect(anOutline(page)).toHaveCount(0);
      expect(await placesOnScreen(page)).toEqual(before);
    });
  }
}

test('at 1440 pixels with the sidebar and the page tree open, the page gives up some of its width for the outline beside it', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'one window');
  await page.setViewportSize({ width: 1440, height: 900 });
  await pageWith(page, 'Guide', longBody(6));
  await expect(outlineBeside(page)).toBeVisible();
  await expect(page.getByTestId('page-tree')).toBeVisible();
  const [view, side] = await Promise.all([
    page.getByTestId('page-view').boundingBox(),
    outlineBeside(page).boundingBox(),
  ]);
  // Narrower than 48rem, not narrower than 40rem; the outline at its right.
  expect(view?.width).toBeLessThan(768);
  expect(view?.width).toBeGreaterThanOrEqual(640);
  expect(side?.x).toBeGreaterThanOrEqual((view?.x ?? 0) + (view?.width ?? 0) - 1);
});

test('near the width where the outline goes beside the page, a page that comes to overflow keeps its layout', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'one window, scrollbars that take room');
  // A column of about 52rem with the sidebar and the page tree: the layouts' boundary.
  await page.setViewportSize({ width: 1352, height: 900 });
  await pageWith(page, 'Notes', '<p>Line 1</p>');
  await pageBody(page).click();
  await page.keyboard.press('ControlOrMeta+End');
  const boxes: string[] = [];
  for (let line = 2; line <= 24; line += 1) {
    await page.keyboard.press('Enter');
    await page.keyboard.type(`Line ${String(line)}`);
    await expect(pageBody(page).locator('p').last()).toHaveText(`Line ${String(line)}`);
    // Two frames on, after anything the line set going.
    await page.evaluate(
      () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
    );
    const box = await page.getByTestId('page-view').boundingBox();
    boxes.push(`${String(box?.x)} ${String(box?.width)}`);
  }
  // It came to overflow, and the page stayed where it was drawn at the first line.
  expect(
    await pageColumn(page).evaluate((column) => column.scrollHeight > column.clientHeight),
  ).toBe(true);
  expect(new Set(boxes).size).toBe(1);
});

/** What each frame showed: the body's headings, and the outline's items, as their first words and count. */
interface Shown {
  body: number;
  bodyFirst: string;
  items: number;
  itemFirst: string;
}

test('a page switch brings the next page’s outline with its body, without a frame of none between', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'one wide window');
  await page.setViewportSize(WIDE);
  const place = await pageWith(page, 'Guide', longBody(6));
  await letSomebodyElseIn(page, place.wikiId);
  await pageBySomebodyElse(page, place.wikiId, { title: 'Other' });
  await treeItem(page, 'Other').click();
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  await pasteIntoBody(page, '<h2>Other 1</h2><p>One.</p><h2>Other 2</h2><p>Two.</p>');
  await expect(outlineItems(page)).toHaveText(['Other 1', 'Other 2']);

  // What each frame paints: sampled where a ResizeObserver hears of a probe changed in
  // every frame, which is after the frame's animation callbacks and its layout.
  await page.evaluate(() => {
    const seen: Shown[] = [];
    (window as unknown as { shownFrames: Shown[] }).shownFrames = seen;
    const probe = document.createElement('div');
    probe.style.cssText = 'position: fixed; top: 0; left: 0; height: 1px; pointer-events: none';
    document.body.append(probe);
    new ResizeObserver(() => {
      const body = document.querySelectorAll('[data-testid="page-body"] .doc-body :is(h1, h2, h3)');
      const items = document.querySelectorAll('[data-testid="page-outline"] li button');
      seen.push({
        body: body.length,
        bodyFirst: body[0]?.textContent ?? '',
        items: items.length,
        itemFirst: items[0]?.textContent.trim() ?? '',
      });
    }).observe(probe);
    let frame = 0;
    const next = (): void => {
      frame += 1;
      probe.style.width = `${String(1 + (frame % 2))}px`;
      requestAnimationFrame(next);
    };
    requestAnimationFrame(next);
  });
  for (const [to, first, count] of [
    ['Guide', 'Section 1', 6],
    ['Other', 'Other 1', 2],
  ] as const) {
    await treeItem(page, to).click();
    await expect(outlineItems(page)).toHaveCount(count);
    await expect(outlineItems(page).first()).toHaveText(first);
  }
  await page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
  );
  const shown = await page.evaluate(
    () => (window as unknown as { shownFrames: Shown[] }).shownFrames,
  );
  // Frames were seen of both pages, each with its own outline.
  expect(shown.some((frame) => frame.body === 6 && frame.items === 6)).toBe(true);
  expect(shown.some((frame) => frame.body === 2 && frame.items === 2)).toBe(true);
  // Every frame whose body had headings had the same in its outline.
  expect(
    shown.filter(
      (frame) =>
        frame.body >= 2 && (frame.items !== frame.body || frame.itemFirst !== frame.bodyFirst),
    ),
  ).toEqual([]);
});

/** Scrolls the page's column so that the top of `words` is `below` pixels under its reading line. */
async function headingUnderTheLine(page: Page, words: string, below: number): Promise<void> {
  await pageColumn(page).evaluate(
    (column, [heading, by]) => {
      const target = [...column.querySelectorAll('.doc-body :is(h1, h2, h3)')].find(
        (one) => one.textContent === heading,
      );
      const line = column.getBoundingClientRect().top + column.clientHeight / 4;
      column.scrollTop += (target?.getBoundingClientRect().top ?? 0) - line - Number(by);
    },
    [words, String(below)],
  );
}

/** A page of 24 sections beside its outline, Section 11 just under the reading line. */
async function sectionsUnderTheLine(page: Page): Promise<void> {
  await page.setViewportSize(WIDE);
  await pageWith(page, 'Guide', longBody(24));
  await expect(outlineBeside(page)).toBeVisible();
  await headingUnderTheLine(page, 'Section 11', 60);
  await expect(currentItem(page)).toHaveText('Section 10');
}

test('the heading marked follows when the column grows taller', async ({ page }, info) => {
  test.skip(info.project.name !== 'desktop', 'one window, resized');
  await sectionsUnderTheLine(page);
  // The reading line goes 125 pixels lower, past Section 11.
  await page.setViewportSize({ width: WIDE.width, height: WIDE.height + 500 });
  await expect(currentItem(page)).toHaveText('Section 11');
});

test('the heading marked follows when what is above the body grows without a scroll', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'desktop', 'one wide window');
  await sectionsUnderTheLine(page);
  // What is above moving does not scroll the column (no anchoring, as at its top).
  await pageColumn(page).evaluate((column) => {
    column.style.overflowAnchor = 'none';
  });
  await headingUnderTheLine(page, 'Section 11', -60);
  await expect(currentItem(page)).toHaveText('Section 11');
  // The head of the page grows: Section 11 goes down below the line.
  await page.getByTestId('page-heading').evaluate((head) => {
    (head as HTMLElement).style.paddingTop = '200px';
  });
  await expect(currentItem(page)).toHaveText('Section 10');
});

test('on a touch screen, a heading chosen with the keyboard gets the caret', async ({
  page,
}, info) => {
  test.skip(info.project.name !== 'mobile', 'a touch screen with a keyboard');
  await pageWith(page, 'Guide', longBody(12));
  expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true);
  const button = page.getByTestId('open-page-outline');
  await button.focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('[data-testid="page-outline"] li button:focus')).toHaveCount(1);
  await page.keyboard.press('Home');
  for (let step = 0; step < 3; step += 1) await page.keyboard.press('ArrowDown');
  await expect(outlineItems(page).nth(3)).toBeFocused();
  await page.keyboard.press('Enter');
  await expect.poll(() => caretIn(page)).toEqual([true, 'Section 4']);
});
