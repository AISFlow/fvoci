/**
 * The notes of a day (its mood, weather and place) as the reader changes them
 * on its page (`EntryStore.mark`).
 *
 * A change is the day's at once, before it is sent: it is what the page shows
 * and what says whether the day is blank (`isBlank`). It is sent on a patch of
 * its own, without a version: a mood at once, what is typed once the typing
 * pauses or the field is left. What one device sent last is what the entry
 * keeps, whichever device sent it. A change not answered yet stays over what
 * is heard meanwhile (`over`), so another device's change does not take back
 * what is being typed here; one refused waits to be sent with the next.
 *
 * While a draft waits to be written as the body (`NotesHost.held`), nothing is
 * sent alone: the draft's write reads the entry as it is, and a patch of the
 * notes landing first would make it find the entry changed. The draft's write
 * carries them (`unanswered`, then `carried`). Every patch goes with
 * `keepalive`, so that one on its way when the page goes still arrives.
 */
import { isBackendError } from './backend.ts';
import type { DayNotes, Diaries, DiaryEntryDetail } from './diary-types.ts';

/** How long after the last key a weather or a place is sent. */
export const TYPING_PAUSE = 800;

/** What the notes need of the entry they are of. */
export interface NotesHost {
  readonly diaries: Diaries | undefined;
  readonly diaryId: string;
  /** The entry they are sent to, once there is one. */
  readonly entryId: string | undefined;
  /** A draft waits to be written as the body: the notes go with it, not alone. */
  readonly held: boolean;
  /** A patch was answered with the entry it made. */
  written(entry: DiaryEntryDetail): void;
  /** A patch was refused; what it held waits to be sent again. */
  refused(error: unknown): void;
}

const NONE: Partial<DayNotes> = {};

/** What of `notes` holds something: a mood taken away, or an emptied weather or place, is dropped. */
function withoutBlank(notes: Partial<DayNotes>): Partial<DayNotes> {
  const entries = Object.entries(notes).filter(([, value]) => value !== null && value !== '');
  return entries.length > 0 ? Object.fromEntries(entries) : NONE;
}

export class EntryNotes {
  readonly #host: NotesHost;
  /** Changed here and not sent yet. */
  #waiting = $state.raw<Partial<DayNotes>>(NONE);
  /** On its way. */
  #sending = $state.raw<Partial<DayNotes>>(NONE);
  /** Which write owns `#sending`: a newer write (e.g. `leave`) leaves an older one's settlement behind. */
  #generation = 0;
  #timer: ReturnType<typeof setTimeout> | undefined;

  constructor(host: NotesHost) {
    this.#host = host;
  }

  /** Something changed here is not answered yet. */
  get pending(): boolean {
    return this.#waiting !== NONE || this.#sending !== NONE;
  }

  /** Notes as they were read or heard, with what is not answered yet over them. */
  over<T extends DayNotes>(notes: T): T {
    return this.pending ? { ...notes, ...this.#sending, ...this.#waiting } : notes;
  }

  /** The reader changed notes: sent now, or once the typing pauses (`later`). */
  change(change: Partial<DayNotes>, when: 'now' | 'later'): void {
    this.#waiting = { ...this.#waiting, ...change };
    clearTimeout(this.#timer);
    if (when === 'now') void this.send();
    else
      this.#timer = setTimeout(() => {
        void this.send();
      }, TYPING_PAUSE);
  }

  /**
   * Sends what waits, once there is an entry, no draft holds it and nothing
   * else is on its way; what is changed meanwhile goes after it.
   */
  async send(): Promise<void> {
    clearTimeout(this.#timer);
    const { diaries, diaryId, entryId, held } = this.#host;
    const sending = this.#waiting;
    if (!diaries || !entryId || held || this.#sending !== NONE || sending === NONE) return;
    await this.#dispatch(diaries, diaryId, entryId, sending, true);
  }

  async #dispatch(
    diaries: Diaries,
    diaryId: string,
    entryId: string,
    sending: Partial<DayNotes>,
    report: boolean,
  ): Promise<void> {
    this.#generation += 1;
    const generation = this.#generation;
    [this.#sending, this.#waiting] = [sending, NONE];
    let written: DiaryEntryDetail;
    try {
      written = await diaries.patchEntry(diaryId, entryId, sending, { keepalive: true });
    } catch (error) {
      if (generation !== this.#generation) return;
      const gone = isBackendError(error) && error.status === 404;
      // An entry deleted elsewhere takes the notes to the one made next (`EntryStore.lose`),
      // but not keys that hold nothing: they were for it, and would clear what another holds.
      const back = gone ? withoutBlank(sending) : sending;
      const later = this.#waiting;
      [this.#sending, this.#waiting] = [NONE, back === NONE ? later : { ...back, ...later }];
      if (!gone && report) this.#host.refused(error);
      if (gone && this.#waiting !== NONE && this.#host.entryId && this.#host.entryId !== entryId) {
        await this.send();
      }
      return;
    }
    if (generation !== this.#generation) return;
    this.#sending = NONE;
    this.#host.written(written);
    if (this.#waiting !== NONE) await this.send();
  }

  /**
   * Everything changed here and not answered yet, on its way or waiting, for a
   * write that carries it (a draft's, one as the page goes); undefined where
   * there is none. It waits until that write is answered (`carried`).
   */
  unanswered(): Partial<DayNotes> | undefined {
    clearTimeout(this.#timer);
    return this.pending ? { ...this.#sending, ...this.#waiting } : undefined;
  }

  /**
   * A write that carried `notes` (`unanswered`) was answered: what waits is
   * answered where it is still what was carried.
   */
  carried(notes: Partial<DayNotes>): void {
    const left = Object.entries(this.#waiting).filter(
      ([key, value]) => notes[key as keyof DayNotes] !== value,
    );
    if (left.length !== Object.keys(this.#waiting).length) {
      this.#waiting = left.length > 0 ? Object.fromEntries(left) : NONE;
    }
  }

  /**
   * The page goes: what is not answered goes now in one write, to arrive still
   * once it has gone; a page kept to come back to hears the answer. It takes
   * over from any write still on its way so that write's earlier answer does
   * not resend what this one carried; held by a draft whose write is on its way
   * already, it goes alone.
   */
  async leave(): Promise<void> {
    const { diaries, diaryId, entryId, held } = this.#host;
    const notes = this.unanswered();
    if (!diaries || !entryId || !notes) return;
    if (this.#sending === NONE && !held) return this.send();
    await this.#dispatch(diaries, diaryId, entryId, notes, false);
  }

  /**
   * The entry is first here (made, or heard of), or went: keys that wait and
   * hold nothing were for none, or for the one gone, and are not sent.
   */
  forgetBlank(): void {
    if (this.#waiting === NONE) return;
    const left = withoutBlank(this.#waiting);
    if (left === NONE) {
      clearTimeout(this.#timer);
      this.#waiting = NONE;
    } else if (Object.keys(left).length !== Object.keys(this.#waiting).length) {
      this.#waiting = left;
    }
  }

  /** Nothing is sent by itself any more: the page goes. */
  stop(): void {
    clearTimeout(this.#timer);
  }
}
