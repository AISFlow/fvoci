/**
 * The diaries of the reader's own space and their entries on the server
 * (`Backend.diary`): the operations under `/w/{workspaceId}/diaries`
 * (`getDiaries`, `createDiary`, `getDiaryEntriesOn`, `getDiaryDays`,
 * `getDiaryRuns`, `createDiaryEntry`, `getDiaryEntry`, `patchDiaryEntry`,
 * `deleteDiaryEntry`). What a diary shares
 * with a board (reading it, renaming, deleting and bringing it back, the
 * sidebar, its archive) goes by the board's operations.
 */
import type { CalendarDate } from '../logic/dates.ts';
import type { MonthDay } from '../logic/diary-days.ts';
import type { Container } from '../logic/wire.ts';
import {
  MOODS,
  type Diaries,
  type DiaryDay,
  type DiaryEntry,
  type DiaryEntryDetail,
  type DiaryRun,
  type EntryPatch,
  type Mood,
} from '../state/diary-types.ts';
import type { DeletedItem, Paging } from '../state/trash-reads.ts';

import { ServerEntryVersions } from './body-versions.ts';
import { bodyOf, containerOf, wireBody, type Schemas } from './convert.ts';
import type { WikiRequest } from './wikis.ts';

export function entryDetailOf(entry: Schemas['DiaryEntry']): DiaryEntryDetail {
  return {
    id: entry.id,
    diaryId: entry.diaryId,
    date: entry.date as CalendarDate,
    mood: entry.mood ?? null,
    weather: entry.weather,
    place: entry.place,
    body: bodyOf(entry.body),
    createdBy: entry.createdBy,
    modifiedBy: entry.modifiedBy,
    createdAt: entry.createdAt,
    updatedAt: entry.updatedAt,
  };
}

/**
 * An entry as a change message brings it: a block of the type `entry`, whose
 * day, mood, weather and place are among its `fields`.
 */
export function entryOfBlock(block: Schemas['Block']): DiaryEntry {
  const fields = block.fields as Record<string, unknown>;
  const text = (value: unknown): string => (typeof value === 'string' ? value : '');
  const date = typeof fields.date === 'string' ? fields.date : block.title;
  return {
    id: block.id,
    diaryId: block.containerId,
    date: date as CalendarDate,
    mood: MOODS.includes(text(fields.mood) as Mood) ? (fields.mood as Mood) : null,
    weather: text(fields.weather),
    place: text(fields.place),
    createdAt: block.createdAt,
    updatedAt: block.updatedAt,
  };
}

function entryPatchOf(patch: EntryPatch): Schemas['DiaryEntryPatch'] {
  return {
    ...(patch.body === undefined ? {} : { body: patch.body && wireBody(patch.body) }),
    ...(patch.appendBody === undefined
      ? {}
      : { appendBody: patch.appendBody && wireBody(patch.appendBody) }),
    ...(patch.readAt === undefined ? {} : { readAt: patch.readAt }),
    ...(patch.mood === undefined ? {} : { mood: patch.mood }),
    ...(patch.weather === undefined ? {} : { weather: patch.weather }),
    ...(patch.place === undefined ? {} : { place: patch.place }),
  };
}

export class ServerDiaries implements Diaries {
  readonly bodyVersions: ServerEntryVersions<DiaryEntryDetail>;
  /** A request under the workspace of the diary `params.diaryId` names (`WikiRequest`'s way). */
  readonly #request: WikiRequest;
  /** Keeps the workspace of each diary read, for the paths under it. */
  readonly #learn: (diaries: readonly Schemas['Container'][]) => void;

  constructor(request: WikiRequest, learn: (diaries: readonly Schemas['Container'][]) => void) {
    this.#request = request;
    this.#learn = learn;
    this.bodyVersions = new ServerEntryVersions(
      (path, params, options) => this.#request(path, params, options),
      entryDetailOf,
    );
  }

  async diaries(workspaceId: string): Promise<Container[]> {
    const diaries = await this.#request<Schemas['Container'][]>('/diaries', {}, {}, workspaceId);
    this.#learn(diaries);
    return diaries.map(containerOf);
  }

