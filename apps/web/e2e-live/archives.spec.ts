/**
 * Archives on the real server: a board exported from its menu, every board
 * from the settings, both read with their progress and saved as the
 * server names them; an archive imported in parts makes its boards again;
 * a file that is no archive is refused and told.
 */
import { readFile } from 'node:fs/promises';
import { crc32 } from 'node:zlib';

import { expect, test, type APIRequestContext } from '@playwright/test';

import { boardLink, boardMenu, chooseFromMenu, sidebar } from '../e2e/shell.support.ts';

import {
  boardPath,
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

/** Builds a valid uncompressed (Stored) ZIP buffer in Node for live Notion upload tests. */
function buildStoredZip(entries: readonly { name: string; content: string | Buffer }[]): Buffer {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  for (const entry of entries) {
    const nameBytes = Buffer.from(entry.name, 'utf8');
    const data = Buffer.isBuffer(entry.content)
      ? entry.content
      : Buffer.from(entry.content, 'utf8');
    const crc = crc32(data) >>> 0;

    const local = Buffer.alloc(30 + nameBytes.length);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x0800, 6); // UTF-8 filename flag
    local.writeUInt16LE(0, 8); // Stored
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    nameBytes.copy(local, 30);
    locals.push(local, data);

    const central = Buffer.alloc(46 + nameBytes.length);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(offset, 42);
    nameBytes.copy(central, 46);
    centrals.push(central);

    offset += local.length + data.length;
  }
  const centralDir = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(centralDir.length, 12);
  eocd.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, centralDir, eocd]);
}

test.use({ locale: 'en-US' });

async function boardsTitled(api: APIRequestContext, workspaceId: string, title: string) {
  const boards = (await (
    await api.get(`/api/v1/w/${workspaceId}/boards`)
  ).json()) as Schemas['Container'][];
  return boards.filter((board) => board.title === title);
}

test('a board is exported and imported again, and a file that is no archive is refused', async ({
  page,
  baseURL,
}, testInfo) => {
  const base = baseURL ?? '';
  const owner = person('archivist');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const title = `Archive ${Date.now().toString(36)}`;
  const made = await makeBoard(api, {
    title,
    cards: [
      { title: 'First', status: 'To do' },
      { title: 'Second', status: 'Done' },
    ],
  });

  const errors = watchErrors(page);
  await page.goto(boardPath(made, made.viewId));
  await signInOnPage(page, owner);

  // The board, from its menu: a ZIP saved under the server's name.
  const saving = page.waitForEvent('download');
  await chooseFromMenu(page, boardMenu(page, title), 'export');
  const download = await saving;
  expect(download.suggestedFilename()).toMatch(/^archive-\d{4}-\d{2}-\d{2}\.fvoci$/);
  const archive = testInfo.outputPath('board.fvoci');
  await download.saveAs(archive);
  expect((await readFile(archive)).subarray(0, 2).toString()).toBe('PK');
  await expect(
    page.getByTestId('transfers').locator('[data-transfer="export"][data-finished]'),
  ).toContainText(`${title} is saved.`);

  // Every board, from the settings.
  await sidebar(page).getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');
  const savingAll = page.waitForEvent('download');
  await settings.getByTestId('export-workspace').click();
  const all = await savingAll;
  const allPath = testInfo.outputPath('workspace.fvoci');
  await all.saveAs(allPath);
  expect((await readFile(allPath)).subarray(0, 2).toString()).toBe('PK');

  // Imported: the board is made again, with its cards.
  await settings.getByTestId('import-archive').setInputFiles(archive);
  await expect(
    page.getByTestId('transfers').locator('[data-transfer="import"][data-finished]'),
  ).toContainText('board.fvoci is imported.');
  await expect.poll(async () => (await boardsTitled(api, made.workspaceId, title)).length).toBe(2);
  const copy = (await boardsTitled(api, made.workspaceId, title)).find(
    (board) => board.id !== made.boardId,
  );
  const cards = (await (
    await api.get(`/api/v1/w/${made.workspaceId}/boards/${copy?.id ?? ''}/cards`)
  ).json()) as Schemas['Card'][];
  expect(cards.map((card) => card.title).sort()).toEqual(['First', 'Second']);
  await page.keyboard.press('Escape');
  await expect(boardLink(page, title)).toHaveCount(2);

  // No archive: refused, told, nothing made.
  await sidebar(page).getByTestId('open-settings').click();
  await settings.getByTestId('import-archive').setInputFiles({
    name: 'notes.fvoci',
    mimeType: 'application/zip',
    buffer: Buffer.from('these are not boards'),
  });
  await expect(page.locator('[data-problem="import"]')).toHaveText(
    /That file is no archive of boards or wikis this app can read\./,
  );
  expect(await boardsTitled(api, made.workspaceId, title)).toHaveLength(2);

  await api.dispose();
  expect(errors).toEqual([]);
});

