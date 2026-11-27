/**
 * The entry of one day of a diary as its page has it open: its body, edited
 * together in its room via `CollaborativeBodyState`, its pre-entry draft, and
 * its day notes (`EntryNotes`).
 */
import type { DocNode } from '../editor/schema.ts';
import type { CalendarDate } from '../logic/dates.ts';
import { BackendError, isBackendError, type BodyDocState } from './backend.ts';
import { BodyUploads } from './body-uploads.svelte.ts';
import { CollaborativeBodyState } from './collab-body.svelte.ts';
import type { DayNotes, Diaries, DiaryEntry, DiaryEntryDetail } from './diary-types.ts';
import { EntryNotes, type NotesHost } from './entry-notes.svelte.ts';
import type { Workspace } from './workspace.svelte.ts';

export interface EntryHost {
  readonly workspace: Workspace;
  readonly diaryId: string;
  received(entry: DiaryEntryDetail): void;
  removed(entryId: string): void;
  catchUpEntry?: (entryId: string) => void;
}

export function isBlank(entry: DayNotes, blankBody: boolean): boolean {
  return blankBody && entry.mood === null && entry.weather === '' && entry.place === '';
}

const NO_NOTES: DayNotes = { mood: null, weather: '', place: '' };
const MAKE_AGAIN_AFTER = 5000;

function passing(error: unknown): boolean {
  return (
    isBackendError(error) && (error.status === 0 || error.status === 429 || error.status >= 500)
  );
}

export class EntryStore {
  readonly host: EntryHost;
  readonly date: CalendarDate;

  entry = $state.raw<DiaryEntryDetail | undefined>(undefined);
  draft = $state.raw<DocNode | null>(null);
  drafting = $state(false);
  unsaved = $state(false);
  flushing = $state(false);

  #wanted = false;
  #making: Promise<void> | undefined;
  #makeAgainAt = 0;
  #again: ReturnType<typeof setTimeout> | undefined;
  #changedHere = false;
  #blankBody = false;
  #closed = false;
  #missedBodyEntryId: string | undefined;
  readonly #notes: EntryNotes;
  readonly #collab: CollaborativeBodyState;
  readonly #bodyUploads: BodyUploads;

