/**
 * An archive of boards as the backend in memory writes it: a line of its
 * own version, then each board and its blocks, one per line. What the rooms
 * of their cards took is written into the cards first (`flushContainers`).
 */
import { createBoard, type Container } from '../../logic/wire.ts';
import type {
  DownloadedFile,
  DownloadOptions,
  ImportCounts,
  ImportFormat,
  ImportSummary,
  ImportWarning,
} from '../backend.ts';

import { FILE_BLOCK_IN_COMMENT, flushContainers } from './bodies.ts';
import { fail } from './refuse.ts';
import { hasFileBlock, MEMORY_ARCHIVE } from './shapes.ts';
import { oldestFirst, type StoredBlock, type World } from './world.ts';

export function archiveOf(
  world: World,
  containers: readonly Container[],
  options: DownloadOptions | undefined,
): DownloadedFile {
  flushContainers(world, containers);
  const lines = [JSON.stringify({ version: MEMORY_ARCHIVE })];
  for (const board of containers) {
    lines.push(JSON.stringify({ type: 'container', data: board }));
    const schedule = board.kind === 'course' ? world.courseSchedules.get(board.id) : undefined;
    if (schedule) {
      lines.push(
        JSON.stringify({
          type: 'courseSchedule',
          data: { containerId: board.id, ...schedule },
        }),
      );
    }
    // As the server writes them, which is the order an import numbers the cards in.
    for (const block of world.blocksOf(board.id).sort(oldestFirst)) {
      const exported =
        block.type === 'card' && block.fields.isTemplate === true
          ? { ...block, number: undefined }
          : block;
      lines.push(JSON.stringify({ type: 'block', data: exported }));
    }
  }
  const bytes = new Blob([`${lines.join('\n')}\n`], { type: 'application/octet-stream' });
  options?.onProgress?.({ sentBytes: bytes.size, totalBytes: bytes.size });
  const day = new Date(world.now()).toISOString().slice(0, 10);
  return { bytes, name: `archive-${day}.fvoci` };
}

/** The lines of an archive of the memory backend; 400 for what is none (`exportContainer`). */
export async function archiveLines(
  file: File,
): Promise<{ type?: string; version?: string; data?: unknown }[]> {
  let lines: { type?: string; version?: string; data?: unknown }[];
  try {
    lines = (await file.text())
      .split('\n')
      .filter((line) => line.trim() !== '')
      .map((line) => JSON.parse(line) as { type?: string; version?: string; data?: unknown });
  } catch {
    fail(400, 'not an archive');
  }
  if (lines[0]?.version !== MEMORY_ARCHIVE) fail(400, 'not an archive of this app');
  return lines;
}

/**
 * A block of an archive's line as the backend keeps it (`containerId`). A
 * comment with a file block refuses the archive (400), as the server refuses
 * it whole.
 */
export function blockOfLine(data: unknown): StoredBlock {
  if (typeof data !== 'object' || data === null) fail(400, 'invalid block in archive');
  const block = data as StoredBlock;
  // As the server refuses the archive whole: a comment has no file block.
  if (block.type === 'comment' && block.body && hasFileBlock(block.body)) {
    fail(400, FILE_BLOCK_IN_COMMENT, { field: 'body' });
  }
  return block;
}

/** The containers of `workspaceId` exported by `exportWorkspace` for `userId`. */
export function exportableContainers(
  world: World,
  workspaceId: string,
  userId: string,
): Container[] {
  return [...world.containers.values()].filter(
    (board) =>
      board.workspaceId === workspaceId &&
      !board.isTemplate &&
      (board.visibility === 'open' ||
        (world.members.get(board.id) ?? []).some((member) => member.userId === userId)),
  );
}

export type ArchivedCourseSchedule = {
  startDate: string;
  targetDate: string;
  studyDays: number;
};

const MAX_SCHEDULE_SPAN_DAYS = 3660;
const DAY_MS = 86_400_000;
const ALLOWED_SCHEDULE_KEYS = new Set([
  'containerId',
  'startDate',
  'targetDate',
  'studyDays',
  'updatedAt',
]);

/** Parses a strict `YYYY-MM-DD` calendar date to UTC day number, or `undefined` if invalid. */
export function parseIsoDateDays(value: string): number | undefined {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!match) return undefined;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (year < 1 || month < 1 || month > 12 || day < 1 || day > 31) return undefined;
  const check = new Date(0);
  check.setUTCFullYear(year, month - 1, day);
  if (
    check.getUTCFullYear() !== year ||
    check.getUTCMonth() + 1 !== month ||
    check.getUTCDate() !== day
  ) {
    return undefined;
  }
  return Math.floor(check.getTime() / DAY_MS);
}

