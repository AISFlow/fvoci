/**
 * The inbox of the top bar: its count follows what the reader is told as it
 * happens, the list is read while it is open, who did each is named, and
 * reading one or all of them shows at once; an answer that may not know of a
 * read does not count, and a read that is refused is put back.
 */
import { describe, expect, it } from 'vitest';

import { NotificationInbox } from '$lib/state/notification-inbox.svelte.ts';
import type { Notification, Notifications } from '$lib/state/notification-types.ts';

import { settle, setup, text } from './support.ts';

async function anInbox() {
  const made = setup();
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  if (!board) throw new Error('no board');
  const card = await made.backend.createCard(board.id, { title: 'Plan' });
  const refused: unknown[] = [];
  const inbox = new NotificationInbox(made.backend.notifications, made.backend, (error) =>
    refused.push(error),
  );
  const comment = (words: string) => made.other.createComment(board.id, card.id, text(words));
  return { ...made, inbox, refused, comment };
}

describe('the inbox', () => {
  it('counts what the reader is told as it happens, and lists it while open', async () => {
    const made = await anInbox();
    const stop = made.inbox.start();
    await settle();
    expect(made.inbox.unread).toBe(0);

    await made.comment('first');
    await settle();
    await settle();
    expect(made.inbox.unread).toBe(1);
    expect(made.inbox.listed).toBe(false);

    await made.inbox.open();
    await settle();
    expect(made.inbox.items).toHaveLength(1);
    expect(made.inbox.people.get(made.other.userId)?.id).toBe(made.other.userId);

    // Open, the list follows: more of the same card joins its line.
    await made.comment('second');
    await settle();
    await settle();
    expect(made.inbox.items.map((one) => one.subject?.times)).toEqual([2]);
    stop();
    expect(made.refused).toEqual([]);
  });

  it('shows one read, and all read, at once', async () => {
    const made = await anInbox();
    made.inbox.start();
    await made.comment('to read');
    await made.inbox.open();
    const [first] = made.inbox.items;
    if (!first) throw new Error('listed');
    const reading = made.inbox.read(first);
    expect(made.inbox.items[0]?.readAt).toBeGreaterThan(0);
    await reading;
    await settle();
    expect(made.inbox.unread).toBe(0);

    await made.comment('again');
    await settle();
    await settle();
    expect(made.inbox.unread).toBe(1);
    await made.inbox.readAll();
    await settle();
    expect(made.inbox.unread).toBe(0);
    expect(made.inbox.items.every((one) => one.readAt > 0)).toBe(true);
  });
});

/** A notification of the stand-in below, unread. */
const ONE: Notification = {
  id: 'n1',
  createdAt: 1,
  updatedAt: 2,
  readAt: 0,
  archivedAt: 0,
  subject: null,
};

/** Notifications whose answers the test gives, one by one, in the order asked. */
function standIn() {
  const counts: ((count: number) => void)[] = [];
  const lists: ((items: Notification[]) => void)[] = [];
  const writes: (() => void)[] = [];
  let refuse = false;
  let hold = false;
  const write = () =>
    hold ? new Promise<void>((resolve) => writes.push(resolve)) : Promise.resolve();
  const notifications: Notifications = {
    list: () => new Promise((resolve) => lists.push(resolve)),
    unread: () => new Promise((resolve) => counts.push(resolve)),
    read: () => (refuse ? Promise.reject(new Error('refused')) : write()),
    readAll: write,
    markUnread: write,
    archive: () => (refuse ? Promise.reject(new Error('refused')) : write()),
    unarchive: write,
    archiveRead: write,
  };
  const backend = { watchSelf: () => () => undefined, usersById: () => Promise.resolve([]) };
  const refused: unknown[] = [];
  const inbox = new NotificationInbox(notifications, backend, (error) => refused.push(error));
  return {
    inbox,
    refused,
    count: (count: number) => counts.shift()?.(count),
    list: (items: Notification[]) => lists.shift()?.(items),
    refuseReads: () => {
      refuse = true;
    },
    /** Reads are written only when told to, one by one. */
    holdWrites: () => {
      hold = true;
    },
    written: () => writes.shift()?.(),
    /** How many counts are asked and not answered yet. */
    countsAsked: () => counts.length,
  };
}

