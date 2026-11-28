/**
 * What an open board or wiki of somebody else's comes to for who works in the
 * workspace (`state/join-on-write.ts`, `state/order.ts`): it waits apart, at
 * the sidebar's foot, until the reader's first write joins it and files it
 * among their own; and one made after the sidebar was read is listed there
 * once opened.
 */
import { describe, expect, it, vi } from 'vitest';

import { createBoardView, type Container } from '$lib/logic/wire.ts';
import type { Category } from '$lib/state/category-types.ts';
import {
  noteMembership,
  openToJoinOf,
  readerOf,
  WikiStore,
  type SidebarReader,
} from '$lib/state/index.ts';
import { sidebarOf } from '$lib/state/order.ts';

import { setup, text } from './support.ts';

const category = (id: string, containerIds: string[], hiddenContainerIds: string[] = []) =>
  ({
    id,
    name: id,
    userId: 'me',
    workspaceId: 't',
    type: 'system',
    collapsed: false,
    sortOrder: 0,
    containerIds,
    hiddenContainerIds,
    createdAt: 0,
    updatedAt: 0,
  }) as Category;

const container = (id: string, title: string, mine: Partial<Container> = {}) =>
  ({
    id,
    title,
    createdAt: 0,
    createdBy: 'them',
    visibility: 'open',
    isTemplate: false,
    ...mine,
  }) as Container;

/** The reader of the sidebar, a member of the workspace who knows of no membership yet. */
const me: SidebarReader = { id: 'me', guest: false, memberOf: new Set() };

describe('the open ones the reader may join', () => {
  it('are the open ones of others that no category files, by title, kept from the loose list', () => {
    const containers = [
      container('zeta', 'Zeta'),
      container('alpha', 'alpha'),
      container('filed', 'Filed'),
      container('hidden', 'Hidden'),
      container('mine', 'Mine', { createdBy: 'me' }),
      container('closed', 'Closed', { visibility: 'private' }),
      container('template', 'Template', { isTemplate: true }),
    ];
    const categories = [category('general', ['filed'], ['hidden'])];
    expect(openToJoinOf(categories, containers, 'en', me).map((one) => one.id)).toEqual([
      'alpha',
      'zeta',
    ]);
    const loose = sidebarOf(categories, containers, 'en', me).flatMap((entry) =>
      entry.containers.map((one) => one.id),
    );
    expect(loose).toEqual(['filed', 'closed', 'mine', 'template']);
  });

  it('are none to a guest, who cannot join them: what they were let into is at the end of the list', () => {
    const containers = [container('zeta', 'Zeta'), container('filed', 'Filed')];
    const categories = [category('general', ['filed'])];
    const guest = { ...me, guest: true };
    expect(openToJoinOf(categories, containers, 'en', guest)).toEqual([]);
    const listed = sidebarOf(categories, containers, 'en', guest).flatMap((entry) =>
      entry.containers.map((one) => one.id),
    );
    expect(listed).toEqual(['filed', 'zeta']);
  });

  it('leave out one the reader is known to be a member of, by a group as well, though no category files it', () => {
    const containers = [container('zeta', 'Zeta'), container('grouped', 'Group board')];
    const categories = [category('general', [])];
    const member = { ...me, memberOf: new Set(['grouped']) };
    expect(openToJoinOf(categories, containers, 'en', member).map((one) => one.id)).toEqual([
      'zeta',
    ]);
    const listed = sidebarOf(categories, containers, 'en', member).flatMap((entry) =>
      entry.containers.map((one) => one.id),
    );
    expect(listed).toEqual(['grouped']);
  });

  it('is told a membership noted after it was worked out, with nothing else changed', async () => {
    const made = setup();
    const reader = made.workspace;
    await reader.load();
    const readerId = (await made.backend.me()).id;
    const { board } = await made.other.createBoard(
      made.world.workspace.id,
      { title: 'Of the group', visibility: 'open' },
      { title: 'Board', fields: createBoardView().fields },
    );
    reader.receiveContainer(board);
    const listed = () => reader.sidebar.flatMap((entry) => entry.containers.map((one) => one.id));
    // Worked out before anything was known of the reader's memberships.
    expect(listed()).not.toContain(board.id);
    noteMembership(reader, board.id, [
      {
        containerId: board.id,
        userId: readerId,
        role: 'editor',
        synthetic: true,
        inheritedRole: 'editor',
      },
    ]);
    expect(listed()).toContain(board.id);
    noteMembership(reader, board.id, []);
    expect(listed()).not.toContain(board.id);
  });

  it('leave out an open board the reader has a role on by a group, once it is opened', async () => {
    const made = setup();
    const reader = made.workspace;
    await reader.load();
    const readerId = (await made.backend.me()).id;
    const { board } = await made.other.createBoard(
      made.world.workspace.id,
      { title: 'Of the group', visibility: 'open' },
      { title: 'Board', fields: createBoardView().fields },
    );
    reader.receiveContainer(board);
    const waiting = () =>
      openToJoinOf(reader.categories, reader.containers, reader.locale, readerOf(reader)).map(
        (one) => one.title,
      );
    expect(waiting()).toEqual(['Of the group']);

    // A group of the reader's is given a role on it: the reader is a member, though not filed.
    made.world.members.set(board.id, [
      ...(made.world.members.get(board.id) ?? []),
      {
        containerId: board.id,
        userId: readerId,
        role: 'editor',
        synthetic: true,
        inheritedRole: 'editor',
      },
    ]);
    const store = await reader.openBoard(board.id);
    expect([store.role, store.joinable]).toEqual(['editor', false]);
    expect(waiting()).toEqual([]);
    expect(reader.sidebar.flatMap((entry) => entry.containers.map((one) => one.id))).toContain(
      board.id,
    );
  });

  it('includes a wiki made after the sidebar was read, once opened, and leaves once written in', async () => {
    const made = setup();
    const reader = made.workspace;
    await reader.load();
    const wiki = await made.other.wiki.createWiki(made.world.workspace.id, {
      title: 'Handbook',
      visibility: 'open',
    });
    const waiting = () =>
      openToJoinOf(reader.categories, reader.containers, reader.locale, readerOf(reader)).map(
        (one) => one.title,
      );
    expect(waiting()).toEqual([]);

    const store = new WikiStore(reader, wiki.id);
    await store.load();
    expect(waiting()).toEqual(['Handbook']);

    await store.createPage({ title: 'Mine' });
    await expect.poll(waiting).toEqual([]);
    const filed = reader.sidebar.flatMap((entry) => entry.containers.map((one) => one.id));
    expect(filed).toContain(wiki.id);
    store.close();
  });
});