test('Trello JSON and Notion ZIP imports create boards/wikis and render structured summaries', async ({
  page,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('ext-importer');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const seed = await makeBoard(api, { title: 'Seed Board', cards: [] });

  const errors = watchErrors(page);
  await page.goto(boardPath(seed, seed.viewId));
  await signInOnPage(page, owner);

  await sidebar(page).getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');

  // 1. Select Trello format and upload a real Trello JSON export.
  await settings.getByRole('radio', { name: /Trello/i }).click();
  const trelloJson = JSON.stringify({
    id: 'tr-live-1',
    name: 'Trello Live Board',
    desc: 'Uploaded from live E2E',
    lists: [
      { id: 'l1', name: 'To Do', closed: false, pos: 1 },
      { id: 'l2', name: 'Old List', closed: true, pos: 2 },
    ],
    cards: [
      {
        id: 'c1',
        name: 'Launch Live',
        desc: 'Ship **now**.',
        idList: 'l1',
        closed: false,
        pos: 1,
        attachments: [{ id: 'a1', name: 'Runbook', url: 'https://example.com/runbook' }],
      },
      { id: 'c2', name: 'Archived Card', idList: 'l1', closed: true, pos: 2 },
    ],
    actions: [
      {
        id: 'act1',
        type: 'commentCard',
        date: '2026-09-01T10:00:00.000Z',
        data: { text: 'Looks ready', card: { id: 'c1' } },
        memberCreator: { fullName: 'QA Lead' },
      },
    ],
  });
  await settings.getByTestId('import-archive').setInputFiles({
    name: 'trello-live.json',
    mimeType: 'application/json',
    buffer: Buffer.from(trelloJson, 'utf8'),
  });

  const summary = settings.getByTestId('import-summary');
  await expect(summary).toBeVisible();
  await expect(summary.getByTestId('import-counts')).toContainText('Boards: 1');
  await expect(summary.getByTestId('import-counts')).toContainText('Cards: 1');
  await expect(summary.getByTestId('import-counts')).toContainText('Comments: 1');
  await expect(summary.getByTestId('import-skipped')).toContainText('Skipped (');
  await expect(summary.getByTestId('import-warnings')).toContainText('Warnings (');
  await expect(summary.getByTestId('import-created-link')).toContainText('Trello Live Board');

  // 2. Select Notion format and upload a real Notion ZIP export (wiki + CSV board + local asset).
  await settings.getByRole('radio', { name: /Notion/i }).click();
  const notionZip = buildStoredZip([
    {
      name: 'Notion Export/Handbook 0123456789abcdef0123456789abcdef.md',
      content:
        '# Handbook\n\nShared knowledge base.\n\n![Missing](missing.png)\n\n[Notes](Handbook%200123456789abcdef0123456789abcdef/notes.txt)\n',
    },
    {
      name: 'Notion Export/Handbook 0123456789abcdef0123456789abcdef/notes.txt',
      content: 'release checklist',
    },
    {
      name: 'Notion Export/Milestones 11111111111111111111111111111111.csv',
      content: 'Name,Status\nAlpha,Done\nBeta,In Progress\n',
    },
  ]);
  await settings.getByTestId('import-archive').setInputFiles({
    name: 'notion-export.zip',
    mimeType: 'application/zip',
    buffer: notionZip,
  });

  await expect(summary.getByTestId('import-counts')).toContainText('Wikis: 1');
  await expect(summary.getByTestId('import-counts')).toContainText('Boards: 1');
  await expect(summary.getByTestId('import-counts')).toContainText('Pages: 1');
  await expect(summary.getByTestId('import-counts')).toContainText('Cards: 2');
  await expect(summary.getByTestId('import-warnings')).toContainText('Warnings (');

  // Click the created Milestones board link to jump directly to it and close Settings.
  await summary.getByTestId('import-created-link').filter({ hasText: 'Milestones' }).click();
  await expect(settings).toBeHidden();
  await expect(boardLink(page, 'Trello Live Board')).toBeVisible();
  await expect(boardLink(page, 'Milestones')).toBeVisible();

  await api.dispose();
  expect(errors).toEqual([]);
});
