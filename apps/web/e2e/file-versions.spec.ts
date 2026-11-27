/**
 * The versions of a file, in the viewer opened from a card's attachments:
 * their list, a new version uploaded, an older one looked at, downloaded and
 * gone back to; and a file that did not become a version, because somebody
 * else wrote one meanwhile, saved under another name.
 *
 * With `FVOCI_SCREENS=1` the tests also take pictures of the list into
 * `target/viewer-h2/` (light and dark), at every size the project runs at
 * (the phone at 390 pixels).
 */
import path from 'node:path';

import { expect, test, type Locator, type Page } from '@playwright/test';

import { humanFileSize } from '../src/lib/logic/format.ts';
import { koreanPdf } from '../tests/support/viewer-files.ts';

import { boardPath } from './address.ts';

const SCREENS = process.env.FVOCI_SCREENS === '1';
const SCREENS_DIR = path.resolve(import.meta.dirname, '../../../target/viewer-h2');
const PDF = 'application/pdf';

interface Uploaded {
  fileId: string;
}

interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  views(boardId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  me(): Promise<{ id: string }>;
  addMember(boardId: string, userId: string, role: string): Promise<unknown>;
  attachments(boardId: string, cardId: string): Promise<{ name: string; fileId: string }[]>;
  upload(boardId: string, file: File): Promise<Uploaded>;
  fileVersions: {
    add(boardId: string, fileId: string, baseVersion: number, upload: Uploaded): Promise<unknown>;
  };
}

interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

interface Place {
  boardId: string;
  cardId: string;
}

/** Opens the card everything is tried on, by its address. */
async function openCard(page: Page): Promise<Place> {
  await page.goto('/');
  await expect(page.getByTestId('board-page')).toBeVisible();
  const place = await page.evaluate(async () => {
    const { backend } = (window as unknown as { fvociDev: Dev }).fvociDev;
    const [workspace] = await backend.workspaces();
    const boards = await backend.boards(workspace?.id ?? '');
    const board = boards.find((each) => each.title.startsWith('Project schedule'));
    if (!board) throw new Error('no board of tasks');
    const views = await backend.views(board.id);
    const card = (await backend.cards(board.id)).find(
      (each) => each.title === 'Design the screens',
    );
    if (!card) throw new Error('no card');
    const first = [...views].sort((a, b) => a.title.localeCompare(b.title))[0];
    return { boardId: board.id, viewId: first?.id ?? '', cardId: card.id };
  });
  await page.goto(boardPath(place.boardId, place.viewId, place.cardId));
  await expect(page.getByTestId('card-attachments')).toBeVisible();
  return place;
}

/** Attaches a PDF of `pages` pages and opens it in the viewer. */
async function viewAttached(page: Page, name: string, pages: number): Promise<Locator> {
  await page
    .getByTestId('attach-input')
    .setInputFiles({ name, mimeType: PDF, buffer: koreanPdf(pages) });
  const row = page.getByTestId('card-attachments').locator('[data-attachment]', { hasText: name });
  await row.getByRole('link', { name }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  return row;
}

const viewer = (page: Page): Locator => page.getByTestId('file-viewer');
const counter = (page: Page): Locator => page.getByTestId('file-viewer-page');
const versionRow = (page: Page, version: number): Locator =>
  page.getByTestId('file-versions-list').locator(`li[data-version="${String(version)}"]`);

/** The list of versions, opened if it is not: narrow, it closes when a version is shown. */
async function versions(page: Page): Promise<Locator> {
  const list = page.getByTestId('file-versions');
  if (!(await list.isVisible())) await page.getByTestId('file-viewer-versions').click();
  await expect(page.getByTestId('file-versions-list')).toBeVisible();
  return list;
}

async function uploadVersion(page: Page, name: string, pages: number): Promise<void> {
  await versions(page);
  await page
    .getByTestId('file-version-input')
    .setInputFiles({ name, mimeType: PDF, buffer: koreanPdf(pages) });
}

async function picture(page: Page, name: string): Promise<void> {
  if (!SCREENS) return;
  const project = test.info().project.name;
  // The phone pictures at the narrowest width the app is drawn for.
  if (project === 'mobile') await page.setViewportSize({ width: 390, height: 844 });
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    // The app has turned (its root's class) and nothing on it is still fading.
    await page.waitForFunction(
      (dark) =>
        document.documentElement.classList.contains('dark') === dark &&
        document.getAnimations().every((animation) => animation.playState !== 'running'),
      theme === 'dark',
    );
    await page.screenshot({ path: path.join(SCREENS_DIR, `${name}-${project}-${theme}.png`) });
  }
  await page.emulateMedia({ colorScheme: null });
}

