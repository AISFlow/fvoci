/**
 * The reader's notifications as the top bar shows them: how many are unread,
 * always, and the newest of them while the list is open. Both are read again
 * when the backend says that what the reader was told changed
 * (`notifications`: something was told, or read, in this tab or another) and
 * when changes may have been missed (`resync`); the list only while it is
 * open. Of two readings, the later one counts, and a reading begun before
 * the reader read one, or all, does not count: it may not know of that; nor
 * does one that answers while a read is being written, which is read again
 * once it is.
 *
 * Who did each is named as the workspace of its card or page names them
 * (`usersById`), asked once a visit; whom it leaves out stays unnamed.
 */
import { SvelteMap } from 'svelte/reactivity';

import type { Backend, User } from './backend.ts';
import type { Notification, Notifications } from './notification-types.ts';

/** How many the list shows: the newest. */
export const LISTED = 50;

type InboxBackend = Pick<Backend, 'watchSelf' | 'usersById'>;

export class NotificationInbox {
  /** How many the reader has not read. */
  unread = $state(0);
  /** The newest, while the list is open; empty before it was read. */
  items = $state.raw<readonly Notification[]>([]);
  /** Whether the list was read since it was opened. */
  listed = $state(false);
  /** The people named so far, by identifier. */
  readonly people = new SvelteMap<string, User>();

  readonly #notifications: Notifications;
  readonly #backend: InboxBackend;
  readonly #refused: (error: unknown) => void;
  #open = false;
  #countReading = 0;
  #listReading = 0;
  /** How many reads are being written: what is read meanwhile may not know of them. */
  #writing = 0;
  /** Who was asked for already, as `workspace/user`. */
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- nothing that is drawn reads it
  readonly #asked = new Set<string>();

  /** `refused` hears what could not be read or written: a session that is over, say. */
  constructor(
    notifications: Notifications,
    backend: InboxBackend,
    refused: (error: unknown) => void,
  ) {
    this.#notifications = notifications;
    this.#backend = backend;
    this.#refused = refused;
  }

  /** Reads the count, and listens from now on; the function that stops listening. */
  start(): () => void {
    void this.#readCount();
    return this.#backend.watchSelf((change) => {
      if (change.kind === 'notifications' || change.kind === 'resync') void this.refresh();
      else if (change.kind === 'user') void this.#refreshUser(change.userId);
    });
  }

  /** Reads the count again, and the list where it is open. */
  async refresh(): Promise<void> {
    await Promise.all([this.#readCount(), this.#open ? this.#readList() : undefined]);
  }

  /** The list is shown: it is read, and kept up to date until it is closed. */
  async open(): Promise<void> {
    this.#open = true;
    await this.#readList();
  }

  /** The list is put away: opened again, it is read again before it shows. */
  close(): void {
    this.#open = false;
    this.listed = false;
  }

  /** Marks one as read, on the screen at once. */
  async read(notification: Notification): Promise<void> {
    if (notification.readAt !== 0) return;
    const asked = this.#forgetReadings();
    const now = Date.now();
    this.items = this.items.map((one) =>
      one.id === notification.id ? { ...one, readAt: now } : one,
    );
    this.unread = Math.max(0, this.unread - 1);
    const written = await this.#write(() => this.#notifications.read(notification.id));
    // Refused, it is put back; asked meanwhile, the answer did not count.
    if (!written || this.#askedSince(asked)) await this.refresh();
  }

  /**
   * Marks as read all of those listed and what came before them: what
   * happened after the newest one shown stays unread.
   */
  async readAll(): Promise<void> {
    const through = this.items.reduce<number | undefined>(
      (newest, one) => Math.max(newest ?? 0, one.updatedAt),
      undefined,
    );
    this.#forgetReadings();
    const now = Date.now();
    this.items = this.items.map((one) => (one.readAt === 0 ? { ...one, readAt: now } : one));
    this.unread = 0;
    await this.#write(() => this.#notifications.readAll(through));
    await this.refresh();
  }

  /** Writes a read; whether it was written. What is read meanwhile does not count. */
  async #write(write: () => Promise<void>): Promise<boolean> {
    this.#writing += 1;
    try {
      await write();
      return true;
    } catch (error) {
      this.#refused(error);
      return false;
    } finally {
      this.#writing -= 1;
    }
  }

  /**
   * The answers of the readings under way do not count: they may not know of a
   * read. Where the readings stand now, for {@link #askedSince}.
   */
  #forgetReadings(): number {
    this.#countReading += 1;
    this.#listReading += 1;
    return this.#countReading + this.#listReading;
  }

  /** Whether something was read since the readings stood at `asked`. */
  #askedSince(asked: number): boolean {
    return this.#countReading + this.#listReading !== asked;
  }

  async #readCount(): Promise<void> {
    this.#countReading += 1;
    const reading = this.#countReading;
    try {
      const unread = await this.#notifications.unread();
      if (reading === this.#countReading && this.#writing === 0) this.unread = unread;
    } catch (error) {
      if (reading === this.#countReading) this.#refused(error);
    }
  }

  async #readList(): Promise<void> {
    this.#listReading += 1;
    const reading = this.#listReading;
    try {
      const items = await this.#notifications.list({ page: 0, perPage: LISTED });
      if (reading !== this.#listReading || !this.#open || this.#writing > 0) return;
      this.items = items;
      this.listed = true;
      void this.#name(items);
    } catch (error) {
      if (reading === this.#listReading) this.#refused(error);
    }
  }

  /** Re-reads an actor's profile across workspaces that named them when `userChanged` arrives. */
  async #refreshUser(userId: string): Promise<void> {
    if (!this.people.has(userId)) return;
    for (const key of [...this.#asked]) {
      if (key.endsWith(`/${userId}`)) this.#asked.delete(key);
    }
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- local deduplication set
    const workspaces = new Set<string>();
    for (const { subject } of this.items) {
      if (subject?.actorId === userId && subject.workspaceId) workspaces.add(subject.workspaceId);
    }
    for (const workspaceId of workspaces) {
      try {
        const found = await this.#backend.usersById(workspaceId, [userId]);
        const updated = found.find((one) => one.id === userId);
        if (updated) {
          this.people.set(userId, updated);
          this.#asked.add(`${workspaceId}/${userId}`);
          return;
        }
      } catch {
        // Ignore transient lookup failures; next list open retries.
      }
    }
  }

  /** Asks the workspace of each for the people the list names that are not known yet. */
  async #name(items: readonly Notification[]): Promise<void> {
    const wanted: Record<string, string[]> = {};
    for (const { subject } of items) {
      if (!subject || subject.actorId === '' || this.people.has(subject.actorId)) continue;
      const key = `${subject.workspaceId}/${subject.actorId}`;
      if (this.#asked.has(key)) continue;
      this.#asked.add(key);
      (wanted[subject.workspaceId] ??= []).push(subject.actorId);
    }
    await Promise.all(
      Object.entries(wanted).map(async ([workspaceId, ids]) => {
        try {
          const found = await this.#backend.usersById(workspaceId, ids);
          for (const user of found) this.people.set(user.id, user);
        } catch {
          // Unnamed this time: asked again when the list is read again.
          for (const id of ids) this.#asked.delete(`${workspaceId}/${id}`);
        }
      }),
    );
  }
}
