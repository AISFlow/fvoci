/**
 * The carets of a body edited together, end to end on the in-memory backend:
 * two tabs of one browser have the same card open (the rooms in memory tell
 * each other's carets, as the server's do), and each shows where the other
 * is, with the reader's name for a while after they moved.
 *
 * A syllable being composed with an input method is typed through the
 * browser's own input method interface (`Input.imeSetComposition`), which
 * drives the same composition events a keyboard's does.
 *
 * `FVOCI_SCREENS=1` also takes the pictures of the carets, into
 * `target/screens/caret/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath } from './address.ts';
import { documentBoundary, engineOf, lineBoundary } from './keyboard.support.ts';
import { scrollTo, uncovered } from './carets.support.ts';
import { screensDir } from './screens.ts';

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

interface Place {
  boardId: string;
  viewId: string;
  cardId: string;
}

interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
}
interface Dev {
  backend: DevBackend;
}

/** A card of the board of tasks, by its title. */
async function find(page: Page, card: string): Promise<Place> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  return page.evaluate(async (cardTitle) => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const found = boards.find((each) => each.title.startsWith('Project schedule'));
    if (!found) throw new Error('no board of tasks');
    const views = await backend.views(found.id);
    const wanted = (await backend.cards(found.id)).find((each) => each.title === cardTitle);
    if (!wanted) throw new Error(`no card ${cardTitle}`);
    const first = [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
    return { boardId: found.id, viewId: first?.id ?? '', cardId: wanted.id };
  }, card);
}

async function bodyEditor(page: Page): Promise<Locator> {
  await expect(page.getByTestId('card-body')).toHaveAttribute('data-editor', 'ready');
  return page.getByTestId('card-body').locator('[contenteditable]').first();
}

/**
 * The same card open in two tabs: the editors of its body, and the second
 * tab. With `body`, the first tab's reader writes those lines in place of
 * what the body held (each tab has a world of its own, made alike: what one
 * writes reaches the other through their rooms), and leaves the body again.
 * `query` is put after the first tab's address (a setting of its world).
 */
async function twoTabs(
  page: Page,
  body?: string[],
  query = '',
): Promise<{ mine: Locator; theirs: Locator; other: Page }> {
  const place = await find(page, 'Design the screens');
  await page.goto(`${boardPath(place.boardId, place.viewId, place.cardId)}${query}`);
  const mine = await bodyEditor(page);
  const other = await page.context().newPage();
  await other.goto(boardPath(place.boardId, place.viewId, place.cardId));
  const theirs = await bodyEditor(other);
  if (body) {
    // Chosen whole by the page, not by a key: with an iPhone's or iPad's identity on Linux's
    // WebKit the editor reads Apple from the User-Agent and touch, and takes Ctrl+A for the
    // start of the line, while the platform (and so the key pressed) says Linux.
    await mine.selectText();
    await page.keyboard.press('Delete');
    for (const [index, line] of body.entries()) {
      if (index > 0) {
        await page.keyboard.press('Enter');
        // WebKit takes text sent at once after Enter into the line it left.
        await expect(mine.locator('p')).toHaveCount(index + 1);
      }
      await page.keyboard.insertText(line);
    }
    await expect(mine.locator('p')).toHaveText(body);
    await page.getByTestId('card-title').click();
  }
  // Each has what the other's room holds, and no caret of the other is left.
  await expect(theirs).toHaveText((await mine.textContent()) ?? '');
  await expect(theirs.locator('.doc-caret')).toHaveCount(0);
  return { mine, theirs, other };
}

const caret = (editor: Locator): Locator => editor.locator('.doc-caret');
const nameOf = (editor: Locator): Locator => editor.locator('.doc-caret-name');
/** How far the name was moved from its caret to stay in sight, in pixels: less than nothing is leftwards. */
const moved = (editor: Locator): Promise<number> =>
  nameOf(editor).evaluate((name) => Number.parseFloat(name.style.getPropertyValue('--moved')) || 0);

/** The text of a line ahead of the caret drawn in it; null when none is. */
const ahead = (line: Locator): Promise<string | null> =>
  line.evaluate((element) => {
    const drawn = element.querySelector('.doc-caret');
    if (!drawn) return null;
    const range = document.createRange();
    range.setStart(element, 0);
    range.setEndBefore(drawn);
    return range.toString();
  });

/**
 * The other's caret goes to the start of its first line and back to the end, and this tab has
 * seen both: the three seconds its name shows begin anew. A name is placed only while it
 * shows, and End on a caret that is at the end already moves nothing. The frame in which
 * the name is placed for that move has passed when this comes back: what places it after
 * is something else (another width).
 */
