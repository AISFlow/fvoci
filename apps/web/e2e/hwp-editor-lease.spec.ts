/**
 * One editor of a file at a time: while somebody else has a Hangul document
 * open in the editor, the viewer says who and offers no "Edit", until their
 * lease is let go of or ends; the editor holds the lease while it is open and
 * lets it go when it closes, and when nobody puts anything into it for 30
 * minutes, saying so; a lease lost while editing is told to the writer; and
 * the page asks before it goes with changes that are not saved.
 *
 * With `FVOCI_SCREENS=1` the editor that let the lease go is pictured into
 * `target/viewer-h3/screens/`.
 *
 * Somebody else is the memory backend's other person (`fvociDev.someoneElse`),
 * an editor of the board.
 */
import { expect, test, type Page } from '@playwright/test';

import { koreanHwpx } from '../tests/support/hwp-files.ts';

import { attach, openCard, picture, viewer } from './file-viewer.support.ts';
import { writtenElsewhere } from './hwp-editor.support.ts';
import { engineOf } from './keyboard.support.ts';

const HWPX_TYPE = 'application/hwp+zip';
/** The lease's term (the server's 90 seconds), and a little more. */
const PAST_THE_TERM = 91_000;
const MINUTE = 60_000;
/** A turn of the editor's holding on to its lease (30 seconds), and a little more. */
const A_TURN = 31_000;

interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  me(): Promise<{ id: string }>;
  addMember(boardId: string, userId: string, role: string): Promise<unknown>;
  attachments(boardId: string, cardId: string): Promise<{ name: string; fileId: string }[]>;
  fileVersions: {
    holdLease(boardId: string, fileId: string, leaseId: string): Promise<{ leaseId: string }>;
    releaseLease(boardId: string, fileId: string, leaseId: string): Promise<void>;
  };
}

interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

/**
 * Somebody else, made an editor of the board, takes the lease of the attached
 * file `name` (or lets theirs go): the lease, or the status it was refused with.
 */
async function theirLease(page: Page, name: string, letGo = ''): Promise<string> {
  return page.evaluate(
    async ({ name, letGo }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const [workspace] = await dev.backend.workspaces();
      const boards = await dev.backend.boards(workspace?.id ?? '');
      const board = boards.find((each) => each.title.startsWith('Project schedule'));
      const cards = await dev.backend.cards(board?.id ?? '');
      const card = cards.find((each) => each.title === 'Design the screens');
      if (!board || !card) throw new Error('no card');
      const other = dev.someoneElse();
      await dev.backend.addMember(board.id, (await other.me()).id, 'editor');
      const attached = (await dev.backend.attachments(board.id, card.id)).find(
        (each) => each.name === name,
      );
      if (!attached) throw new Error('the file is not attached');
      if (letGo) {
        await other.fileVersions.releaseLease(board.id, attached.fileId, letGo);
        return '';
      }
      try {
        return (await other.fileVersions.holdLease(board.id, attached.fileId, '')).leaseId;
      } catch (error) {
        return `refused ${String((error as { status?: number }).status)}`;
      }
    },
    { name, letGo },
  );
}

const editing = (page: Page) => page.getByTestId('file-viewer-editing');
const editButton = (page: Page) => page.getByTestId('file-viewer-edit');
const editor = (page: Page) => page.getByTestId('file-editor');