describe('the inbox, against what it is answered', () => {
  it('lets no count asked before a read undo it', async () => {
    const made = standIn();
    made.inbox.start();
    made.count(1);
    const opened = made.inbox.open();
    made.list([ONE]);
    await opened;
    await settle();
    expect(made.inbox.unread).toBe(1);

    // Asked again, and the line read before the answer comes: it says one still.
    const refreshed = made.inbox.refresh();
    const reading = made.inbox.read(ONE);
    expect(made.inbox.unread).toBe(0);
    made.count(1);
    made.list([ONE]);
    await Promise.all([refreshed, reading]);
    expect(made.inbox.unread).toBe(0);
    expect(made.inbox.items[0]?.readAt).toBeGreaterThan(0);
  });

  it('puts back a read that is refused, as it is read again', async () => {
    const made = standIn();
    made.inbox.start();
    made.count(1);
    const opened = made.inbox.open();
    made.list([ONE]);
    await opened;
    made.refuseReads();
    const reading = made.inbox.read(ONE);
    expect(made.inbox.unread).toBe(0);
    await settle();
    made.count(1);
    made.list([ONE]);
    await reading;
    expect(made.refused).toHaveLength(1);
    expect([made.inbox.unread, made.inbox.items[0]?.readAt]).toEqual([1, 0]);
  });

  it('lets nothing answered while a read is written undo it, and reads again', async () => {
    const made = standIn();
    made.inbox.start();
    made.count(1);
    const opened = made.inbox.open();
    made.list([ONE]);
    await opened;
    await settle();

    // Asked after the click, answered before the read is stored: it says one still.
    made.holdWrites();
    const reading = made.inbox.read(ONE);
    const refreshed = made.inbox.refresh();
    made.count(1);
    made.list([ONE]);
    await refreshed;
    expect(made.inbox.unread).toBe(0);
    expect(made.inbox.items[0]?.readAt).toBeGreaterThan(0);

    made.written();
    await settle();
    expect(made.countsAsked()).toBe(1);
    made.count(0);
    made.list([{ ...ONE, readAt: 5 }]);
    await reading;
    expect([made.inbox.unread, made.inbox.items[0]?.readAt]).toEqual([0, 5]);
  });

  it('lets no list asked before all are read undo it', async () => {
    const made = standIn();
    const opened = made.inbox.open();
    made.list([ONE]);
    await opened;

    const refreshed = made.inbox.refresh();
    made.holdWrites();
    const reading = made.inbox.readAll();
    made.count(1);
    made.list([ONE]);
    await refreshed;
    expect(made.inbox.items[0]?.readAt).toBeGreaterThan(0);

    made.written();
    await settle();
    made.count(0);
    made.list([{ ...ONE, readAt: 5 }]);
    await reading;
    expect([made.inbox.unread, made.inbox.items[0]?.readAt]).toEqual([0, 5]);
  });

  it('puts back one put away where that is refused', async () => {
    const made = standIn();
    made.inbox.start();
    made.count(1);
    const opened = made.inbox.open();
    made.list([ONE]);
    await opened;
    await settle();
    made.refuseReads();
    const archiving = made.inbox.archive(ONE);
    expect([made.inbox.unread, made.inbox.items]).toEqual([0, []]);
    await settle();
    made.count(1);
    made.list([ONE]);
    await archiving;
    expect([made.inbox.unread, made.inbox.items]).toEqual([1, [ONE]]);
    expect(made.refused).toHaveLength(1);
  });

  it('shows a list opened again only once it is read again', async () => {
    const made = standIn();
    const first = made.inbox.open();
    made.list([ONE]);
    await first;
    expect(made.inbox.listed).toBe(true);
    made.inbox.close();
    expect(made.inbox.listed).toBe(false);
    const again = made.inbox.open();
    await settle();
    expect(made.inbox.listed).toBe(false);
    made.list([]);
    await again;
    expect([made.inbox.listed, made.inbox.items]).toEqual([true, []]);
  });
});