async function movedAgain(other: Page, mine: Locator, text: string): Promise<void> {
  const line = mine.locator('p').first();
  await lineBoundary(other, 'start');
  await expect.poll(() => ahead(line)).toBe('');
  await lineBoundary(other, 'end');
  await expect.poll(() => ahead(line)).toBe(text);
  await mine.evaluate(
    () =>
      new Promise((resolve) => {
        requestAnimationFrame(resolve);
      }),
  );
}

/** The name shows, and is within the body: placed a frame after it is drawn. */
async function inside(editor: Locator): Promise<void> {
  await expect(caret(editor)).toHaveAttribute('data-named', '');
  await expect
    .poll(async () => {
      const [box, name] = await Promise.all([editor.boundingBox(), nameOf(editor).boundingBox()]);
      if (!box || !name) return 'not drawn';
      const fits =
        name.x >= box.x - 1 && name.x + name.width <= box.x + box.width + 1 && name.y >= box.y - 1;
      return fits ? 'inside' : `outside: ${JSON.stringify({ box, name })}`;
    })
    .toBe('inside');
}

test("each tab shows where the other is, with the reader's name for a while after they moved", async ({
  page,
}) => {
  const { mine, theirs, other } = await twoTabs(page, ['The first line.', 'The second line.']);
  await expect(caret(mine)).toHaveCount(0);

  // The other tab's reader goes into the body: a caret, and their name.
  await theirs.locator('p').nth(1).click();
  await expect(caret(mine)).toHaveCount(1);
  await expect(caret(mine)).toHaveAttribute('data-named', '');
  await expect(nameOf(mine)).toHaveAttribute('data-name', /\S/);
  await expect(nameOf(mine)).toHaveCSS('opacity', '1');
  // It is in the line they are in, and is no part of what a screen reader is told.
  await expect(mine.locator('p').nth(1).locator('.doc-caret')).toHaveCount(1);
  await expect(caret(mine)).toHaveAttribute('aria-hidden', 'true');
  // Nobody is shown their own caret.
  await expect(caret(theirs)).toHaveCount(0);

  // The name goes after a few seconds; the line stays.
  await expect(caret(mine)).not.toHaveAttribute('data-named', '', { timeout: 6000 });
  await expect(nameOf(mine)).toHaveCSS('opacity', '0');
  await expect(caret(mine)).toHaveCount(1);
  // They write: the caret follows, and the name is back.
  await lineBoundary(other, 'end');
  await other.keyboard.type(' More.');
  await expect(mine.locator('p').nth(1)).toHaveText('The second line. More.');
  await expect(caret(mine)).toHaveAttribute('data-named', '');
  // A selection of theirs is tinted.
  await lineBoundary(other, 'start', { select: true });
  await expect(mine.locator('.doc-caret-selection').first()).toBeVisible();

  // Both ways: the first tab's caret shows in the second.
  await mine.locator('p').first().click();
  await expect(caret(theirs)).toHaveCount(1);
  // Leaving the body takes the caret away.
  await page.getByTestId('card-title').click();
  await expect(caret(theirs)).toHaveCount(0);
  // (A tab that goes away says nothing to the rooms in memory: its caret is let go thirty
  // seconds after its last word. The server tells at once: its tests, and those against it.)
  await other.close();
});

test('a name is not cut off at the edges: under the first line, and moved left at the right edge', async ({
  page,
}) => {
  // A word too long for a row is cut at the right edge: the end of its first row is at the edge.
  const { mine, theirs, other } = await twoTabs(page, ['W'.repeat(400), 'Below.']);
  // At the very start of the body: nothing above to draw on.
  await theirs.locator('p').first().click();
  await documentBoundary(other, 'start');
  await expect(caret(mine)).toHaveAttribute('data-below', '');
  await inside(mine);
  // One letter before the end of the first row of the word: at the right edge. (The very end
  // of a row is the start of the next, where a caret is drawn.)
  await lineBoundary(other, 'end');
  await other.keyboard.press('ArrowLeft');
  await inside(mine);
  await expect.poll(() => moved(mine)).toBeLessThan(0);
  // In the line below there is room above, once it is in sight.
  await documentBoundary(other, 'end');
  // (On a phone the long word's rows take it out of sight: it is scrolled back into it.)
  await mine.locator('p').nth(1).scrollIntoViewIfNeeded();
  await expect(caret(mine)).not.toHaveAttribute('data-below', '');
  await inside(mine);
  await other.close();
});

