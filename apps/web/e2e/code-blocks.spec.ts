/**
 * Code blocks end to end, on the in-memory backend: coloured under the
 * policy by a worker of the app's own, in the editor and where a body is
 * only read; the language changed from the keys; the copy button; the
 * numbers of the lines; an input method and a second reader in the same
 * block; and a block too long to colour, which stays plain while
 * the page goes on.
 *
 * `FVOCI_SCREENS=1` also takes the pictures, wide and 390 px wide, light and
 * dark, into `target/screens/code-blocks/` (`e2e/screens.ts`).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { boardPath, placeOf } from './address.ts';
import { openCardWithBody, type Dev } from './embeds.support.ts';
import { clipboardPermissions, engineOf, tabKey } from './keyboard.support.ts';
import { screensDir } from './screens.ts';

test.describe.configure({ timeout: 90_000 });

const SOURCE = [
  '// The answer, asked for.',
  'export function ask(question: string): number {',
  '  const answer = 42;',
  '  return question.length > 0 ? answer : -1;',
  '}',
].join('\n');

const codeBody = (language: string | null, source: string): object => ({
  type: 'doc',
  content: [
    { type: 'paragraph', content: [{ type: 'text', text: 'Before the code.' }] },
    {
      type: 'codeBlock',
      attrs: { language },
      content: [{ type: 'text', text: source }],
    },
  ],
});

/** What the page refused under its content security policy. */
function refusals(page: Page): string[] {
  const seen: string[] = [];
  page.on('console', (message) => {
    if (message.text().includes('Content Security Policy')) seen.push(message.text());
  });
  return seen;
}

const block = (body: Locator): Locator => body.locator('.doc-code').first();

/** Where the editor holds the caret, as an offset into the text of the body's first code block. */
const caretInText = (body: Locator): Promise<number> =>
  body
    .locator('[contenteditable="true"]')
    .first()
    .evaluate((root) => {
      const { state } = (
        root as HTMLElement & {
          editor: {
            state: {
              selection: { head: number };
              doc: {
                descendants: (
                  visit: (node: { type: { name: string } }, pos: number) => boolean,
                ) => void;
              };
            };
          };
        }
      ).editor;
      let start = -1;
      state.doc.descendants((node, pos) => {
        if (start < 0 && node.type.name === 'codeBlock') start = pos + 1;
        return start < 0;
      });
      return state.selection.head - start;
    });

async function coloured(body: Locator): Promise<void> {
  await expect(block(body).locator('.doc-code-keyword').first()).toBeVisible({ timeout: 30_000 });
}

/** The colour a run is drawn in, and the colour of the block's plain text. */
async function colours(body: Locator): Promise<{ keyword: string; plain: string }> {
  return block(body)
    .locator('pre > code')
    .evaluate((code) => ({
      keyword: getComputedStyle(code.querySelector('.doc-code-keyword') ?? code).color,
      plain: getComputedStyle(code).color,
    }));
}

test('a code block in the editor is coloured by a worker of the app, under the policy', async ({
  page,
}) => {
  const seen = refusals(page);
  const workers: string[] = [];
  page.on('worker', (worker) => workers.push(worker.url()));
  const body = await openCardWithBody(page, 'Getting started', codeBody('ts', SOURCE));
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await coloured(body);
  const { keyword, plain } = await colours(body);
  expect(keyword).not.toBe(plain);
  await expect(block(body).locator('.doc-code-language')).toHaveAccessibleName(/TypeScript/);
  // The text is the text: no letter of it moved into the bar.
  await expect(block(body).locator('pre > code')).toHaveText(SOURCE);
  expect(workers.some((url) => new URL(url).pathname.includes('/code-highlight-worker-'))).toBe(
    true,
  );
  expect(workers.every((url) => new URL(url).origin === new URL(page.url()).origin)).toBe(true);
  expect(seen).toEqual([]);
});

test('a worker slow to load still colours: a text’s time starts once the worker is ready', async ({
  page,
}) => {
  // Longer than a text is given (HIGHLIGHT_TIME), as on a slow network.
  await page.route(/code-highlight-worker-[^/]*\.js$/, async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    await route.continue();
  });
  const body = await openCardWithBody(page, 'Getting started', codeBody('ts', SOURCE));
  await coloured(body);
});