/** Attaches a document and shows it in the viewer. */
async function shown(page: Page, name: string): Promise<void> {
  const row = await attach(page, name, koreanHwpx(), HWPX_TYPE);
  await row.getByRole('link', { name }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
}

test('while somebody else edits a file the viewer says who and offers no Edit, until their lease ends', async ({
  page,
}) => {
  await page.clock.install();
  await openCard(page);
  const name = '편집 중.hwpx';
  const row = await attach(page, name, koreanHwpx(), HWPX_TYPE);
  expect(await theirLease(page, name)).not.toMatch(/^refused/);

  await row.getByRole('link', { name }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(editing(page)).toHaveAttribute('data-editing', 'true');
  await expect(editing(page)).toHaveText(/^\S.* is editing this file\.$/);
  await expect(editing(page)).not.toContainText('Someone else');
  // "Edit" is there, unavailable, and says why.
  await expect(editButton(page)).toBeDisabled();
  const why = await editing(page).getAttribute('id');
  await expect(editButton(page)).toHaveAttribute('aria-describedby', why ?? '');
  await picture(page, 'viewer-h2', 'lease-editing');

  // Their window died: nobody held on to the lease, which ended with its term.
  await page.clock.fastForward(PAST_THE_TERM);
  await expect(editing(page)).toHaveAttribute('data-editing', 'false');
  await expect(editing(page)).toHaveText('');
  await expect(editButton(page)).toBeEnabled();
  await expect(editButton(page)).not.toHaveAttribute('aria-describedby');
  await editButton(page).click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  expect(await theirLease(page, name)).toBe('refused 409');
});

test('the editor holds the file’s lease while it is open and lets it go when it closes', async ({
  page,
}) => {
  await openCard(page);
  const name = '내 편집.hwpx';
  await shown(page, name);
  await expect(editing(page)).toHaveAttribute('data-editing', 'false');
  await editButton(page).click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  expect(await theirLease(page, name)).toBe('refused 409');

  await page.getByTestId('file-editor-close').click();
  await expect(editor(page)).toHaveCount(0);
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  // The viewer asks once the lease went, and finds nobody editing.
  await expect(editing(page)).toHaveAttribute('data-editing', 'false');
  await expect(editButton(page)).toBeEnabled();
  const theirs = await theirLease(page, name);
  expect(theirs).not.toMatch(/^refused/);
  await theirLease(page, name, theirs);
});

test('Edit, where somebody else took the lease since the viewer asked, opens nothing and says who', async ({
  page,
}) => {
  await openCard(page);
  const name = '먼저 온.hwpx';
  await shown(page, name);
  await expect(editing(page)).toHaveAttribute('data-editing', 'false');
  expect(await theirLease(page, name)).not.toMatch(/^refused/);

  // By the keyboard: the focus stays on "Edit", which says why it is unavailable.
  await editButton(page).focus();
  await page.keyboard.press('Enter');
  await expect(editing(page)).toHaveText(/ is editing this file\.$/);
  await expect(editButton(page)).toBeDisabled();
  await expect(editButton(page)).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(editor(page)).toHaveCount(0);
});

test('a lease lost while editing is told to the writer, who goes on', async ({ page }) => {
  await page.clock.install();
  await openCard(page);
  const name = '잠든 창.hwpx';
  await shown(page, name);
  await editButton(page).click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  await expect(editor(page)).toHaveAttribute('data-lease', 'held');

  // The window slept past the lease's term, and somebody else took the file's meanwhile.
  await page.clock.setSystemTime(Date.now() + PAST_THE_TERM * 2);
  expect(await theirLease(page, name)).not.toMatch(/^refused/);
  await page.clock.runFor(31_000);
  await expect(editor(page)).toHaveAttribute('data-lease', 'lost');
  await expect(page.getByTestId('file-editor-said')).toHaveText(
    'Someone else started editing this file. A save still never writes over a newer version.',
  );
  await expect(page.getByTestId('file-editor-save')).toBeEnabled();
});

test('an editor nobody puts anything into for 30 minutes lets the file go, says so first, and takes it back at the next input', async ({
  page,
}) => {
  await page.clock.install();
  await openCard(page);
  const name = '쉬는 창.hwpx';
  await shown(page, name);
  await editButton(page).click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  await expect(page.getByTestId('file-editor-frame')).toBeFocused();
  await page.keyboard.type('NOT SAVED');
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
  const said = page.getByTestId('file-editor-said');

  await page.clock.runFor(25 * MINUTE + A_TURN);
  await expect(said).toHaveText(
    'If nothing is typed or clicked within 5 min, editing is let go so that someone else may edit this file.',
  );
  await expect(editor(page)).toHaveAttribute('data-lease', 'held');
  await page.clock.runFor(5 * MINUTE);
  await expect(editor(page)).toHaveAttribute('data-lease', 'resting');
  await expect(said).toHaveText(
    'Editing was let go after 30 minutes without input: someone else may edit this file now. Your unsaved changes stay; type or click to edit again.',
  );
  // What was not saved is there still.
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
  await picture(page, 'viewer-h3/screens', 'lease-resting');
  // Somebody else may take it, and lets it go again.
  const theirs = await theirLease(page, name);
  expect(theirs).not.toMatch(/^refused/);
  await theirLease(page, name, theirs);

  // The wheel turned over the document, or, in WebKit for the iPhone and iPad, which has no
  // wheel, a finger put on it: the editor's once more, and nobody else's. Chromium's touch
  // screens keep the wheel, so that it is not left to the desktop alone.
  const frame = await page.getByTestId('file-editor-frame').boundingBox();
  if (!frame) throw new Error('no frame');
  const [x, y] = [frame.x + frame.width / 2, frame.y + frame.height / 2];
  if (engineOf(page) === 'webkit' && test.info().project.use.hasTouch) {
    await page.touchscreen.tap(x, y);
  } else {
    await page.mouse.move(x, y);
    await page.mouse.wheel(0, 120);
  }
  await expect(editor(page)).toHaveAttribute('data-lease', 'held');
  await expect(said).toHaveText('There are unsaved changes');
  expect(await theirLease(page, name)).toBe('refused 409');
});

test('where somebody else took the file and wrote a version while the editor let it go, the writer is told and nothing is saved over', async ({
  page,
}) => {
  await page.clock.install();
  await openCard(page);
  const name = '쉬는 동안.hwpx';
  await shown(page, name);
  await editButton(page).click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  await page.keyboard.type('MINE');
  await page.clock.runFor(30 * MINUTE + A_TURN);
  await expect(editor(page)).toHaveAttribute('data-lease', 'resting');

  // Somebody else edits it meanwhile, writes a version, and is at it still.
  const theirs = await theirLease(page, name);
  expect(theirs).not.toMatch(/^refused/);
  await writtenElsewhere(page, name, koreanHwpx());
  // A press on the app's own header is input too.
  await page.getByTestId('file-editor-said').click();
  await expect(editor(page)).toHaveAttribute('data-lease', 'lost');
  await expect(page.getByTestId('file-editor-said')).toHaveText(
    'Someone else started editing this file. A save still never writes over a newer version.',
  );
  // They let it go: it is the editor's again at its next turn.
  await theirLease(page, name, theirs);
  await page.clock.runFor(A_TURN);
  await expect(editor(page)).toHaveAttribute('data-lease', 'held');

  // Their version is not saved over: the edit may be kept as a file of its own.
  await page.getByTestId('file-editor-save').click();
  const stale = page.getByTestId('file-editor-stale');
  await expect(stale).toContainText('The file got a newer version meanwhile');
  await stale.getByRole('button', { name: 'Cancel' }).click();
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
});

test('the page asks before it goes with changes that are not saved', async ({ page }) => {
  await openCard(page);
  const name = '저장 전.hwpx';
  await shown(page, name);
  await editButton(page).click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  await expect(page.getByTestId('file-editor-frame')).toBeFocused();
  await page.keyboard.type('NOT SAVED');
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');

  const asked = page.waitForEvent('dialog');
  await page.close({ runBeforeUnload: true });
  const dialog = await asked;
  expect(dialog.type()).toBe('beforeunload');
  await dialog.dismiss();
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
});
