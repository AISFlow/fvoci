/**
 * A file block opens the file viewer, on the backend in memory: a PDF put
 * into a wiki's page or a card's body opens when its name is pressed (by
 * the mouse, a finger, or Enter and Space once Tab is on it), or with Enter
 * once the block is selected, and closes again; over a card the card stays
 * open. A deck and a Hangul document (HWPX, HWP) open from theirs too. In
 * the editor the block is still selected, dragged and deleted as before,
 * and the keys pressed on its name edit nothing; a file the viewer would
 * not show offers its download alone, and so does one whose viewer cannot
 * be fetched.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { HWP_TEXT, koreanHwp, koreanHwpx } from '../tests/support/hwp-files.ts';
import { koreanPdf } from '../tests/support/viewer-files.ts';
import { koreanPptx, PPTX_TEXT } from '../tests/support/viewer-pptx.ts';

import { boardPath } from './address.ts';
import { picture } from './file-viewer.support.ts';
import { openApp } from './shell.support.ts';
import { nameThePage, newWiki, pageBody } from './wiki.support.ts';

/** Where the pictures go with `FVOCI_SCREENS=1`: `target/viewer-p2/`. */
const SCREENS = 'viewer-p2';

const NAME = '회의록 2026-11.pdf';

interface Dev {
  backend: {
    workspaces(): Promise<{ id: string }[]>;
    boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
    views(boardId: string): Promise<{ id: string }[]>;
    cards(boardId: string): Promise<{ id: string; title: string }[]>;
  };
}

const viewer = (page: Page): Locator => page.getByTestId('file-viewer');

/** A new wiki's first page, named, its body ready to be written. */
async function newPage(page: Page): Promise<Locator> {
  await openApp(page);
  await newWiki(page, 'Committee');
  await page.getByTestId('wiki-first-page').click();
  await nameThePage(page, 'Minutes');
  await expect(page.getByTestId('page-body')).toHaveAttribute('data-editor', 'ready');
  return pageBody(page);
}

/** A card of the board of tasks, its body ready to be written. */
async function cardBody(page: Page): Promise<Locator> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title.startsWith('Project schedule'));
    if (!board) throw new Error('no board of tasks');
    const [view] = await backend.views(board.id);
    const card = (await backend.cards(board.id)).find(
      (each) => each.title === 'Design the screens',
    );
    return { boardId: board.id, viewId: view?.id ?? '', cardId: card?.id ?? '' };
  });
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  const body = page.getByTestId('card-body');
  await expect(body).toHaveAttribute('data-editor', 'ready');
  return body.locator('[contenteditable="true"]').first();
}

/** Writes a line, then puts a file under it through the slash menu; gives its block. */
async function putFile(page: Page, body: Locator, name: string, bytes: Buffer, type: string) {
  await body.click();
  await page.keyboard.press('ControlOrMeta+End');
  await page.keyboard.press('Enter');
  await page.keyboard.type('Above:');
  await page.keyboard.press('Enter');
  await page.keyboard.type('/file');
  await expect(page.locator('[data-suggestions="slash"]').getByRole('option')).toHaveText([
    'File or image',
  ]);
  const [chooser] = await Promise.all([
    page.waitForEvent('filechooser'),
    page.keyboard.press('Enter'),
  ]);
  await chooser.setFiles({ name, mimeType: type, buffer: bytes });
  const block = body.locator('[data-file-block]', { hasText: name });
  await expect(block).toHaveAttribute('data-state', 'shown');
  return block;
}

/**
 * Selects the block with an arrow from the end of the line above it: the arrow right, which the
 * editor reads from where the caret is (the arrow down measures the page, still settling). Right
 * after the file is put in, a click may come while the page is still drawn (on a loaded machine,
 * the block was left unselected): the caret is put again until it is seen at the end of the line,
 * and the arrow is pressed once.
 */
async function selectByArrow(page: Page, body: Locator, block: Locator): Promise<void> {
  const atEnd = (): Promise<boolean> =>
    page.evaluate(() => {
      const selection = document.getSelection();
      const line = selection?.anchorNode?.parentElement?.closest('p');
      if (!selection?.anchorNode || line?.textContent !== 'Above:') return false;
      const rest = document.createRange();
      rest.setStart(selection.anchorNode, selection.anchorOffset);
      rest.setEnd(line, line.childNodes.length);
      return rest.toString() === '';
    });
  await expect(async () => {
    await body.getByText('Above:').last().click();
    await page.keyboard.press('End');
    expect(await atEnd()).toBe(true);
  }).toPass({ timeout: 5000 });
  await page.keyboard.press('ArrowRight');
  await expect(block).toHaveClass(/ProseMirror-selectednode/);
}