test('the language is found and taken from the keys, and the bar is reached with Tab', async ({
  page,
}) => {
  const body = await openCardWithBody(page, 'Getting started', codeBody('ts', SOURCE));
  // First the written body, then its editor: the editor is ready on the seed body first,
  // and clicking before the replacement arrives lands on the seed paragraph instead.
  await expect(body.locator('p').first()).toHaveText('Before the code.');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  const language = block(body).locator('button.doc-code-language');
  await expect(language).toBeVisible();
  // From the text before the block, Tab is the bar's first button, and Shift+Tab the text again.
  const tab = await tabKey(page);
  await body.locator('p').first().click();
  await page.keyboard.press(tab);
  await expect(language).toBeFocused();
  await page.keyboard.press(await tabKey(page, true));
  await expect(body.locator('[contenteditable="true"]').first()).toBeFocused();
  await page.keyboard.press(tab);
  await expect(language).toBeFocused();
  await page.keyboard.press('ArrowDown');
  const search = block(body).getByRole('combobox');
  await expect(search).toBeFocused();
  await expect(language).toHaveAttribute('aria-expanded', 'true');
  // The field and the rows stay inside the list's box, and the rows are not indented as the body's lists are.
  const popup = block(body).locator('.doc-code-popup');
  const boxOf = async (part: Locator) => {
    const box = await part.boundingBox();
    if (!box) throw new Error('the part is not on the page');
    return box;
  };
  const around = await boxOf(popup);
  for (const part of [search, block(body).getByRole('option').first()]) {
    const box = await boxOf(part);
    expect(box.x).toBeGreaterThanOrEqual(around.x);
    expect(box.x + box.width).toBeLessThanOrEqual(around.x + around.width);
    expect(box.x - around.x).toBeLessThan(8);
  }
  await page.keyboard.type('pyth');
  await expect(block(body).getByRole('option')).toHaveText(['Python']);
  await page.keyboard.press('Enter');
  await expect(language).toBeFocused();
  await expect(language).toHaveAccessibleName(/Python/);
  await expect(block(body).locator('pre > code')).toHaveClass('language-python');
  // The copy button is the next stop.
  await page.keyboard.press(tab);
  await expect(block(body).locator('button.doc-code-copy')).toBeFocused();
  // What is typed with the focus on a button is not typed into the body.
  await page.keyboard.type('zz');
  await expect(block(body).locator('pre > code')).toHaveText(SOURCE);
  // Escape gives the focus back to the button, the list closed and nothing changed.
  await language.focus();
  await page.keyboard.press('Enter');
  await expect(search).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(language).toBeFocused();
  await expect(language).toHaveAttribute('aria-expanded', 'false');
  await expect(language).toHaveAccessibleName(/Python/);
});

test('the copy button copies the text exactly, without the numbers of the lines, and says so', async ({
  page,
}) => {
  const engine = engineOf(page) ?? 'chromium';
  await page.context().grantPermissions(clipboardPermissions(engine));
  await page.addInitScript(() => {
    localStorage.setItem('fvoci:code-lines', '1');
  });
  const body = await openCardWithBody(page, 'Getting started', codeBody('ts', SOURCE));
  await expect(block(body).locator('.doc-code-line')).toHaveCount(5);
  const copy = block(body).locator('button.doc-code-copy');
  await expect(copy).toHaveAccessibleName(/Copy code|코드 복사/);
  await copy.click();
  await expect(copy).toHaveAttribute('data-copied', '');
  await expect(block(body).getByRole('status')).not.toBeEmpty();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(SOURCE);
  // And for a moment only.
  await expect(copy).not.toHaveAttribute('data-copied', '', { timeout: 5_000 });
  await expect(block(body).getByRole('status')).toBeEmpty();
});

