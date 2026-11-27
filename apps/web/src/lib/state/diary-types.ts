/**
 * The diaries of the reader's own space and their entries, part of the data
 * boundary (`backend.ts`, `Backend.diary`).
 *
 * A diary is a board of the kind `diary` (`Container.kind`), only ever in the
 * reader's own space: it is renamed, deleted, brought back, filed in the
 * sidebar, exported and searched as a board is, by the board's operations.
 * Nobody else is on it: no members, no public link, no copies. What only a
 * diary has is here: its entries, one a day at most, each with a body
 * written together in its room as a card's is (`Backend.openDoc` with the
 * entry in place of the card).
 *
 * The day of an entry is the day where its writer was, as their device has it:
 * the server never decides what today is. It is what the entry is made with
 * and never changes.
 */
import type { DocNode } from '../editor/schema.ts';
import type { CalendarDate } from '../logic/dates.ts';
import type { MonthDay } from '../logic/diary-days.ts';
import type { Container } from '../logic/wire.ts';
import type { BodyVersions } from './body-versions.ts';
import type { DeletedItem, Paging } from './trash-reads.ts';

/** How the day was, as its writer marks it. */
export type Mood = 'great' | 'good' | 'okay' | 'bad' | 'awful';

/** The moods, the best first: the order a writer picks from. */
export const MOODS: readonly Mood[] = ['great', 'good', 'okay', 'bad', 'awful'];

/** The most characters of the weather and of the place of a day (`ENTRY_NOTE_MAX_CHARS`). */
export const NOTE_MAX_CHARS = 100;

/** What a day carries besides its body: none of it is searched. */
export interface DayNotes {
  mood: Mood | null;
  /** Empty for none. */
  weather: string;
  /** Empty for none. */
  place: string;
}

/** A day of a diary that has an entry, as its calendar marks it: no more of the entry. */
export interface DiaryDay {
  date: CalendarDate;
  entryId: string;
  mood: Mood | null;
}

/**
 * Days of a diary one after the other, each written: what the days written in
 * a row are counted by (`logic/diary-streak.ts`), on the reader's today.
 */
export interface DiaryRun {
  from: CalendarDate;
  to: CalendarDate;
  /** Both ends counted. */
  days: number;
}

/** An entry as a change brings it: without its body. */
export interface DiaryEntry extends DayNotes {
  id: string;
  diaryId: string;
  date: CalendarDate;
  createdAt: number;
  updatedAt: number;
}

/** An entry read on its own or with its page: with its body. */
export interface DiaryEntryDetail extends DiaryEntry {
  body: DocNode | null;
  createdBy: string;
  modifiedBy: string;
}

/** What an entry is changed to; what is left out stays. */
export interface EntryPatch extends Partial<DayNotes> {
  /** A whole new body, sent with `readAt`. Cannot be combined with `appendBody`. */
  body?: DocNode | null;
  /** Blocks appended after the entry's current body inside one server transaction. */
  appendBody?: DocNode | null;
  /** The `updatedAt` the entry was read at: one whose body changed since is refused (409). */
  readAt?: number;
}

export interface Diaries {
  /** Versions kept of each entry's body (`getDiaryEntryBodyVersions`, `keepDiaryEntryBodyVersion`, etc.). */
  readonly bodyVersions: BodyVersions;
  /** The diaries of the reader's own space, oldest first; none of the trash. 404 for any other workspace. */
  diaries(workspaceId: string): Promise<Container[]>;
  /**
   * Makes a diary in the reader's own space, filed under their default
   * category; past the space's limit it is 400 (`too_many_diaries`).
   * `ifNone`: only where the space has none, else its oldest is answered.
   */
  createDiary(workspaceId: string, draft: { title?: string; ifNone?: boolean }): Promise<Container>;
  /** The entries of the diary on `monthDay` of every year, with their bodies, in no promised order. */
  entriesOn(diaryId: string, monthDay: MonthDay): Promise<DiaryEntryDetail[]>;
  /** The written days from `from` to `to`, both included, the earliest first; a year (366 days) at most, else 400. */
  days(diaryId: string, from: CalendarDate, to: CalendarDate): Promise<DiaryDay[]>;
  /** Every run of written days of the diary, the earliest first. */
  runs(diaryId: string): Promise<DiaryRun[]>;
  /**
   * Makes the entry of a day; where the day has one, that one is answered and
   * nothing is made. A day that is no day, or more than two days past the
   * server's, is 400 at `date`.
   */
  createEntry(diaryId: string, date: CalendarDate): Promise<DiaryEntryDetail>;
  entry(diaryId: string, entryId: string): Promise<DiaryEntryDetail>;
  /**
   * A mood, weather or place is set as it is sent, whatever was there; a whole
   * `body` requires `readAt` (compared against the entry's body change time, so
   * metadata-only edits do not conflict); `appendBody` appends after whatever
   * body is stored inside one transaction. Weather and place longer than
   * `NOTE_MAX_CHARS` are 400.
   * `keepalive`: sent as the page goes, and still sent once it has gone.
   */
  patchEntry(
    diaryId: string,
    entryId: string,
    patch: EntryPatch,
    options?: { keepalive?: boolean },
  ): Promise<DiaryEntryDetail>;
  /**
   * Removes the entry so its day takes a new one: completely empty entries
   * (no body, mood, weather, or place) are purged directly instead of piling
   * up in the trash; entries with any content move to the diary's trash.
   */
  deleteEntry(diaryId: string, entryId: string, options?: { keepalive?: boolean }): Promise<void>;
  /**
   * The diary's entries in the trash, the last deleted first: a page. Each is
   * titled with its day (`2026-11-21`) and has its mood.
   */
  deletedEntries(diaryId: string, paging?: Paging): Promise<DeletedItem[]>;
  /**
   * Brings an entry back to its day with its body. Where the day has an entry
   * since, 409 with the reason `day_taken` and nothing changes; where the
   * diary has as many as it may, 400 `too_many_entries`; 404 if the trash has
   * no such entry.
   */
  restoreEntry(diaryId: string, entryId: string): Promise<DiaryEntryDetail>;
}

/** Where a backend keeps its diaries. One by a public link has none. */
export interface DiariesOfBackend {
  readonly diary?: Diaries;
}