  async createDiary(
    workspaceId: string,
    draft: { title?: string; ifNone?: boolean },
  ): Promise<Container> {
    const request: Schemas['NewDiary'] = {
      ...(draft.title ? { title: draft.title } : {}),
      ...(draft.ifNone ? { ifNone: true } : {}),
    };
    const diary = await this.#request<Schemas['Container']>(
      '/diaries',
      {},
      { method: 'POST', json: request },
      workspaceId,
    );
    this.#learn([diary]);
    return containerOf(diary);
  }

  async entriesOn(diaryId: string, monthDay: MonthDay): Promise<DiaryEntryDetail[]> {
    const entries = await this.#request<Schemas['DiaryEntry'][]>(
      '/diaries/{diaryId}/days/{monthDay}',
      { diaryId, monthDay },
    );
    return entries.map(entryDetailOf);
  }

  async days(diaryId: string, from: CalendarDate, to: CalendarDate): Promise<DiaryDay[]> {
    const days = await this.#request<Schemas['DiaryDay'][]>(
      '/diaries/{diaryId}/days',
      { diaryId },
      { query: { from, to } },
    );
    return days.map((day) => ({
      date: day.date as CalendarDate,
      entryId: day.entryId,
      mood: day.mood ?? null,
    }));
  }

  async runs(diaryId: string): Promise<DiaryRun[]> {
    const runs = await this.#request<Schemas['DiaryRun'][]>('/diaries/{diaryId}/runs', {
      diaryId,
    });
    return runs.map((run) => ({
      from: run.from as CalendarDate,
      to: run.to as CalendarDate,
      days: run.days,
    }));
  }

  async createEntry(diaryId: string, date: CalendarDate): Promise<DiaryEntryDetail> {
    const request: Schemas['NewDiaryEntry'] = { date };
    return entryDetailOf(
      await this.#request<Schemas['DiaryEntry']>(
        '/diaries/{diaryId}/entries',
        { diaryId },
        { method: 'POST', json: request },
      ),
    );
  }

  async entry(diaryId: string, entryId: string): Promise<DiaryEntryDetail> {
    return entryDetailOf(
      await this.#request<Schemas['DiaryEntry']>('/diaries/{diaryId}/entries/{entryId}', {
        diaryId,
        entryId,
      }),
    );
  }

  async patchEntry(
    diaryId: string,
    entryId: string,
    patch: EntryPatch,
    options: { keepalive?: boolean } = {},
  ): Promise<DiaryEntryDetail> {
    return entryDetailOf(
      await this.#request<Schemas['DiaryEntry']>(
        '/diaries/{diaryId}/entries/{entryId}',
        { diaryId, entryId },
        { method: 'PATCH', json: entryPatchOf(patch), keepalive: options.keepalive },
      ),
    );
  }

  async deleteEntry(
    diaryId: string,
    entryId: string,
    options: { keepalive?: boolean } = {},
  ): Promise<void> {
    await this.#request(
      '/diaries/{diaryId}/entries/{entryId}',
      { diaryId, entryId },
      { method: 'DELETE', keepalive: options.keepalive },
    );
  }

  async deletedEntries(diaryId: string, paging?: Paging): Promise<DeletedItem[]> {
    const entries = await this.#request<Schemas['DeletedDiaryEntry'][]>(
      '/diaries/{diaryId}/trash/entries',
      { diaryId },
      { query: { ...paging } },
    );
    return entries.map(({ id, date, deletedAt, deletedBy, mood }) => ({
      id,
      title: date,
      deletedAt,
      deletedBy,
      mood: mood && MOODS.includes(mood) ? mood : null,
    }));
  }

  async restoreEntry(diaryId: string, entryId: string): Promise<DiaryEntryDetail> {
    return entryDetailOf(
      await this.#request<Schemas['DiaryEntry']>(
        '/diaries/{diaryId}/entries/{entryId}/restore',
        { diaryId, entryId },
        { method: 'POST' },
      ),
    );
  }
}