test('the numbers of the lines stay beside the first row of a line that wraps, and out of the selection', async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem('fvoci:code-lines', '1');
  });
  const long = `const words = "${'word '.repeat(80)}";\nconst after = 1;`;
  const body = await openCardWithBody(page, 'Getting started', codeBody('ts', long));
  const numbers = block(body).locator('.doc-code-line');
  await expect(numbers).toHaveCount(2);
  const places = await block(body)
    .locator('pre > code')
    .evaluate((code) => {
      const markers = [...code.querySelectorAll('.doc-code-line')];
      const tops = markers.map((marker) => marker.getBoundingClientRect().top);
      // The first letter of a line: where its first row is.
      const text = code.textContent;
      const letterAt = (at: number): DOMRect => {
        const range = document.createRange();
        const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
        let seen = 0;
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          const length = node.nodeValue?.length ?? 0;
          if (seen + length > at) {
            range.setStart(node, at - seen);
            range.setEnd(node, at - seen + 1);
            break;
          }
          seen += length;
        }
        return range.getBoundingClientRect();
      };
      const rows = [letterAt(0).top, letterAt(text.indexOf('\n') + 1).top];
      const left = markers[0]?.getBoundingClientRect().right ?? 0;
      return {
        tops,
        rows,
        textLeft: letterAt(0).left,
        numberRight: left,
        unselectable: getComputedStyle(markers[0] ?? code).userSelect,
        hidden: markers.every((marker) => marker.getAttribute('aria-hidden') === 'true'),
      };
    });
  // The first line wraps over several rows; each number stands as high beside its line's first
  // row as the other, and left of the text.
  const [first = 0, second = 0] = places.tops;
  const [firstRow = 0, secondRow = 0] = places.rows;
  expect(secondRow - firstRow).toBeGreaterThan(40);
  expect(Math.abs(second - secondRow - (first - firstRow))).toBeLessThan(1);
  expect(places.numberRight).toBeLessThanOrEqual(places.textLeft);
  expect(places.unselectable).toBe('none');
  expect(places.hidden).toBe(true);
});

test('a block too long for a number at each line has them in one column, level with its lines', async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem('fvoci:code-lines', '1');
  });
  const lines = Array.from(
    { length: 400 },
    (_, line) => `let value${String(line)} = "${'x'.repeat(90)}";`,
  );
  const body = await openCardWithBody(page, 'Getting started', codeBody('ts', lines.join('\n')));
  const column = block(body).locator(':scope > .doc-code-column');
  await expect(column).toBeVisible();
  const rows = await block(body).evaluate((frame) => {
    const numbers = frame.querySelector<HTMLElement>(':scope > .doc-code-column');
    const pre = frame.querySelector<HTMLElement>('pre');
    const text = frame.querySelector('pre > code')?.firstChild;
    if (!numbers || !pre || !(text instanceof Text)) return null;
    const pitch = numbers.getBoundingClientRect().height / 400;
    const top = (line: number): number => {
      const range = document.createRange();
      const at = text.data.split('\n').slice(0, line).join('\n').length + (line > 0 ? 1 : 0);
      range.setStart(text, at);
      range.setEnd(text, at + 1);
      return range.getBoundingClientRect().top;
    };
    pre.scrollLeft = 200;
    return {
      lines: [1, 399].map((line) => Math.round(top(line) - top(0) - line * pitch)),
      scrolled: pre.scrollLeft > 0,
      covered: getComputedStyle(numbers).backgroundColor === getComputedStyle(pre).backgroundColor,
    };
  });
  // Each number is level with its line; the lines do not wrap, and scroll under the numbers.
  expect(rows).toEqual({ lines: [0, 0], scrolled: true, covered: true });
});

test('a card the reader may only look at names the language and still copies', async ({ page }) => {
  const engine = engineOf(page) ?? 'chromium';
  await page.context().grantPermissions(clipboardPermissions(engine));
  const body = await openCardWithBody(page, 'Study notes', codeBody('rust', 'fn main() {}'));
  await expect(body).toHaveAttribute('data-editor', 'reading');
  await coloured(body);
  const language = block(body).locator('button.doc-code-language');
  await expect(language).toBeDisabled();
  await expect(language).toHaveAccessibleName(/Rust/);
  await block(body).locator('button.doc-code-copy').click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('fn main() {}');
});

/**
 * Has somebody write the open card's body anew, and opens the version
 * before in the dialog of versions: the body as it was, only shown.
 */
async function openVersionBefore(page: Page, body: Locator): Promise<Locator> {
  const { boardId, cardId } = placeOf(page.url());
  await page.evaluate(
    async ([atBoard, atCard]) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const { updatedAt } = await dev.backend.card(atBoard, atCard);
      await dev.someoneElse().writeCardBody(atBoard, atCard, {
        body: {
          type: 'doc',
          content: [{ type: 'paragraph', content: [{ type: 'text', text: 'New' }] }],
        },
        readAt: updatedAt,
      });
    },
    [boardId, cardId] as const,
  );
  await expect(body).toContainText('New');
  await page.getByTestId('card-menu').click();
  await page.locator('[data-action="versions"]').click();
  // Newest first: the body as it is, then the one with the code, then the seed's.
  await page.getByTestId('body-versions').locator('[data-version]').nth(1).click();
  return page.getByTestId('body-version');
}