test('a PDF in a page opens in the viewer by its name and by Enter, and the block is deleted as before', async ({
  page,
  hasTouch,
}) => {
  const body = await newPage(page);
  const block = await putFile(page, body, NAME, koreanPdf(2), 'application/pdf');
  const open = block.getByRole('button', { name: `View ${NAME}` });
  await expect(open).toHaveText(NAME);
  // The download stays a link of its own.
  await expect(block.getByRole('link', { name: `Download ${NAME}` })).toBeVisible();
  await picture(page, SCREENS, 'file-block');

  // Pressed by a finger where the screen is touched, by the mouse elsewhere.
  if (hasTouch) await open.tap();
  else await open.click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(viewer(page).getByRole('heading', { name: NAME })).toBeVisible();
  await expect(page.getByTestId('file-viewer-page')).toContainText('1 / 2');
  await picture(page, SCREENS, 'pdf-file-block');
  await page.getByTestId('file-viewer-close').click();
  await expect(viewer(page)).toHaveCount(0);

  // Reached with Tab from the line above, the name is a button: Enter and Space press it, and
  // the editor makes nothing of them (no line split, no block typed over).
  const text = await body.innerText();
  await body.getByText('Above:').last().click();
  await page.keyboard.press('End');
  await page.keyboard.press('Tab');
  await expect(open).toBeFocused();
  for (const key of ['Enter', 'Space']) {
    await page.keyboard.press(key);
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    await page.keyboard.press('Escape');
    await expect(viewer(page)).toHaveCount(0);
    // Closed, the focus is back on the name it was opened from.
    await expect(open).toBeFocused();
  }
  // Nor the browser's shortcuts that edit, nor text put in (WebKit would apply them where the
  // caret was).
  for (const key of ['Backspace', 'ControlOrMeta+X', 'ControlOrMeta+V', 'ControlOrMeta+B']) {
    await page.keyboard.press(key);
  }
  await page.keyboard.insertText('가');
  // So on the download, the next stop.
  await page.keyboard.press('Tab');
  await expect(block.getByRole('link', { name: `Download ${NAME}` })).toBeFocused();
  await page.keyboard.press('Backspace');
  await page.keyboard.insertText('나');
  await expect.poll(() => body.innerText()).toBe(text);
  await expect(block).toHaveCount(1);
  // Back in the text with Shift+Tab, the caret is where it was: at the end of the line above.
  await page.keyboard.press('Shift+Tab');
  await page.keyboard.press('Shift+Tab');
  await expect(body).toBeFocused();
  await page.keyboard.type('!');
  await expect(body.getByText('Above:!')).toHaveCount(1);
  await page.keyboard.press('Backspace');
  // Pressed by an assistive technology (a click of no pointer, the focus left in the text), the
  // name has the focus back when the viewer closes, not what had it when the viewer opened.
  await open.dispatchEvent('click');
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await expect(open).toBeFocused();

  // By the keyboard: selected with an arrow, to the name and back with Tab and Shift+Tab, still
  // selected; opened with Enter, closed with Escape.
  await selectByArrow(page, body, block);
  await page.keyboard.press('Tab');
  await expect(open).toBeFocused();
  await page.keyboard.press('Shift+Tab');
  await expect(body).toBeFocused();
  await expect(block).toHaveClass(/ProseMirror-selectednode/);
  await page.keyboard.press('Enter');
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  // Back in the editor, the block still selected: Delete deletes it, and nothing else.
  await expect(body).toBeFocused();
  await expect(block).toHaveClass(/ProseMirror-selectednode/);
  await page.keyboard.press('Delete');
  await expect(body.locator('[data-file-block]')).toHaveCount(0);
  await expect(body).toContainText('Above:');
});

test('a PDF in a card opens over the card, which stays open when the viewer closes', async ({
  page,
}) => {
  const body = await cardBody(page);
  const block = await putFile(page, body, NAME, koreanPdf(1), 'application/pdf');
  const card = page.getByTestId('card-dialog');
  await block.getByRole('button', { name: `View ${NAME}` }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  // A press in the document is the viewer's, not a press outside the card.
  await page.frameLocator('[data-testid="file-viewer-frame"]').locator('.page').first().click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await expect(card).toBeVisible();
  await expect(block).toBeVisible();

  // Pressed with the mouse, the block was selected too: Backspace deletes it once the viewer is closed.
  await block.getByRole('button', { name: `View ${NAME}` }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await page.getByTestId('file-viewer-close').click();
  await expect(block).toHaveClass(/ProseMirror-selectednode/);
  await page.keyboard.press('Backspace');
  await expect(body.locator('[data-file-block]')).toHaveCount(0);
  await expect(card).toBeVisible();
});

test('a finger on the name in a body not being written opens the file, and the body takes no focus', async ({
  page,
  hasTouch,
}) => {
  test.skip(!hasTouch, 'a finger needs a touch screen');
  const body = await cardBody(page);
  const block = await putFile(page, body, NAME, koreanPdf(1), 'application/pdf');
  // Move the selection off the newly inserted file block (the arrow puts the caret at the end of the
  // line above it, a finger's screen or not), see that it is off, then take it out of the body as
  // when the card was only opened.
  await page.keyboard.press('ArrowLeft');
  await expect(block).not.toHaveClass(/ProseMirror-selectednode/);
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    document.getSelection()?.removeAllRanges();
  });
  const where = (): Promise<{ focused: boolean; selected: boolean }> =>
    body.evaluate((root) => {
      const selection = document.getSelection();
      const anchor = selection && selection.rangeCount > 0 ? selection.anchorNode : null;
      return {
        focused: document.activeElement === root,
        selected: anchor !== null && root.contains(anchor),
      };
    });
  const name = block.getByRole('button', { name: `View ${NAME}` });
  await name.scrollIntoViewIfNeeded();
  expect(await where()).toEqual({ focused: false, selected: false });

  await name.tap();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  expect(await where()).toEqual({ focused: false, selected: false });
  await page.getByTestId('file-viewer-close').tap();
  await expect(viewer(page)).toHaveCount(0);
  // The focus is back on the name, not in the body: no keyboard comes up.
  await expect(name).toBeFocused();
  expect(await where()).toEqual({ focused: false, selected: false });
  await expect(block).not.toHaveClass(/ProseMirror-selectednode/);
});

