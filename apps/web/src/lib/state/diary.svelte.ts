/**
 * One diary as its page shows it: the entries of one day of the year in every
 * year (`show`), each day's entry opened through an `EntryStore`.
 */
import type { CalendarDate } from '../logic/dates.ts';
import { monthDayOf, type MonthDay } from '../logic/diary-days.ts';
import type { Container } from '../logic/wire.ts';
import { isBackendError, type Change } from './backend.ts';
import { DiaryCalendar } from './diary-calendar.svelte.ts';
import type { DiaryEntry, DiaryEntryDetail } from './diary-types.ts';
import { EntryStore, type EntryHost } from './entry.svelte.ts';
import type { LoadStatus, Workspace } from './workspace.svelte.ts';

export class DiaryStore implements EntryHost {
  readonly kind = 'diary' as const;
  readonly workspace: Workspace;
  readonly diaryId: string;

  status = $state<LoadStatus>('idle');
  missing = $state(false);
  monthDay = $state.raw<MonthDay | undefined>(undefined);
  entries = $state.raw<readonly DiaryEntryDetail[]>([]);
  readonly calendar: DiaryCalendar;

  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown: each store is
  readonly #days = new Map<CalendarDate, EntryStore>();
  #stop: (() => void) | undefined;
  #readings = 0;
  #deletions = 0;
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- consulted, never shown
  readonly #goneAt = new Map<string, number>();

  constructor(workspace: Workspace, diaryId: string) {
    this.workspace = workspace;
    this.diaryId = diaryId;
    this.calendar = new DiaryCalendar(this);
  }

  get diary(): Container | undefined {
    return this.workspace.containers.find((container) => container.id === this.diaryId);
  }