test('a version read in the dialog is coloured as the editor colours it', async ({ page }) => {
  const body = await openCardWithBody(page, 'Customer contacts', codeBody('ts', SOURCE));
  await coloured(body);
  const inEditor = await colours(body);
  const shown = await openVersionBefore(page, body);
  await coloured(shown);
  // Read-only: the name, small, and the copy button; nothing to change the language with.
  await expect(block(shown).locator('.doc-code-name')).toHaveText('TypeScript');
  await expect(block(shown).locator('button.doc-code-language')).toHaveCount(0);
  await expect(block(shown).locator('button.doc-code-copy')).toBeVisible();
  expect(await colours(shown)).toEqual(inEditor);
});

/** Where the first letter of a block's text is in the window. */
const firstLetter = (code: Locator): Promise<{ x: number; y: number }> =>
  code.evaluate((element) => {
    const text = document.createTreeWalker(element, NodeFilter.SHOW_TEXT).nextNode();
    if (!text) throw new Error('the block has no text');
    const range = document.createRange();
    range.setStart(text, 0);
    range.setEnd(text, 1);
    const { x, y } = range.getBoundingClientRect();
    return { x, y };
  });

test('a block of a thousand lines, numbered and only shown, keeps its text where it is when its frame comes', async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem('fvoci:code-lines', '1');
  });
  // What frames the code blocks of a body that is only shown comes late, as on a slow network.
  let release = (): void => undefined;
  const late = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(/\/code-view-[^/]*\.js$/, async (route) => {
    await late;
    await route.continue();
  });
  // Past the length that is coloured (CODE_HIGHLIGHT_LIMIT): only the frame and the numbers come.
  const source = Array.from({ length: 1_200 }, (_, index) => `line ${index + 1}`).join('\n');
  const body = await openCardWithBody(page, 'Customer contacts', codeBody('ts', source));
  const shown = await openVersionBefore(page, body);
  const code = shown.locator('pre > code').first();
  await expect(code).toContainText('line 1200');
  await expect(shown.locator('.doc-code')).toHaveCount(0);
  const before = await firstLetter(code);
  release();
  await expect(block(shown).locator('.doc-code-line')).toHaveCount(1_200);
  // Four digits from the first drawing: the room of the numbers does not grow when they come.
  expect(await firstLetter(code)).toEqual(before);
});

test('a syllable composed in a code block is kept, and coloured once it is done', async ({
  page,
}) => {
  test.skip(engineOf(page) !== 'chromium', "an input method is driven through Chromium's protocol");
  const body = await openCardWithBody(page, 'Getting started', codeBody('ts', 'const a = 1;'));
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await coloured(body);
  const code = block(body).locator('pre > code');
  await code.click();
  await page.keyboard.press('End');
  await page.keyboard.type(' // ');
  const ime = await page.context().newCDPSession(page);
  for (const text of ['ㅎ', '하', '한', '한ㄱ', '한그', '한글']) {
    await ime.send('Input.imeSetComposition', {
      text,
      selectionStart: text.length,
      selectionEnd: text.length,
    });
  }
  await ime.send('Input.insertText', { text: '한글' });
  await expect(code).toHaveText('const a = 1; // 한글');
  await expect(code.locator('.doc-code-comment')).toHaveText('// 한글', { timeout: 30_000 });
  await expect(code.locator('.doc-code-keyword').first()).toHaveText('const');
});

