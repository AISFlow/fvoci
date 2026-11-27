/**
 * The diaries of the backend in memory (`Backend.diary`), as the server keeps
 * them (`crates/store/src/diaries.rs`): a diary is a board of the kind
 * `diary`, private, its maker its only member, in their own space and
 * nowhere else (any other workspace is not there for its operations: 404); a
 * space has at most `DIARIES_PER_SPACE`. Its entries are blocks of the type
 * `entry`, titled with their day, which is among their `fields` with their
 * mood, weather and place; a diary has one entry a day (making a day's entry
 * again answers the one there) and at most `ENTRIES_PER_DIARY`. A day is a
 * day of the calendar no more than two days past the world's own (UTC).
 * Deleting an entry puts it in the trash, where it holds no day; it comes
 * back to its day unless the day has an entry since (409 `day_taken`).
 */
import type { DocNode } from '../../editor/schema.ts';
import {
  addDays,
  dateOfUtc,
  daysBetween,
  parseCalendarDate,
  type CalendarDate,
} from '../../logic/dates.ts';
import { monthDayOf, type MonthDay } from '../../logic/diary-days.ts';
import { runsOf } from '../../logic/diary-streak.ts';
import { createBoard, type Container } from '../../logic/wire.ts';
import {
  NOTE_MAX_CHARS,
  type DayNotes,
  type Diaries,
  type DiaryDay,
  type DiaryEntryDetail,
  type DiaryRun,
  type EntryPatch,
} from '../diary-types.ts';

import type { BackendError } from '../problems.ts';
import type { DeletedItem, Paging } from '../trash-reads.ts';

import { checkedBody, checkedWhole } from './bodies.ts';
import { MemoryBodyVersions, writtenWhole } from './body-versions.ts';
import { fileAsNew } from './categories.ts';
import { pause } from './pause.ts';
import { fail } from './refuse.ts';
import { changeOf, copy, entryOf, memberOf } from './shapes.ts';
import { checkWorkspace, ownSpaceOf } from './spaces.ts';
import { pageOf } from './paging.ts';
import { deletion } from './trash.ts';
import type { StoredBlock, StoredCard, World } from './world.ts';

/** The server's limits (`crates/model/src/diary.rs`). */
export const DIARIES_PER_SPACE = 20;
export const ENTRIES_PER_DIARY = 40_000;
export const DAYS_AHEAD = 2;
/** The most days one list of written days spans (`DAYS_PER_LIST`). */
const DAYS_PER_LIST = 366;

/** What the diaries share with the backend they are of. */
export interface DiaryHost {
  readonly world: World;
  readonly userId: string;
  /** Milliseconds every call waits before it answers. */
  readonly latency: number;
}

const isDiary = (board: Container | undefined): board is Container => board?.kind === 'diary';

function detailOf(block: StoredBlock): DiaryEntryDetail {
  return {
    ...entryOf(block),
    body: copy(block.body ?? null),
    createdBy: block.createdBy,
    modifiedBy: block.modifiedBy,
  };
}

export class MemoryDiaries implements Diaries {
  readonly #host: DiaryHost;
  readonly #failures = new Map<keyof Diaries, BackendError>();
  #bodyVersions: MemoryBodyVersions | undefined;

  constructor(host: DiaryHost) {
    this.#host = host;
  }