  constructor(host: EntryHost, date: CalendarDate, entry?: DiaryEntryDetail) {
    this.host = host;
    this.date = date;
    this.entry = entry;
    this.#notes = new EntryNotes(this.#notesHost());
    this.#bodyUploads = new BodyUploads(() => this.host.workspace);
    this.#collab = new CollaborativeBodyState({
      workspace: host.workspace,
      probeStillThere: async () => {
        const current = this.entry;
        const diaries = this.host.workspace.backend.diary;
        if (!current || !diaries) throw new BackendError(404, { detail: 'gone' });
        return diaries.entry(this.host.diaryId, current.id);
      },
      onGone: () => {
        this.lose();
      },
      onState: (state) => {
        if (state.pending) this.#changedHere = true;
      },
      onDocReleased: () => {
        this.#catchUp();
      },
    });
  }

  get room(): BodyDocState | undefined {
    return this.#collab.room;
  }

  get lost(): readonly string[] {
    return this.#collab.lost;
  }

  get hasRoomDoc(): boolean {
    return !this.#closed && this.#collab.hasDoc;
  }

  #notesHost(): NotesHost {
    const host = this.host;
    const entry = (): DiaryEntryDetail | undefined => this.entry;
    const drafted = (): boolean => this.draft !== null && !this.unsaved;
    return {
      get diaries() {
        return host.workspace.backend.diary;
      },
      diaryId: host.diaryId,
      get entryId() {
        return entry()?.id;
      },
      get held() {
        return drafted();
      },
      written: (written) => {
        const held = this.entry;
        if (held?.id !== written.id) return;
        this.entry = this.#notes.over(held.updatedAt > written.updatedAt ? held : written);
        host.received(this.entry);
      },
      refused: (error) => {
        host.workspace.report('save', error);
      },
    };
  }

  get notes(): DayNotes {
    return this.#notes.over(this.entry ?? NO_NOTES);
  }

  mark(change: Partial<DayNotes>, when: 'now' | 'later' = 'now'): void {
    this.#notes.change(change, when);
    this.#changedHere = true;
    const entry = this.entry;
    if (!entry) {
      this.#makeIfNone();
      return;
    }
    this.entry = this.#notes.over(entry);
    this.host.received(this.entry);
  }

  sendNotes(): void {
    void this.#notes.send();
  }

  #wantsEntry(): boolean {
    return this.draft !== null || !isBlank(this.#notes.over(NO_NOTES), true);
  }

  get body(): DocNode | null {
    return this.entry?.body ?? null;
  }

  /** Applies an authoritative restored `DiaryEntryDetail` immediately (W1.4). */
  applyRestored(detail: DiaryEntryDetail): void {
    if (this.#closed || detail.date !== this.date) return;
    this.entry = this.#notes.over(detail);
    this.host.received(this.entry);
  }

  /**
   * Updates metadata from a live `entries` change while the room already holds
   * the Yjs document (`hasRoomDoc`), reading the live body from `room.doc` in
   * memory rather than re-downloading over HTTP (§1.3.3).
   */
  adoptMeta(meta: DiaryEntry): void {
    if (meta.date !== this.date || this.#closed || this.entry?.id !== meta.id) return;
    if (this.entry.updatedAt >= meta.updatedAt) return;
    this.#missedBodyEntryId = meta.id;
    this.entry = this.#notes.over({ ...this.entry, ...meta });
    this.host.received(this.entry);
    const doc = this.room?.doc;
    if (doc) this.#syncBodyFromDoc(doc);
  }

  #syncBodyFromDoc(doc: NonNullable<BodyDocState['doc']>): void {
    const entryId = this.entry?.id;
    if (!entryId || this.#closed) return;
    void import('../editor/room-body.ts').then(({ bodyOfDoc }) => {
      if (this.#closed || this.entry?.id !== entryId || (this.room?.doc && this.room.doc !== doc)) {
        return;
      }
      const body = bodyOfDoc(doc);
      if (body === undefined) return;
      this.entry = this.#notes.over({ ...this.entry, body });
      this.host.received(this.entry);
    });
  }

  #catchUp(): void {
    const entryId = this.#missedBodyEntryId;
    if (!entryId || this.#closed) return;
    if (this.room?.doc) {
      this.#syncBodyFromDoc(this.room.doc);
      return;
    }
    this.#missedBodyEntryId = undefined;
    if (this.entry?.id === entryId) this.host.catchUpEntry?.(entryId);
  }

  adopt(entry: DiaryEntryDetail): void {
    if (entry.date !== this.date || this.#closed) return;
    const held = this.entry;
    if (held?.id === entry.id && held.updatedAt > entry.updatedAt) return;
    if (held && held.id !== entry.id) this.lose();
    const first = this.entry === undefined;
    if (first) this.#notes.forgetBlank();
    this.#missedBodyEntryId = undefined;
    this.entry = this.#notes.over(entry);
    if (first) void this.#notes.send();
    if (first && this.#wanted) this.openBody();
  }

  lose(): void {
    const opened = this.#collab.detach();
    [this.entry, this.#changedHere, this.#blankBody, this.#missedBodyEntryId] = [
      undefined,
      false,
      false,
      undefined,
    ];
    opened?.close();
    this.#notes.forgetBlank();
    if (this.#wantsEntry() && !this.#closed) this.#makeIfNone();
  }

  write(draft: DocNode | null): void {
    this.draft = draft;
    this.drafting = true;
    if (draft !== null) this.#makeIfNone();
    else void this.#notes.send();
  }

  #makeIfNone(): void {
    if (this.entry || this.#making || Date.now() < this.#makeAgainAt) return;
    this.#making = this.#make().finally(() => {
      this.#making = undefined;
    });
  }

  async #make(): Promise<void> {
    const diaries = this.host.workspace.backend.diary;
    if (!diaries) return;
    try {
      const made = await diaries.createEntry(this.host.diaryId, this.date);
      this.#notes.forgetBlank();
      const entry = this.#notes.over(made);
      this.host.received(entry);
      this.unsaved = false;
      if (this.entry === undefined) {
        [this.entry, this.#changedHere] = [entry, true];
      }
      const noted = this.#notes.send();
      if (this.#closed || !this.#wanted) {
        await noted;
        await this.#flush();
      } else this.openBody();
    } catch (error) {
      this.#makeAgainAt = Date.now() + MAKE_AGAIN_AFTER;
      if (!this.unsaved) this.host.workspace.report('save', error);
      this.unsaved = true;
      if (!passing(error)) return;
      this.#again = setTimeout(() => {
        [this.#again, this.#makeAgainAt] = [undefined, 0];
        if (!this.#closed && this.#wantsEntry()) this.#makeIfNone();
      }, MAKE_AGAIN_AFTER);
    }
  }

  takeDraft(): DocNode | null {
    if (this.flushing) return null;
    const draft = this.draft;
    [this.draft, this.drafting] = [null, false];
    void this.#notes.send();
    return draft;
  }

  noteBlank(blank: boolean): void {
    this.#blankBody = blank;
  }

  // --------------------------------------------------------------- body

  openBody(): void {
    this.#wanted = true;
    const entryId = this.entry?.id;
    if (this.#closed || !entryId) return;
    this.#collab.open(
      this.host.workspace.backend,
      this.host.workspace.info?.id,
      this.host.diaryId,
      entryId,
    );
  }

  #emptied(): DiaryEntryDetail | undefined {
    const entry = this.entry;
    const room = this.room;
    const alone = (room?.carets?.getStates().size ?? 0) <= 1;
    const held = this.draft !== null || this.flushing;
    const left = this.#changedHere && alone && !held;
    const blank = entry && isBlank(this.#notes.over(entry), this.#blankBody);
    return entry && room?.doc && left && blank ? entry : undefined;
  }

  closeBody(): void {
    this.#wanted = false;
    const emptied = this.#emptied();
    const closingDoc = !emptied ? this.room?.doc : undefined;
    const opened = this.#collab.detach();
    if (closingDoc) this.#syncBodyFromDoc(closingDoc);
    if (emptied) this.#notes.stop();
    else void this.#notes.send();
    [this.#changedHere, this.#blankBody, this.#missedBodyEntryId] = [false, false, undefined];
    if (this.draft !== null && this.entry) void this.#flush();
    if (!opened) return;
    if (!emptied) {
      opened.close();
      return;
    }
    void opened.settle().then(() => {
      opened.close();
      if (this.#collab.isOpen || this.#wanted) {
        if (this.entry?.id === emptied.id) this.#changedHere = true;
        void this.#notes.send();
        return undefined;
      }
      return this.#delete(emptied);
    });
  }

  leave(persisted: boolean): void {
    void this.#collab.settle();
    const diaries = this.host.workspace.backend.diary;
    const emptied = this.#emptied();
    if (!diaries || !this.entry) return;
    if (this.draft !== null && !this.flushing) void this.#flush({ keepalive: true });
    else if (emptied && !persisted) {
      void diaries
        .deleteEntry(this.host.diaryId, emptied.id, { keepalive: true })
        .catch(() => undefined);
    } else void this.#notes.leave();
  }

  close(): void {
    this.closeBody();
    this.#closed = true;
    clearTimeout(this.#again);
    this.#notes.stop();
    if (this.#wantsEntry() && !this.entry && !this.#making) {
      this.#makeAgainAt = 0;
      this.#makeIfNone();
    }
    this.#bodyUploads.giveUp();
  }

  async #flush(options: { keepalive?: boolean } = {}): Promise<void> {
    const entry = this.entry;
    const diaries = this.host.workspace.backend.diary;
    const draft = this.draft;
    if (!entry || !diaries || draft === null || this.flushing) return;
    this.flushing = true;
    const notes = this.#notes.unanswered() ?? {};
    try {
      const written = await this.#writeDraft(diaries, entry, { draft, notes }, options);
      if (this.entry?.id !== entry.id) return;
      this.#notes.carried(notes);
      if (this.draft === draft) [this.draft, this.drafting] = [null, false];
      [this.entry, this.unsaved, this.#changedHere] = [this.#notes.over(written), false, false];
      this.host.received(this.entry);
      void this.#notes.send();
    } catch (error) {
      if (this.entry?.id !== entry.id) return;
      this.unsaved = true;
      this.host.workspace.report('save', error);
      void this.#notes.send();
    } finally {
      this.flushing = false;
    }
  }

  #writeDraft(
    diaries: Diaries,
    entry: DiaryEntryDetail,
    { draft, notes }: { draft: DocNode; notes: Partial<DayNotes> },
    options: { keepalive?: boolean },
  ): Promise<DiaryEntryDetail> {
    return diaries.patchEntry(
      this.host.diaryId,
      entry.id,
      { ...notes, appendBody: draft },
      options,
    );
  }

  async #delete(entry: DiaryEntryDetail): Promise<void> {
    try {
      await this.host.workspace.backend.diary?.deleteEntry(this.host.diaryId, entry.id);
      if (this.entry?.id === entry.id) this.lose();
      this.host.removed(entry.id);
    } catch (error) {
      if (isBackendError(error) && error.status === 404) return;
      if (this.entry?.id === entry.id) this.#changedHere = true;
      this.host.workspace.report('save', error);
      void this.#notes.send();
    }
  }

  settleBody(): Promise<void> {
    return this.#collab.settle();
  }

  retryBody(): void {
    this.#collab.retry();
  }

  sayLost(why: string): void {
    this.#collab.sayLost(why);
  }

  dismissLost(): void {
    this.#collab.dismissLost();
  }

  uploadFile(file: File): Promise<string> {
    return this.#bodyUploads.run(file, (options) =>
      this.host.workspace.backend.upload(this.host.diaryId, file, options),
    );
  }
}