test('two readers write in one block, and each sees the other’s words coloured', async ({
  page,
}) => {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title.startsWith('Customer contacts'));
    const views = await backend.views(board?.id ?? '');
    const cards = await backend.cards(board?.id ?? '');
    const [card] = [...cards].sort((one, other) => one.title.localeCompare(other.title));
    return { boardId: board?.id ?? '', viewId: views[0]?.id ?? '', cardId: card?.id ?? '' };
  });
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  const mine = page.getByTestId('card-body');
  await expect(mine).toHaveAttribute('data-editor', 'ready');
  const other = await page.context().newPage();
  await other.goto(boardPath(place.boardId, place.viewId, place.cardId));
  const theirs = other.getByTestId('card-body');
  await expect(theirs).toHaveAttribute('data-editor', 'ready');
  // The first writes a code block, in place of the body, by the shortcut of three back quotes.
  await mine.locator('[contenteditable="true"]').first().selectText();
  await page.keyboard.press('Delete');
  await page.keyboard.type('```ts ');
  await page.keyboard.type('let mine = 1;');
  await coloured(mine);
  await coloured(theirs);
  await expect(block(theirs).locator('pre > code')).toHaveText('let mine = 1;');
  // The other writes a line under it. The editor learns of the caret's move
  // by End from the page a frame later, which a loaded machine may give after
  // the next key: Enter waits for it, as a hand would.
  await block(theirs).locator('pre > code').click();
  await other.keyboard.press('End');
  await expect.poll(() => caretInText(theirs)).toBe('let mine = 1;'.length);
  await other.keyboard.press('Enter');
  await other.keyboard.type('const theirs = "two";');
  for (const body of [mine, theirs]) {
    await expect(block(body).locator('pre > code')).toHaveText(
      'let mine = 1;\nconst theirs = "two";',
    );
    // As a list: while the colours of the line before the last key are still on it, the
    // string is in two pieces, which is not yet the answer and not a failure either.
    await expect(block(body).locator('.doc-code-string')).toHaveText(['"two"'], {
      timeout: 30_000,
    });
  }
  await other.close();
});

test('a block past the length that is coloured stays plain, and the page goes on', async ({
  page,
}) => {
  // Code that would be coloured, longer than the highlighter takes (CODE_HIGHLIGHT_LIMIT):
  // it is never asked for. One that takes the worker too long is given up (code-highlight.test.ts).
  const body = await openCardWithBody(
    page,
    'Getting started',
    codeBody('ts', 'const a = 1; '.repeat(1_000)),
  );
  // First the written body, then its editor: the editor is ready before that body comes,
  // on the one the card had, and a press then lands in a document that is replaced under
  // it, and the keys are lost.
  await expect(body.locator('p').first()).toHaveText('Before the code.');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  await body.locator('p').first().click();
  await page.keyboard.press('End');
  const started = Date.now();
  await page.keyboard.type(' typed');
  await expect(body.locator('p').first()).toHaveText('Before the code. typed');
  expect(Date.now() - started).toBeLessThan(5_000);
  // No colour comes, and the page still answers.
  await page.waitForTimeout(1_500);
  await expect(block(body).locator('pre span:not(.doc-code-line)')).toHaveCount(0);
  await expect(page.getByTestId('card-title')).toBeVisible();
});

// ------------------------------------------------------------ the pictures

test.describe('pictures of code blocks', () => {
  test.skip(!process.env.FVOCI_SCREENS, 'pictures are taken only when asked for');

  test('the editor and a version, wide and narrow, light and dark', async ({ page }, info) => {
    test.skip(info.project.name !== 'desktop', 'one project takes every width');
    const dir = screensDir('code-blocks');
    await page.addInitScript(() => {
      localStorage.setItem('fvoci:code-lines', '1');
    });
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: 900 });
      for (const theme of ['light', 'dark'] as const) {
        const body = await openCardWithBody(page, 'Getting started', codeBody('ts', SOURCE));
        // The theme is the account's, and stays from one picture to the next: each sets its own.
        await page.evaluate(
          (chosen) =>
            (window as unknown as { fvociDev: Dev }).fvociDev.backend.patchSettings({
              theme: chosen,
            }),
          theme,
        );
        if (theme === 'dark') await expect(page.locator('html')).toHaveClass(/dark/);
        else await expect(page.locator('html')).not.toHaveClass(/dark/);
        await coloured(body);
        const name = `${width}-${theme}`;
        await block(body).screenshot({ path: path.join(dir, `editor-${name}.png`) });
        await block(body).locator('button.doc-code-language').click();
        await block(body).getByRole('combobox').fill('s');
        await page.screenshot({ path: path.join(dir, `picker-${name}.png`) });
        await page.keyboard.press('Escape');
        await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
        await block(body).locator('button.doc-code-copy').click();
        await expect(block(body).getByRole('status')).not.toBeEmpty();
        await block(body).screenshot({ path: path.join(dir, `copied-${name}.png`) });
        // The same block only shown, in the dialog of versions.
        const shown = await openVersionBefore(page, body);
        await coloured(shown);
        await block(shown).screenshot({ path: path.join(dir, `version-${name}.png`) });
      }
    }
  });
});