/** The methods a write off the body sends, recorded in the order they are sent. */
const SENT = [
  'joinContainer',
  'upload',
  'createAttachment',
  'deleteAttachment',
  'deleteComment',
] as const;

/**
 * The seed's board, opened to the reader as an open board they are no member
 * of, with its first card open: there a comment of theirs (written before they
 * left) and an attachment wait to be written to.
 */
async function openUnjoined() {
  const made = setup();
  await made.workspace.load();
  const first = made.workspace.sidebar.flatMap((entry) => entry.containers)[0];
  const stored = first && made.world.containers.get(first.id);
  if (!first || !stored) throw new Error('the seed has no board');
  const readerId = (await made.backend.me()).id;
  const cardId = (await made.backend.cards(first.id)).find((card) => !card.fields.isTemplate)?.id;
  if (!cardId) throw new Error('the seed board has no card');
  const comment = await made.backend.createComment(first.id, cardId, text('Mine, from before'));
  const uploaded = await made.backend.upload(first.id, new File(['old'], 'old.txt'));
  const attachment = await made.backend.createAttachment(first.id, cardId, uploaded);
  made.world.containers.set(first.id, { ...stored, visibility: 'open' });
  made.world.members.set(
    first.id,
    (made.world.members.get(first.id) ?? []).filter((member) => member.userId !== readerId),
  );
  const board = await made.workspace.openBoard(first.id);
  const card = await board.showCard(cardId);
  const sent: string[] = [];
  for (const method of SENT) {
    const real = made.backend[method].bind(made.backend) as (...args: unknown[]) => unknown;
    vi.spyOn(made.backend, method).mockImplementation(((...args: unknown[]) => {
      sent.push(method);
      return real(...args);
    }) as never);
  }
  return { made, board, card, sent, readerId, commentId: comment.id, attachmentId: attachment.id };
}

describe('a write off the body of an open board the reader is no member of', () => {
  it.each([
    {
      write: 'a file attached',
      run: ({ card }: Awaited<ReturnType<typeof openUnjoined>>) =>
        card.attach(new File(['hello'], 'hello.txt', { type: 'text/plain' })),
      sends: ['upload', 'createAttachment'],
    },
    {
      write: 'an attachment deleted',
      run: ({ card, attachmentId }: Awaited<ReturnType<typeof openUnjoined>>) =>
        card.deleteAttachment(attachmentId),
      sends: ['deleteAttachment'],
    },
    {
      write: 'their own comment deleted',
      run: ({ card, commentId }: Awaited<ReturnType<typeof openUnjoined>>) =>
        card.deleteComment(commentId),
      sends: ['deleteComment'],
    },
  ])('joins it first: $write', async ({ run, sends }) => {
    const opened = await openUnjoined();
    const { made, board, sent, readerId } = opened;
    expect([board.joinable, board.joinsOnWrite, board.canEdit]).toEqual([true, true, true]);

    await run(opened);

    expect(sent).toEqual(['joinContainer', ...sends]);
    expect(made.workspace.problems).toHaveLength(0);
    expect(board.joinable).toBe(false);
    // What the reader may do was read again for the role the join gave, and is taken as read.
    expect([board.role, board.permissionsRole]).toEqual(['editor', 'editor']);
    expect(board.effectivePermissions).toBe(board.permissions);
    const joined = (made.world.members.get(board.boardId) ?? []).filter(
      (member) => member.userId === readerId,
    );
    expect(joined.map((member) => member.role)).toEqual(['editor']);
  });
});