function hasMatchingStudyDay(startDay: number, targetDay: number, studyDays: number): boolean {
  const span = Math.min(targetDay - startDay, 6);
  for (let offset = 0; offset <= span; offset += 1) {
    const weekday = (new Date((startDay + offset) * DAY_MS).getUTCDay() + 6) % 7;
    if ((studyDays & (1 << weekday)) !== 0) return true;
  }
  return false;
}

function parseCourseScheduleLine(
  data: unknown,
  containers: readonly Container[],
): { containerId: string; schedule: ArchivedCourseSchedule } {
  if (typeof data !== 'object' || data === null || Array.isArray(data)) {
    fail(400, 'invalid courseSchedule line');
  }
  const raw = data as Record<string, unknown>;
  for (const key of Object.keys(raw)) {
    if (!ALLOWED_SCHEDULE_KEYS.has(key)) fail(400, `unknown courseSchedule field: ${key}`);
  }
  if (raw.containerId !== undefined && typeof raw.containerId !== 'string') {
    fail(400, 'invalid courseSchedule target');
  }
  let containerId = typeof raw.containerId === 'string' ? raw.containerId : '';
  if (!containerId) {
    if (containers.length !== 1) fail(400, 'missing containerId in courseSchedule');
    containerId = containers[0]?.id ?? '';
  }
  if (
    typeof raw.startDate !== 'string' ||
    typeof raw.targetDate !== 'string' ||
    typeof raw.studyDays !== 'number' ||
    !Number.isInteger(raw.studyDays) ||
    raw.studyDays < 1 ||
    raw.studyDays > 127
  ) {
    fail(400, 'invalid courseSchedule line');
  }
  const startDay = parseIsoDateDays(raw.startDate);
  const targetDay = parseIsoDateDays(raw.targetDate);
  if (startDay === undefined || targetDay === undefined || startDay > targetDay) {
    fail(400, 'invalid courseSchedule date range');
  }
  if (targetDay - startDay > MAX_SCHEDULE_SPAN_DAYS) {
    fail(400, 'courseSchedule span exceeds maximum');
  }
  if (!hasMatchingStudyDay(startDay, targetDay, raw.studyDays)) {
    fail(400, 'courseSchedule range contains no study day');
  }
  return {
    containerId,
    schedule: {
      startDate: raw.startDate,
      targetDate: raw.targetDate,
      studyDays: raw.studyDays,
    },
  };
}

const ALLOWED_LINE_TYPES = new Set(['container', 'block', 'containerMember', 'courseSchedule']);

/** Reads an archive file into its containers, blocks, and optional course schedules (`importArchive`). */
export async function readArchive(file: File): Promise<{
  containers: Container[];
  blocks: StoredBlock[];
  schedules: Map<string, ArchivedCourseSchedule>;
}> {
  const lines = await archiveLines(file);
  for (const line of lines.slice(1)) {
    if (!line.type || !ALLOWED_LINE_TYPES.has(line.type)) {
      fail(400, `unsupported archive line type: ${String(line.type)}`);
    }
  }
  const containers = lines
    .filter((line) => line.type === 'container')
    .map((line) => {
      const raw = line.data as Partial<Container> | null | undefined;
      if (
        !raw ||
        typeof raw !== 'object' ||
        (raw.visibility !== 'open' && raw.visibility !== 'private')
      ) {
        fail(400, 'invalid container in archive');
      }
      return raw as Container;
    });
  const blocks = lines
    .filter((line) => line.type === 'block')
    .map((line) => blockOfLine(line.data));
  const schedules = new Map<string, ArchivedCourseSchedule>();
  for (const line of lines) {
    if (line.type !== 'courseSchedule') continue;
    const { containerId, schedule } = parseCourseScheduleLine(line.data, containers);
    if (schedules.has(containerId)) fail(400, 'duplicate courseSchedule');
    schedules.set(containerId, schedule);
  }
  const byId = new Map(containers.map((container) => [container.id, container]));
  for (const containerId of schedules.keys()) {
    const target = byId.get(containerId);
    if (!target || (target.kind ?? 'board') !== 'course') {
      fail(400, 'courseSchedule must reference a course container');
    }
  }
  return { containers, blocks, schedules };
}

interface TrelloCardLike {
  id?: unknown;
  name?: unknown;
  idList?: unknown;
  closed?: unknown;
}

interface TrelloListLike {
  id?: unknown;
  name?: unknown;
  closed?: unknown;
}

