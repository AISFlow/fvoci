/**
 * The calendar of a diary: the written days of the month it shows, each with
 * its mood, and the runs of written days that the days written in a row are
 * counted by (`logic/diary-streak.ts`, on the reader's today).
 *
 * The month is asked for alone (`Diaries.days`), the runs of the diary at once
 * (`Diaries.runs`): neither reads an entry. What changes in the diary is heard
 * by its store (`DiaryStore`) and passed on here: an entry made or changed is
 * put in its day (a change carries its day and mood), and the runs are read
 * again when a day is written that no run holds, or an entry is deleted.
 */
import { calendarDate, type CalendarDate } from '../logic/dates.ts';
import { isBackendError } from './backend.ts';
import type { DiaryDay, DiaryEntry, DiaryRun } from './diary-types.ts';
import type { Workspace } from './workspace.svelte.ts';

/** What the calendar needs of its diary's store. */
export interface CalendarHost {
  readonly workspace: Workspace;
  readonly diaryId: string;
}

/** The first and the last day of the month of `date`. */
export function monthAround(date: CalendarDate): { from: CalendarDate; to: CalendarDate } {
  const [year, month] = [Number(date.slice(0, 4)), Number(date.slice(5, 7))];
  return { from: calendarDate(year, month, 1), to: calendarDate(year, month + 1, 0) };
}

export class DiaryCalendar {
  readonly #host: CalendarHost;

  /** The first day of the month shown; undefined before one is. */
  month = $state.raw<CalendarDate | undefined>(undefined);
  /** The written days of that month, the earliest first. */
  days = $state.raw<readonly DiaryDay[]>([]);
  /** Every run of written days of the diary, the earliest first. */
  runs = $state.raw<readonly DiaryRun[]>([]);
  /** The runs were read: until then the counts are not known. */
  counted = $state(false);

  #monthReads = 0;
  #counting: Promise<void> | undefined;
  #countAgain = false;
  /** `resync` asked for a count while one was on its way: retry even if that one fails. */
  #resyncCount = false;
  /** The last count was refused: wait for `resync` instead of asking again on every edit. */
  #refused = false;

  constructor(host: CalendarHost) {
    this.#host = host;
  }

  /** Shows the month of `date`: its written days are read. */
  async showMonth(date: CalendarDate): Promise<void> {
    const { from, to } = monthAround(date);
    if (this.month !== from) [this.month, this.days] = [from, []];
    const diaries = this.#host.workspace.backend.diary;
    if (!diaries) return;
    this.#monthReads += 1;
    const reading = this.#monthReads;
    try {
      const days = await diaries.days(this.#host.diaryId, from, to);
      if (reading === this.#monthReads && this.month === from) this.days = days;
    } catch (error) {
      this.#failed(error);
    }
  }

  /** Reads the runs; asked again while they are read, they are read once more after. */
  count(force = false): Promise<void> {
    if (this.#counting) {
      if (force) this.#resyncCount = true;
      else this.#countAgain = true;
      return this.#counting;
    }
    this.#counting = this.#count().finally(() => {
      this.#counting = undefined;
      const retry = this.#resyncCount || (this.#countAgain && !this.#refused);
      this.#resyncCount = false;
      this.#countAgain = false;
      if (retry) void this.count();
    });
    return this.#counting;
  }

  async #count(): Promise<void> {
    const diaries = this.#host.workspace.backend.diary;
    if (!diaries) return;
    try {
      [this.runs, this.counted, this.#refused] = [
        await diaries.runs(this.#host.diaryId),
        true,
        false,
      ];
    } catch (error) {
      this.#refused = true;
      this.#failed(error);
    }
  }

  /** Entries made or changed: their days, and the runs where a day no run holds is written. */
  heard(entries: readonly DiaryEntry[]): void {
    const month = this.month ? monthAround(this.month) : undefined;
    let days = this.days;
    for (const entry of entries) {
      if (month && entry.date >= month.from && entry.date <= month.to) {
        const day: DiaryDay = { date: entry.date, entryId: entry.id, mood: entry.mood };
        days = [...days.filter((one) => one.date !== entry.date), day].sort((first, second) =>
          first.date.localeCompare(second.date),
        );
      }
    }
    if (days !== this.days) this.days = days;
    const held = (date: CalendarDate): boolean =>
      this.runs.some((run) => run.from <= date && date <= run.to);
    // Being read: read again after, not to miss what was heard. A count refused
    // waits for what is missed to be read (`resync`), not for every key typed.
    if (
      this.#counting ||
      (this.counted && !this.#refused && entries.some((entry) => !held(entry.date)))
    ) {
      void this.count();
    }
  }

  /** Blocks of the diary deleted: an entry's day is no longer written. */
  heardDeleted(blockIds: readonly string[]): void {
    const days = this.days.filter((day) => !blockIds.includes(day.entryId));
    if (days.length !== this.days.length) this.days = days;
    if (!this.#refused) void this.count();
  }

  /** What was missed is read again. */
  resync(): void {
    if (this.month) void this.showMonth(this.month);
    void this.count(true);
  }

  #failed(error: unknown): void {
    // A diary not there says so on its page (`DiaryStore.missing`).
    if (isBackendError(error) && (error.status === 404 || error.status === 403)) return;
    this.#host.workspace.report('load', error);
  }
}