  get bodyVersions(): MemoryBodyVersions {
    this.#bodyVersions ??= new MemoryBodyVersions(
      this.#world,
      this.#host.userId,
      () => this.#enter('bodyVersions'),
      'entry',
    );
    return this.#bodyVersions;
  }

  /** Makes the next call of `method` reject with `error`, once. */
  failNext(method: keyof Diaries, error: BackendError): void {
    this.#failures.set(method, error);
  }

  async #enter(method: keyof Diaries): Promise<void> {
    await pause(this.#host.latency);
    const failure = this.#failures.get(method);
    if (failure) {
      this.#failures.delete(method);
      throw failure;
    }
  }

  get #world(): World {
    return this.#host.world;
  }

  /** 404 unless the workspace is the reader's own space (which has diaries). */
  #ownSpace(workspaceId: string): void {
    checkWorkspace(this.#world, this.#host.userId, workspaceId);
    if (ownSpaceOf(this.#world, this.#host.userId).id !== workspaceId) {
      fail(404, 'no diaries in this workspace');
    }
  }

  #diariesIn(workspaceId: string): Container[] {
    return [...this.#world.containers.values()]
      .filter((board) => isDiary(board) && board.workspaceId === workspaceId)
      .sort((first, second) => first.createdAt - second.createdAt);
  }

  /** The reader's diary; anything else is not there. */
  #diary(diaryId: string): Container {
    const diary = this.#world.containers.get(diaryId);
    const mine = (this.#world.members.get(diaryId) ?? []).some(
      (member) => member.userId === this.#host.userId,
    );
    if (!isDiary(diary) || !mine) fail(404, 'no such diary');
    checkWorkspace(this.#world, this.#host.userId, diary.workspaceId);
    return diary;
  }

  #entriesOf(diaryId: string): StoredBlock[] {
    return this.#world.blocksOf(diaryId).filter((block) => block.type === 'entry');
  }

  #entry(diaryId: string, entryId: string): StoredBlock {
    const block = this.#world.blocks.get(entryId);
    if (block?.type !== 'entry' || block.containerId !== diaryId) fail(404, 'no such entry');
    return block;
  }

  async diaries(workspaceId: string): Promise<Container[]> {
    await this.#enter('diaries');
    this.#ownSpace(workspaceId);
    return this.#diariesIn(workspaceId).map(copy);
  }

  async createDiary(
    workspaceId: string,
    draft: { title?: string; ifNone?: boolean },
  ): Promise<Container> {
    await this.#enter('createDiary');
    this.#ownSpace(workspaceId);
    const world = this.#world;
    const userId = this.#host.userId;
    const there = this.#diariesIn(workspaceId);
    const oldest = there[0];
    if (draft.ifNone && oldest) return copy(oldest);
    if (there.length >= DIARIES_PER_SPACE) {
      fail(400, 'the space has as many diaries as it may', {
        reason: 'too_many_diaries',
        limit: DIARIES_PER_SPACE,
      });
    }
    const now = world.now();
    const diary: Container = {
      ...createBoard(),
      id: world.newId(),
      workspaceId,
      kind: 'diary',
      createdBy: userId,
      modifiedBy: userId,
      visibility: 'private',
      title: draft.title ?? '',
      cardProperties: [],
      createdAt: now,
      updatedAt: now,
    };
    world.containers.set(diary.id, diary);
    world.members.set(diary.id, [memberOf(diary.id, userId, 'admin')]);
    world.tellUsers([userId], { kind: 'container', container: copy(diary) });
    fileAsNew(world, userId, diary);
    return copy(diary);
  }

  async entriesOn(diaryId: string, monthDay: MonthDay): Promise<DiaryEntryDetail[]> {
    await this.#enter('entriesOn');
    this.#diary(diaryId);
    return this.#entriesOf(diaryId)
      .map(detailOf)
      .filter((entry) => monthDayOf(entry.date) === monthDay)
      .sort((first, second) => second.date.localeCompare(first.date));
  }

  async days(diaryId: string, from: CalendarDate, to: CalendarDate): Promise<DiaryDay[]> {
    await this.#enter('days');
    this.#diary(diaryId);
    for (const [field, day] of [
      ['from', from],
      ['to', to],
    ] as const) {
      if (!parseCalendarDate(day)) fail(400, 'no such day', { field });
    }
    if (daysBetween(from, to) >= DAYS_PER_LIST) {
      fail(400, 'too many days at once', { field: 'to', limit: DAYS_PER_LIST });
    }
    return this.#entriesOf(diaryId)
      .map(entryOf)
      .filter((entry) => entry.date >= from && entry.date <= to)
      .sort((first, second) => first.date.localeCompare(second.date))
      .map((entry) => ({ date: entry.date, entryId: entry.id, mood: entry.mood }));
  }

  async runs(diaryId: string): Promise<DiaryRun[]> {
    await this.#enter('runs');
    this.#diary(diaryId);
    return runsOf(this.#entriesOf(diaryId).map((block) => entryOf(block).date));
  }

  async createEntry(diaryId: string, date: CalendarDate): Promise<DiaryEntryDetail> {
    await this.#enter('createEntry');
    this.#diary(diaryId);
    const world = this.#world;
    const day = parseCalendarDate(date);
    if (!day) fail(400, 'no such day', { field: 'date' });
    if (day > addDays(dateOfUtc(world.now()), DAYS_AHEAD)) {
      fail(400, 'the day is too far ahead', { field: 'date' });
    }
    const entries = this.#entriesOf(diaryId);
    const there = entries.find((block) => block.title === day);
    if (there) return detailOf(there);
    if (entries.length >= ENTRIES_PER_DIARY) {
      fail(400, 'the diary has as many entries as it may', {
        reason: 'too_many_entries',
        limit: ENTRIES_PER_DIARY,
      });
    }
    const now = world.now();
    const userId = this.#host.userId;
    const block: StoredBlock = {
      id: world.newId(),
      containerId: diaryId,
      // As the server's: an entry is below no block (`EntryBelowBlock`).
      parentId: '',
      createdBy: userId,
      modifiedBy: userId,
      type: 'entry',
      title: day,
      fields: { date: day, mood: null, weather: '', place: '' },
      body: null,
      bodyUpdatedAt: now,
      createdAt: now,
      updatedAt: now,
    };
    world.blocks.set(block.id, block);
    world.tellContainer(diaryId, changeOf(diaryId, block));
    return detailOf(block);
  }

  async entry(diaryId: string, entryId: string): Promise<DiaryEntryDetail> {
    await this.#enter('entry');
    this.#diary(diaryId);
    return detailOf(this.#entry(diaryId, entryId));
  }

  async patchEntry(diaryId: string, entryId: string, patch: EntryPatch): Promise<DiaryEntryDetail> {
    await this.#enter('patchEntry');
    this.#diary(diaryId);
    const block = this.#entry(diaryId, entryId);
    if (patch.body !== undefined && patch.appendBody !== undefined) {
      fail(400, '`body` and `appendBody` cannot be sent in the same patch', {
        field: 'appendBody',
      });
    }
    if (patch.body !== undefined && patch.readAt === undefined) {
      fail(400, 'a body is written with the version it was read at', { field: 'readAt' });
    }
    const notes = checkedNotes(patch);
    if (patch.body !== undefined) {
      const body = checkedWhole(this.#world, diaryId, entryId, patch.body);
      checkReadAt(block, patch.readAt);
      this.#writeBody(diaryId, block, body, notes);
      return detailOf(block);
    }
    if (patch.appendBody !== undefined) {
      const extra = checkedBody(this.#world, diaryId, patch.appendBody, 'appendBody');
      if (extra !== null) {
        this.#world.bodyFlushed?.(entryId);
        const existing = block.body ?? null;
        const merged =
          existing === null
            ? extra
            : { ...existing, content: [...(existing.content ?? []), ...(extra.content ?? [])] };
        const checked = checkedBody(this.#world, diaryId, merged, 'appendBody');
        checkReadAt(block, patch.readAt);
        this.#writeBody(diaryId, block, checked, notes);
        return detailOf(block);
      }
      checkReadAt(block, patch.readAt);
    } else {
      checkReadAt(block, patch.readAt);
    }
    if (Object.keys(notes).length > 0) {
      block.fields = { ...block.fields, ...notes };
      block.updatedAt = this.#world.now();
      block.modifiedBy = this.#host.userId;
      this.#world.tellContainer(diaryId, changeOf(diaryId, block));
    }
    return detailOf(block);
  }

  #writeBody(
    diaryId: string,
    block: StoredBlock,
    body: DocNode | null,
    notes: Partial<DayNotes>,
  ): void {
    const changed = JSON.stringify(block.body ?? null) !== JSON.stringify(body);
    writtenWhole(this.#world, block as StoredCard, body, this.#host.userId, () => {
      block.body = body;
      block.fields = { ...block.fields, ...notes };
      const now = this.#world.now();
      block.updatedAt = now;
      if (changed) block.bodyUpdatedAt = now;
      block.modifiedBy = this.#host.userId;
    });
    // As the server: its room's document is made anew from the body written whole.
    this.#world.bodyWritten?.(block.id);
    this.#world.tellContainer(diaryId, changeOf(diaryId, block));
  }

  async deleteEntry(diaryId: string, entryId: string): Promise<void> {
    await this.#enter('deleteEntry');
    this.#diary(diaryId);
    const block = this.#entry(diaryId, entryId);
    this.#world.bodyFlushed?.(entryId);
    const entry = entryOf(block);
    const empty =
      (block.body ?? null) === null &&
      entry.mood === null &&
      entry.weather === '' &&
      entry.place === '';
    if (empty) {
      this.#world.bodyVersions.delete(entryId);
    } else {
      this.#world.deletedBlocks.set(entryId, {
        containerId: diaryId,
        blocks: [block],
        ...deletion({ world: this.#world, userId: this.#host.userId }),
      });
    }
    this.#world.blocks.delete(entryId);
    this.#world.tellContainer(diaryId, {
      kind: 'blocksDeleted',
      containerId: diaryId,
      blockIds: [entryId],
    });
  }

  async deletedEntries(diaryId: string, paging?: Paging): Promise<DeletedItem[]> {
    await this.#enter('deletedEntries');
    this.#diary(diaryId);
    const items: DeletedItem[] = [];
    for (const [id, deleted] of this.#world.deletedBlocks) {
      const [block] = deleted.blocks;
      if (deleted.containerId !== diaryId || block?.type !== 'entry') continue;
      const { mood } = entryOf(block);
      const { deletedAt, deletedBy } = deleted;
      items.push({ id, title: block.title, deletedAt, deletedBy, mood });
    }
    return pageOf(
      items.sort((one, other) => other.deletedAt - one.deletedAt),
      paging,
    );
  }

  async restoreEntry(diaryId: string, entryId: string): Promise<DiaryEntryDetail> {
    await this.#enter('restoreEntry');
    this.#diary(diaryId);
    const deleted = this.#world.deletedBlocks.get(entryId);
    const [block] = deleted?.blocks ?? [];
    if (deleted?.containerId !== diaryId || block?.type !== 'entry') {
      fail(404, 'no such entry in the trash');
    }
    const entries = this.#entriesOf(diaryId);
    if (entries.some((each) => each.title === block.title)) {
      fail(409, 'the day has an entry since', { reason: 'day_taken' });
    }
    if (entries.length >= ENTRIES_PER_DIARY) {
      fail(400, 'the diary has as many entries as it may', {
        reason: 'too_many_entries',
        limit: ENTRIES_PER_DIARY,
      });
    }
    this.#world.deletedBlocks.delete(entryId);
    this.#world.blocks.set(entryId, block);
    this.#world.tellContainer(diaryId, changeOf(diaryId, block));
    return detailOf(block);
  }
}