  async show(monthDay: MonthDay): Promise<void> {
    if (!this.#stop) {
      this.#stop = this.workspace.backend.watchContainer(this.diaryId, (change) => {
        this.#hear(change);
      });
      void this.calendar.count();
    }
    if (this.monthDay !== monthDay) {
      for (const day of this.#days.values()) day.close();
      this.#days.clear();
      this.entries = [];
      this.monthDay = monthDay;
      this.status = 'loading';
    }
    await this.#read(monthDay);
  }

  async #read(monthDay: MonthDay): Promise<void> {
    const diaries = this.workspace.backend.diary;
    if (!diaries) return;
    this.#readings += 1;
    const reading = this.#readings;
    const asked = this.#deletions;
    try {
      const read = await diaries.entriesOn(this.diaryId, monthDay);
      if (reading !== this.#readings || this.monthDay !== monthDay) return;
      const entries = read.filter((entry) => (this.#goneAt.get(entry.id) ?? 0) <= asked);
      this.entries = entries;
      for (const entry of entries) this.#days.get(entry.date)?.adopt(entry);
      for (const day of this.#days.values()) {
        const held = day.entry;
        if (held && !entries.some((one) => one.id === held.id)) this.#gone(held);
      }
      [this.status, this.missing] = ['ready', false];
    } catch (error) {
      if (reading !== this.#readings) return;
      if (isBackendError(error) && (error.status === 404 || error.status === 403)) {
        [this.status, this.missing] = ['failed', true];
      } else {
        this.status = 'failed';
        this.workspace.report('load', error);
      }
    }
  }

  day(date: CalendarDate): EntryStore {
    let day = this.#days.get(date);
    if (!day) {
      day = new EntryStore(
        this,
        date,
        this.entries.find((entry) => entry.date === date),
      );
      this.#days.set(date, day);
    }
    return day;
  }

  received(entry: DiaryEntryDetail): void {
    this.calendar.heard([entry]);
    if (monthDayOf(entry.date) !== this.monthDay) return;
    this.entries = [...this.entries.filter((one) => one.id !== entry.id), entry];
  }

  removed(entryId: string): void {
    this.#deletions += 1;
    this.#goneAt.set(entryId, this.#deletions);
    this.entries = this.entries.filter((one) => one.id !== entryId);
  }

  /**
   * Brings an entry back from the diary's trash to its day, shown there at
   * once; refused (`day_taken`: the day has an entry since), it throws.
   */
  async restoreEntry(entryId: string): Promise<void> {
    const diaries = this.workspace.backend.diary;
    if (!diaries) return;
    const entry = await diaries.restoreEntry(this.diaryId, entryId);
    this.#goneAt.delete(entryId);
    this.received(entry);
    this.#days.get(entry.date)?.adopt(entry);
  }

  catchUpEntry(entryId: string): void {
    this.#fetchEntry(entryId);
  }

  #hear(change: Change): void {
    switch (change.kind) {
      case 'entries':
        this.calendar.heard(change.entries);
        for (const entry of change.entries) this.#changed(entry);
        return;
      case 'blocksDeleted':
        this.calendar.heardDeleted(change.blockIds);
        for (const id of change.blockIds) this.#deleted(id);
        return;
      case 'containerDeleted':
        if (change.containerId === this.diaryId) [this.status, this.missing] = ['failed', true];
        return;
      case 'resync':
        this.calendar.resync();
        if (this.monthDay) void this.#read(this.monthDay);
        return;
      default:
    }
  }

  /**
   * An entry made or changed: when the open `EntryStore` for that day already
   * holds the active Yjs document (`hasRoomDoc`), reads the live body from the
   * room's `Y.Doc` in memory instead of re-downloading it over HTTP (§1.3.3).
   */
  #changed(entry: DiaryEntry): void {
    if (monthDayOf(entry.date) !== this.monthDay) return;
    const shown = this.entries.find((one) => one.id === entry.id);
    if (shown && shown.updatedAt >= entry.updatedAt) return;
    const day = this.#days.get(entry.date);
    if (day?.entry?.id === entry.id && day.hasRoomDoc) {
      day.adoptMeta(entry);
      return;
    }
    this.#fetchEntry(entry.id);
  }

  #fetchEntry(entryId: string): void {
    const asked = this.#deletions;
    void this.workspace.backend.diary?.entry(this.diaryId, entryId).then(
      (detail) => {
        const now = this.entries.find((one) => one.id === detail.id);
        if (now && now.updatedAt > detail.updatedAt) return;
        if ((this.#goneAt.get(detail.id) ?? 0) > asked) return;
        this.received(detail);
        this.#days.get(detail.date)?.adopt(detail);
      },
      () => undefined,
    );
  }

  #deleted(entryId: string): void {
    this.removed(entryId);
    for (const day of this.#days.values()) if (day.entry?.id === entryId) day.lose();
  }

  #gone(entry: DiaryEntryDetail): void {
    const asked = this.#deletions;
    void this.workspace.backend.diary?.entry(this.diaryId, entry.id).then(
      (still) => {
        if ((this.#goneAt.get(still.id) ?? 0) > asked) return;
        const now = this.entries.find((one) => one.id === still.id);
        if (now && now.updatedAt > still.updatedAt) return;
        this.received(still);
        this.#days.get(still.date)?.adopt(still);
      },
      (error: unknown) => {
        if (isBackendError(error) && error.status === 404) {
          this.calendar.heardDeleted([entry.id]);
          this.#deleted(entry.id);
        }
      },
    );
  }

  async settleBodies(): Promise<void> {
    await Promise.all([...this.#days.values()].map((day) => day.settleBody()));
  }

  leave(persisted: boolean): void {
    for (const day of this.#days.values()) day.leave(persisted);
  }

  close(): void {
    this.#stop?.();
    this.#stop = undefined;
    for (const day of this.#days.values()) day.close();
    this.#days.clear();
  }
}

export async function firstDiary(workspace: Workspace): Promise<Container | undefined> {
  const diaries = workspace.backend.diary;
  const shown = workspace.info;
  if (!diaries || !shown) return undefined;
  try {
    const diary = await diaries.createDiary(shown.id, { ifNone: true });
    workspace.receiveContainer(diary);
    return diary;
  } catch (error) {
    workspace.report('save', error);
    return undefined;
  }
}

/** Makes another volume of diary in the reader's own space, titled `title` (empty: untitled). */
export async function newDiary(
  workspace: Workspace,
  title: string,
): Promise<Container | undefined> {
  const diaries = workspace.backend.diary;
  const shown = workspace.info;
  if (!diaries || !shown) return undefined;
  try {
    const diary = await diaries.createDiary(shown.id, title ? { title } : {});
    workspace.receiveContainer(diary);
    return diary;
  } catch (error) {
    workspace.report('save', error);
    return undefined;
  }
}