test('the versions of a file are listed, written, looked at, downloaded and gone back to', async ({
  page,
}) => {
  await openCard(page);
  const row = await viewAttached(page, '보고서.pdf', 1);
  await expect(counter(page)).toContainText('1 / 1');
  const firstSize = humanFileSize(koreanPdf(1).length);
  const nextSize = humanFileSize(koreanPdf(3).length);
  expect(nextSize).not.toBe(firstSize);
  await expect(row).toContainText(firstSize);

  // The list opens from the header, with the focus on its heading; the file has one version.
  const button = page.getByTestId('file-viewer-versions');
  await expect(button).toHaveAttribute('aria-expanded', 'false');
  await button.click();
  await expect(button).toHaveAttribute('aria-expanded', 'true');
  const list = page.getByTestId('file-versions');
  await expect(list.getByRole('heading', { name: 'Versions of the file' })).toBeFocused();
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(1);
  await expect(versionRow(page, 1).getByTestId('file-version-current')).toBeVisible();
  await expect(versionRow(page, 1).getByTestId('file-version-number')).toHaveText('Version 1');
  await expect(versionRow(page, 1)).toHaveAttribute('data-shown', '');
  await expect(versionRow(page, 1).getByTestId('file-version-restore')).toHaveCount(0);
  // Closed with its own button, the focus is back on the one it was opened from.
  await page.getByTestId('file-versions-close').click();
  await expect(list).toHaveCount(0);
  await expect(button).toBeFocused();

  // A new version from a file: it is what the viewer shows, and the one before stays.
  await uploadVersion(page, 'next.pdf', 3);
  await expect(counter(page)).toContainText('1 / 3');
  // Its upload was told where every upload is, over the viewer.
  await expect(
    page.getByTestId('transfers').locator('[data-transfer="upload"][data-name="next.pdf"]'),
  ).toHaveAttribute('data-finished', '');
  await versions(page);
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(2);
  await expect(versionRow(page, 2).getByTestId('file-version-current')).toBeVisible();
  await expect(versionRow(page, 2)).toHaveAttribute('data-shown', '');
  await expect(page.getByTestId('file-viewer-older')).toHaveCount(0);
  // The card's row of the file says the newest version's size.
  await expect(row).toContainText(nextSize);
  await picture(page, 'versions');

  // The first version is downloaded under the file's name, and looked at in the file's place.
  const saved = page.waitForEvent('download');
  await versionRow(page, 1).getByTestId('file-version-download').click();
  expect((await saved).suggestedFilename()).toBe('보고서.pdf');
  await versionRow(page, 1).getByTestId('file-version-view').click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(counter(page)).toContainText('1 / 1');
  await expect(page.getByTestId('file-viewer-older')).toBeVisible();
  await picture(page, 'versions-older');

  // Going back asks first, and writes a new version with the first one's bytes.
  await versions(page);
  await expect(versionRow(page, 1)).toHaveAttribute('data-shown', '');
  await versionRow(page, 2).getByTestId('file-version-view').click();
  await expect(counter(page)).toContainText('1 / 3');
  await versions(page);
  await versionRow(page, 1).getByTestId('file-version-restore').click();
  const asked = page.getByRole('alertdialog', { name: 'Restore this version?' });
  await expect(asked).toBeVisible();
  await asked.getByRole('button', { name: 'Restore' }).click();
  await expect(counter(page)).toContainText('1 / 1');
  await versions(page);
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(3);
  await expect(versionRow(page, 3)).toContainText('Restored from version 1');
  await expect(versionRow(page, 3).getByTestId('file-version-current')).toBeVisible();
  await expect(page.getByTestId('file-viewer-older')).toHaveCount(0);
  await expect(row).toContainText(firstSize);

  // The card reads the file as its newest version.
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  await page
    .getByTestId('card-attachments')
    .locator('[data-attachment]', { hasText: '보고서.pdf' })
    .getByRole('link', { name: '보고서.pdf' })
    .click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(counter(page)).toContainText('1 / 1');
});

/** Somebody else, an editor of the board, writes a version of the file from the version `base`. */
async function writtenElsewhere(page: Page, place: Place, name: string, base: number) {
  await page.evaluate(
    async ({ boardId, cardId, name, base }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const other = dev.someoneElse();
      if (base === 1) await dev.backend.addMember(boardId, (await other.me()).id, 'editor');
      const attached = (await dev.backend.attachments(boardId, cardId)).find(
        (each) => each.name === name,
      );
      if (!attached) throw new Error('the file is not attached');
      const theirs = await other.upload(
        boardId,
        new File(['%PDF-1.4 theirs'], 'theirs.pdf', { type: 'application/pdf' }),
      );
      await other.fileVersions.add(boardId, attached.fileId, base, theirs);
    },
    { ...place, name, base },
  );
}

test('a file that somebody else wrote a version of meanwhile is saved under another name', async ({
  page,
}) => {
  const place = await openCard(page);
  const row = await viewAttached(page, '계획.pdf', 1);
  await expect(row).toContainText(humanFileSize(koreanPdf(1).length));

  // A version written elsewhere after the file was opened: the card is told its size,
  // and the list, read when it is shown, says that what is on screen is the one before.
  await writtenElsewhere(page, place, '계획.pdf', 1);
  await expect(row).toContainText(humanFileSize('%PDF-1.4 theirs'.length));
  await versions(page);
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(2);
  await expect(versionRow(page, 1)).toHaveAttribute('data-shown', '');
  await expect(versionRow(page, 2).getByTestId('file-version-current')).toBeVisible();
  await expect(page.getByTestId('file-viewer-older')).toBeVisible();

  // Another, which the list that is shown has not read.
  await writtenElsewhere(page, place, '계획.pdf', 2);

  await uploadVersion(page, '내 계획.pdf', 2);
  const refusal = page.getByTestId('file-version-refusal');
  await expect(refusal).toHaveAttribute('data-refusal', 'stale');
  await expect(refusal).toContainText('Somebody else wrote a new version in the meantime');
  // Nothing was written over theirs, and the list was read again.
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(3);
  await picture(page, 'versions-stale');

  await page.getByTestId('file-version-save-apart').click();
  await expect(refusal).toHaveCount(0);
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(3);
  if (await page.getByTestId('file-versions').isVisible()) {
    await page.getByTestId('file-versions-close').click();
  }
  await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);
  // The file is attached to the same card, as a file of its own.
  const attachments = page.getByTestId('card-attachments').locator('[data-attachment]');
  await expect(attachments.filter({ hasText: '내 계획.pdf' })).toHaveCount(1);
  await expect(attachments.filter({ hasText: '계획.pdf' })).toHaveCount(2);
});