/**
 * Checks `readAt` against the entry's body change window `[bodyUpdatedAt, updatedAt]`,
 * so metadata-only patches (`mood`/`weather`/`place`) do not cause spurious 409s
 * while any actual body change rejects stale `readAt` values.
 */
function checkReadAt(block: StoredBlock, readAt: number | undefined): void {
  if (readAt === undefined) return;
  const bodyAt = block.bodyUpdatedAt ?? (block.body ? block.updatedAt : block.createdAt);
  if (readAt < bodyAt || readAt > block.updatedAt) {
    fail(409, 'the entry changed since it was read');
  }
}

/** The notes a patch sets, as the server takes them: a weather or place of `NOTE_MAX_CHARS` at most. */
function checkedNotes(patch: EntryPatch): Partial<DayNotes> {
  const notes: Partial<DayNotes> = {};
  if (patch.mood !== undefined) notes.mood = patch.mood;
  for (const field of ['weather', 'place'] as const) {
    const text = patch[field];
    if (text === undefined) continue;
    if (Array.from(text).length > NOTE_MAX_CHARS) {
      fail(400, `the ${field} is too long`, { field, limit: NOTE_MAX_CHARS });
    }
    if (text.includes('\0')) fail(400, `the ${field} holds a NUL`, { field });
    notes[field] = text;
  }
  return notes;
}
