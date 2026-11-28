/**
 * The inbox sorted as a mail inbox is (`NotificationInbox` on the in-memory
 * backend): each tab lists its own; one put away leaves the inbox for the
 * archive, read, and one marked unread there comes back, counted; every one
 * read is put away at once; and more of one put away brings it back.
 */
import { describe, expect, it } from 'vitest';

import { NotificationInbox } from '$lib/state/notification-inbox.svelte.ts';

import { settle, setup, text } from './support.ts';

/** The reader's inbox, and two cards of theirs the other person comments on. */
async function twoToldOf() {
  const made = setup();
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  if (!board) throw new Error('no board');
  const first = await made.backend.createCard(board.id, { title: 'First' });
  const second = await made.backend.createCard(board.id, { title: 'Second' });
  const refused: unknown[] = [];
  const inbox = new NotificationInbox(made.backend.notifications, made.backend, (error) =>
    refused.push(error),
  );
  const comment = (cardId: string) => made.other.createComment(board.id, cardId, text('Seen it'));
  await comment(first.id);
  await comment(second.id);
  inbox.start();
  await inbox.open();
  await settle();
  return { ...made, inbox, refused, comment, first, second };
}

const titles = (inbox: NotificationInbox) => inbox.items.map((one) => one.subject?.title);

describe('the inbox sorted', () => {
  it('puts one away into the archive, read, and brings it back unread', async () => {
    const made = await twoToldOf();
    expect([made.inbox.unread, titles(made.inbox)]).toEqual([2, ['Second', 'First']]);
    const [newest] = made.inbox.items;
    if (!newest) throw new Error('listed');

    const archiving = made.inbox.archive(newest);
    expect([made.inbox.unread, titles(made.inbox)]).toEqual([1, ['First']]);
    await archiving;
    await settle();
    expect([made.inbox.unread, titles(made.inbox)]).toEqual([1, ['First']]);

    await made.inbox.show('archived');
    expect(titles(made.inbox)).toEqual(['Second']);
    const [archived] = made.inbox.items;
    expect(archived?.readAt).toBeGreaterThan(0);
    expect(archived?.archivedAt).toBeGreaterThan(0);
    if (!archived) throw new Error('archived');

    await made.inbox.markUnread(archived);
    await settle();
    expect([made.inbox.unread, made.inbox.items]).toEqual([2, []]);
    await made.inbox.show('unread');
    expect(titles(made.inbox)).toEqual(['Second', 'First']);
    expect(made.refused).toEqual([]);
  });

  it('lists in each tab its own, and a kind none of them is', async () => {
    const made = await twoToldOf();
    const [, older] = made.inbox.items;
    if (!older) throw new Error('listed');
    await made.inbox.read(older);
    await made.inbox.show('unread');
    expect(titles(made.inbox)).toEqual(['Second']);
    await made.inbox.show('mention');
    expect([made.inbox.listed, made.inbox.items]).toEqual([true, []]);
    await made.inbox.show('all');
    expect(titles(made.inbox)).toEqual(['Second', 'First']);
  });

  it('puts every one read away at once, and takes one back read', async () => {
    const made = await twoToldOf();
    const [, older] = made.inbox.items;
    if (!older) throw new Error('listed');
    await made.inbox.toggleRead(older);
    await made.inbox.archiveRead();
    await settle();
    expect([made.inbox.unread, titles(made.inbox)]).toEqual([1, ['Second']]);

    await made.inbox.show('archived');
    const [archived] = made.inbox.items;
    if (!archived) throw new Error('archived');
    expect(archived.subject?.title).toBe('First');
    await made.inbox.unarchive(archived);
    await settle();
    expect(made.inbox.items).toEqual([]);
    await made.inbox.show('all');
    expect(titles(made.inbox)).toEqual(['Second', 'First']);
    expect(made.inbox.items[1]?.readAt).toBeGreaterThan(0);
    expect(made.inbox.unread).toBe(1);
  });

  it('brings one put away back with more of it, unread', async () => {
    const made = await twoToldOf();
    const [newest] = made.inbox.items;
    if (!newest) throw new Error('listed');
    await made.inbox.archive(newest);
    await made.comment(made.second.id);
    await settle();
    await settle();
    expect([made.inbox.unread, titles(made.inbox)]).toEqual([2, ['Second', 'First']]);
    expect(made.inbox.items[0]?.archivedAt).toBe(0);
  });
});