test('a name is not cut off at the top of a body scrolled in its card, drawn or scrolled after', async ({
  page,
}) => {
  const lines = Array.from({ length: 60 }, (_, index) => `Line ${String(index + 1)}.`);
  const { mine, theirs, other } = await twoTabs(page, lines);
  // The first tab's body is scrolled until the thirtieth line is the first one seen.
  expect(await scrollTo(mine.locator('p').nth(29))).toBe(true);
  await theirs.locator('p').nth(29).click();
  await lineBoundary(other, 'end');
  await expect(caret(mine)).toHaveAttribute('data-named', '');
  await expect(caret(mine)).toHaveAttribute('data-below', '');
  await expect.poll(() => uncovered(mine)).toBe(true);
  // Drawn above a line in sight, the name goes under it as that line is scrolled to the top.
  await theirs.locator('p').nth(33).click();
  await lineBoundary(other, 'end');
  await expect(caret(mine)).not.toHaveAttribute('data-below', '');
  await expect(caret(mine)).toHaveAttribute('data-named', '');
  expect(await scrollTo(mine.locator('p').nth(33))).toBe(true);
  await expect(caret(mine)).toHaveAttribute('data-below', '');
  await expect.poll(() => uncovered(mine)).toBe(true);
  await other.close();
});

test('a name finds its place again when the window is another width', async ({ page }) => {
  const { mine, theirs, other } = await twoTabs(page, ['Short.', 'Below.']);
  await page.setViewportSize({ width: 1280, height: 800 });
  await theirs.locator('p').first().click();
  await lineBoundary(other, 'end');
  await expect(caret(mine)).toHaveAttribute('data-named', '');
  // Wide, there is room to the right of a short line; narrow, there is none, and the name
  // is moved left as far as it takes, never past the left edge.
  await expect.poll(() => moved(mine)).toBe(0);
  // Each width has the name's whole three seconds to itself: with one move at the start,
  // all three widths had to be laid out and placed within three seconds of it.
  for (const width of [390, 150]) {
    await movedAgain(other, mine, 'Short.');
    await page.setViewportSize({ width, height: 800 });
    await inside(mine);
  }
  await expect.poll(() => moved(mine)).toBeLessThan(0);
  await movedAgain(other, mine, 'Short.');
  await page.setViewportSize({ width: 1280, height: 800 });
  // A name that went out before it was placed says so: it is not a name placed wrongly.
  await expect
    .poll(async () =>
      (await caret(mine).getAttribute('data-named')) === null
        ? 'the name went out before it was placed'
        : moved(mine),
    )
    .toBe(0);
  await other.close();
});

test('a caret never blinks, and its name does not fade for a reader who asked for less motion', async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  const { mine, theirs, other } = await twoTabs(page);
  await theirs.locator('p').first().click();
  await expect(caret(mine)).toHaveCount(1);
  await expect(caret(mine)).toHaveCSS('animation-name', 'none');
  await expect(nameOf(mine)).toHaveCSS('animation-name', 'none');
  // No time to speak of: the app's own rule for less motion leaves a hundredth of a millisecond.
  const fades = (): Promise<number> =>
    nameOf(mine).evaluate((name) => Number.parseFloat(getComputedStyle(name).transitionDuration));
  expect(await fades()).toBeLessThan(0.001);
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect(caret(mine)).toHaveCSS('animation-name', 'none');
  expect(await fades()).toBeGreaterThan(0.1);
  await other.close();
});