/** Reads an import file in `format` (`fvoci`, `trello`, or `notion`) for the memory backend. */
export async function readImportFile(
  file: File,
  format: ImportFormat,
  workspaceId: string,
): Promise<{
  containers: Container[];
  blocks: StoredBlock[];
  schedules: Map<string, ArchivedCourseSchedule>;
  skipped: ImportWarning[];
}> {
  if (format === 'fvoci') {
    const parsed = await readArchive(file);
    return { ...parsed, skipped: [] };
  }
  if (file.size === 0) fail(400, 'cannot import an empty file');
  if (format === 'trello') {
    return { ...(await readTrelloImport(file, workspaceId)), schedules: new Map() };
  }
  const head = new Uint8Array(await file.slice(0, 2).arrayBuffer());
  if (head[0] !== 0x50 || head[1] !== 0x4b) fail(400, 'invalid zip archive');
  const title = file.name.replace(/\.zip$/i, '') || 'Notion Import';
  const wiki: Container = { ...createBoard(), workspaceId, title, kind: 'wiki' };
  return { containers: [wiki], blocks: [], schedules: new Map(), skipped: [] };
}

async function readTrelloImport(
  file: File,
  workspaceId: string,
): Promise<{ containers: Container[]; blocks: StoredBlock[]; skipped: ImportWarning[] }> {
  let raw: unknown;
  try {
    raw = JSON.parse(await file.text());
  } catch {
    fail(400, 'invalid Trello board JSON');
  }
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    fail(400, 'invalid Trello board JSON');
  }
  const data = raw as Record<string, unknown>;
  const hasLists = data.lists !== undefined;
  const hasCards = data.cards !== undefined;
  if (
    (!hasLists && !hasCards) ||
    (hasLists && !Array.isArray(data.lists)) ||
    (hasCards && !Array.isArray(data.cards))
  ) {
    fail(400, 'JSON does not look like a Trello board export');
  }
  const lists = (hasLists ? data.lists : []) as unknown[];
  const cards = (hasCards ? data.cards : []) as unknown[];
  const skipped: ImportWarning[] = [];
  const closedLists = new Set<string>();
  for (const item of lists) {
    if (typeof item !== 'object' || item === null) continue;
    const list = item as TrelloListLike;
    const listId = typeof list.id === 'string' ? list.id : '';
    const listName = typeof list.name === 'string' ? list.name : listId;
    if (list.closed === true && listId) {
      closedLists.add(listId);
      skipped.push({
        code: 'closed_item_skipped',
        item: listName,
        reason: 'archived list skipped',
      });
    }
  }
  const boardName = typeof data.name === 'string' ? data.name.trim() : '';
  const boardDesc = typeof data.desc === 'string' ? data.desc : '';
  const board: Container = {
    ...createBoard(),
    workspaceId,
    title: boardName || 'Trello Board',
    description: boardDesc,
  };
  const blocks: StoredBlock[] = [];
  for (const item of cards) {
    if (typeof item !== 'object' || item === null) continue;
    const card = item as TrelloCardLike;
    const cardId = typeof card.id === 'string' ? card.id : `trello-${String(blocks.length)}`;
    const cardName = typeof card.name === 'string' ? card.name : '';
    const cardListId = typeof card.idList === 'string' ? card.idList : '';
    if (card.closed === true || (cardListId && closedLists.has(cardListId))) {
      skipped.push({
        code: 'closed_item_skipped',
        item: cardName || cardId,
        reason: 'archived card skipped',
      });
      continue;
    }
    blocks.push({
      id: cardId,
      parentId: '',
      containerId: board.id,
      type: 'card',
      title: cardName,
      fields: {},
      body: null,
      createdBy: '',
      modifiedBy: '',
      createdAt: 0,
      updatedAt: 0,
    });
  }
  return { containers: [board], blocks, skipped };
}

/** Builds an `ImportSummary` from containers and blocks inserted into the memory world. */
export function summarizeImport(
  format: ImportFormat,
  created: readonly Container[],
  blocks: readonly StoredBlock[],
  skipped: readonly ImportWarning[],
): ImportSummary {
  const counts: Required<{ -readonly [K in keyof ImportCounts]: number }> = {
    boards: 0,
    wikis: 0,
    courses: 0,
    cards: 0,
    pages: 0,
    units: 0,
    comments: 0,
    files: 0,
  };
  for (const container of created) {
    const kind = container.kind ?? 'board';
    if (kind === 'wiki') counts.wikis += 1;
    else if (kind === 'course') counts.courses += 1;
    else if (kind === 'board') counts.boards += 1;
  }
  for (const block of blocks) {
    if (block.type === 'card') counts.cards += 1;
    else if (block.type === 'page') counts.pages += 1;
    else if (block.type === 'unit') counts.units += 1;
    else if (block.type === 'comment') counts.comments += 1;
  }
  return {
    format,
    imported: counts,
    membersLeftOut: 0,
    containerIds: created.map((container) => container.id),
    skipped,
    warnings: [],
  };
}