/** The other kinds a file block opens, each with what its first page is called and shows. */
const OTHERS = [
  {
    name: '분기 보고.pptx',
    bytes: koreanPptx,
    type: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
    first: 'Slide 1 of 3',
    page: '.slide[data-page="1"]',
    text: PPTX_TEXT.title,
  },
  {
    name: '회의록.hwpx',
    bytes: koreanHwpx,
    type: 'application/hwp+zip',
    first: 'Page 1 of 2',
    page: '.page[data-page="1"] .hwp-text',
    text: HWP_TEXT.title,
  },
  {
    name: '회의록.hwp',
    bytes: koreanHwp,
    type: 'application/x-hwp',
    first: 'Page 1 of 2',
    page: '.page[data-page="1"] .hwp-text',
    text: HWP_TEXT.title,
  },
];

for (const other of OTHERS) {
  test(`${other.name} in a page opens in the viewer by its name`, async ({ page }) => {
    const body = await newPage(page);
    const block = await putFile(page, body, other.name, other.bytes(), other.type);
    await block.getByRole('button', { name: `View ${other.name}` }).click();
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    await expect(page.getByTestId('file-viewer-announce')).toHaveText(other.first);
    const frame = page.frameLocator('[data-testid="file-viewer-frame"]');
    await expect(frame.locator(other.page)).toContainText(other.text);
    await page.keyboard.press('Escape');
    await expect(viewer(page)).toHaveCount(0);
  });
}

test('a block dragged by its name moves, and the viewer does not open', async ({ page }) => {
  const body = await newPage(page);
  await body.click();
  await page.keyboard.type('First line');
  const block = await putFile(page, body, NAME, koreanPdf(1), 'application/pdf');
  const open = block.getByRole('button', { name: `View ${NAME}` });
  // The viewer opens on a press of the name: counted, as a drag is no press.
  await page.evaluate(() => {
    const counted = window as unknown as { pressed: number };
    counted.pressed = 0;
    document.addEventListener(
      'click',
      (event) => {
        if (event.target instanceof Element && event.target.closest('.doc-file-open')) {
          counted.pressed += 1;
        }
      },
      true,
    );
  });
  await open.dragTo(body.getByText('First line'), { targetPosition: { x: 2, y: 2 } });
  await expect(body.locator('> *').first()).toHaveAttribute('data-file-block', '');
  expect(await page.evaluate(() => (window as unknown as { pressed: number }).pressed)).toBe(0);
  await expect(viewer(page)).toHaveCount(0);
});

test('a file block whose viewer cannot be fetched downloads its file instead', async ({
  page,
  browserName,
}) => {
  await page.route(/\/static\/FileViewer-[0-9a-f]+\.js$/, (route) => route.abort());
  const body = await newPage(page);
  const block = await putFile(page, body, NAME, koreanPdf(1), 'application/pdf');
  const [saved] = await Promise.all([
    page.waitForEvent('download'),
    block.getByRole('button', { name: `View ${NAME}` }).click(),
  ]);
  // WebKit saves a name's spaces as `_`.
  expect(saved.suggestedFilename()).toBe(
    browserName === 'webkit' ? NAME.replaceAll(' ', '_') : NAME,
  );
  await expect(viewer(page)).toHaveCount(0);
});

test('a file the viewer does not show offers its download alone, by its link and by Enter', async ({
  page,
}) => {
  const body = await newPage(page);
  const block = await putFile(page, body, 'notes.zip', Buffer.from('PK notes'), 'application/zip');
  await expect(block.getByRole('button')).toHaveCount(0);
  await expect(block.locator('.doc-file-name')).toHaveText('notes.zip');
  await selectByArrow(page, body, block);
  const [saved] = await Promise.all([page.waitForEvent('download'), page.keyboard.press('Enter')]);
  expect(saved.suggestedFilename()).toBe('notes.zip');
  await expect(viewer(page)).toHaveCount(0);
  await expect(block).toHaveCount(1);
});