test('a syllable being composed is kept while the other moves and writes in the same line', async ({
  page,
}) => {
  const engine = engineOf(page);
  test.skip(
    engine !== 'chromium',
    `an input method is driven through Chromium's protocol; ${engine ?? 'this browser'} has none`,
  );
  // What the room sends waits two seconds after a stroke (HELD_IDLE), and the other's steps
  // between two strokes take longer than that on a busy machine: the reader's world holds it
  // for as long as the test may take, so that the strokes alone say the syllable goes on.
  // Letting go after the quiet is the room client's unit test (doc-room-carets.test.ts).
  const { mine, theirs, other } = await twoTabs(
    page,
    ['가나다 라마바', 'Below.'],
    `?held=${test.info().timeout}`,
  );
  const ime = await page.context().newCDPSession(page);
  const compose = (text: string): Promise<unknown> =>
    ime.send('Input.imeSetComposition', {
      text,
      selectionStart: text.length,
      selectionEnd: text.length,
    });
  const commit = (text: string): Promise<unknown> => ime.send('Input.insertText', { text });

  // The reader composes at the end of the first line: ㅎ, 하.
  await mine.locator('p').first().click();
  await lineBoundary(page, 'end');
  await compose('ㅎ');
  await compose('하');
  await expect(mine.locator('p').first()).toHaveText('가나다 라마바하');

  // Meanwhile the other comes into the same line (their caret is told at once; the editor
  // draws it into the line being composed when it can).
  await theirs.locator('p').first().click();
  await lineBoundary(other, 'start');
  await other.keyboard.press('ArrowRight');
  // They write there, and in another line. Neither is in the reader's body while the syllable
  // is being composed (each stroke says it still is).
  await other.keyboard.type('앞에 ');
  await documentBoundary(other, 'end');
  await other.keyboard.type(' 아래.');
  await expect(theirs.locator('p')).toHaveText(['가앞에 나다 라마바하', 'Below. 아래.']);
  await compose('한');
  // The room has all the other wrote; then they go to the start of the line below, where they
  // have not been. A caret is not held back, and what the room says is heard in its order:
  // once that caret is in the reader's page, all they wrote has come, and waits.
  await expect(other.getByTestId('card-body')).not.toHaveAttribute('data-pending');
  await lineBoundary(other, 'start');
  await expect.poll(() => ahead(mine.locator('p').nth(1))).toBe('');
  await expect(mine.locator('p')).toHaveText(['가나다 라마바한', 'Below.']);

  // The syllable is committed and the next begins at once, as a keyboard does it: what waited
  // comes in between the two, and the next syllable is composed on it undisturbed.
  await commit('한');
  await compose('ㄱ');
  await expect(mine.locator('p')).toHaveText(['가앞에 나다 라마바한ㄱ', 'Below. 아래.']);
  await compose('그');
  await compose('글');
  await commit('글');
  for (const body of [mine, theirs]) {
    await expect(body.locator('p')).toHaveText(['가앞에 나다 라마바한글', 'Below. 아래.']);
  }
  // The other's caret is where they go, also behind what waited when it was written.
  await lineBoundary(other, 'end');
  await expect.poll(() => ahead(mine.locator('p').nth(1))).toBe('Below. 아래.');
  await other.close();
});

test('a move of the caret by a key is kept while the other writes and moves', async ({ page }) => {
  const { mine, theirs, other } = await twoTabs(page, ['a'.repeat(24), 'Below.']);
  await theirs.locator('p').nth(1).click();
  await lineBoundary(other, 'end');
  await mine.locator('p').first().click();

  // The other writes without a pause: every letter changes the body and moves their caret,
  // and each comes to the reader's page between two of the reader's keys or in the moment
  // after one. The browser moves the reader's caret itself (Home, an arrow) and tells the
  // editor a moment later: no move is taken back by what came in meanwhile.
  const written = 'x'.repeat(400);
  const still = { writing: true };
  const writing = other.keyboard.type(written, { delay: 8 }).finally(() => {
    still.writing = false;
  });
  try {
    // While they write, and no fewer than these.
    for (let round = 0; round < 12 || (still.writing && round < 60); round += 1) {
      await lineBoundary(page, 'start');
      for (let step = 0; step < 20; step += 1) await page.keyboard.press('ArrowRight');
      expect(await page.evaluate(() => getSelection()?.anchorOffset)).toBe(20);
    }
  } finally {
    await writing.catch(() => undefined);
  }
  for (const body of [mine, theirs]) {
    await expect(body.locator('p')).toHaveText(['a'.repeat(24), `Below.${written}`]);
  }
  await other.close();
});

test('the pictures of the carets', async ({ page }, testInfo) => {
  test.skip(!process.env.FVOCI_SCREENS, 'set FVOCI_SCREENS=1 to take the pictures');
  test.skip(testInfo.project.name !== 'desktop', 'the sizes are set here');
  const long = 'A line long enough to reach the right edge of the body and to wrap beyond it. ';
  for (const [size, width, height] of [
    ['wide', 1280, 800],
    ['390', 390, 844],
  ] as const) {
    for (const scheme of ['light', 'dark'] as const) {
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme: scheme });
      const { mine, theirs, other } = await twoTabs(page, [long.repeat(2), 'The second line.']);
      await other.setViewportSize({ width, height });
      const shot = async (name: string): Promise<void> => {
        await expect(caret(mine)).toHaveAttribute('data-named', '');
        await page.screenshot({
          path: path.join(screensDir('caret'), `${name}-${size}-${scheme}.png`),
        });
      };
      await theirs.locator('p').nth(1).click();
      await lineBoundary(other, 'start');
      await lineBoundary(other, 'end', { select: true });
      await shot('selection');
      await documentBoundary(other, 'start');
      await lineBoundary(other, 'end');
      await other.keyboard.press('ArrowLeft');
      await expect.poll(() => moved(mine)).toBeLessThan(0);
      await shot('right-edge');
      await documentBoundary(other, 'start');
      await expect(caret(mine)).toHaveAttribute('data-below', '');
      await shot('first-line');
      await other.close();
    }
  }
});
