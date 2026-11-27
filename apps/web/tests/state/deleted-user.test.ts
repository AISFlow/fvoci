/**
 * Somebody whose account was deleted, as the workspace names them, and the
 * session of an account that waits to be deleted, which opens nothing.
 */
import { describe, expect, it, vi } from 'vitest';

import { BackendError } from '$lib/state/index.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';
import { eraseUser } from '$lib/state/memory/users.ts';
import { assignedIds, authorIds, mentionedIn } from '$lib/state/user-names.ts';
import { Workspace } from '$lib/state/workspace.svelte.ts';

import { SMALL } from './support.ts';

function made() {
  const { world, readerId } = seedWorld([SMALL]);
  const otherId = [...world.users.keys()].find((id) => id !== readerId);
  if (!otherId) throw new Error('the seed has one person only');
  return { world, backend: new MemoryBackend(world, readerId), otherId };
}

describe('somebody whose account was deleted', () => {
  it('is none of the people, and is named as a deleted user where they wrote', async () => {
    const { world, backend, otherId } = made();
    const card = [...world.blocks.values()].find((block) => block.type === 'card');
    if (!card) throw new Error('the seed has no card');
    world.blocks.set(card.id, { ...card, createdBy: otherId, modifiedBy: otherId });
    const before = world.users.get(otherId)?.username ?? '';
    eraseUser(world, otherId);

    const workspace = new Workspace({
      backend,
      locale: () => 'en',
      goneUserName: () => 'Deleted user',
    });
    await workspace.load();
    expect(workspace.users.map((user) => user.id)).not.toContain(otherId);
    expect(workspace.userName(otherId)).toBeUndefined();
    expect(await workspace.searchUsers(before)).toEqual([]);

    await workspace.learnUsers([otherId]);
    // Nor when the account came or went: the name tells it, not a time.
    expect(workspace.user(otherId)).toMatchObject({
      username: '',
      displayName: '',
      email: '',
      createdAt: 0,
      updatedAt: 0,
    });
    expect(workspace.user(otherId)?.deletedAt).toBeUndefined();
    expect(workspace.userName(otherId)).toBe('Deleted user');
    // On no board any more.
    for (const members of world.members.values()) {
      expect(members.map((member) => member.userId)).not.toContain(otherId);
    }
  });

  it('is as unknown as anybody without a name where the app has no word for them', async () => {
    const { world, backend, otherId } = made();
    eraseUser(world, otherId);
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load();
    await workspace.learnUsers([otherId]);
    expect(workspace.userName(otherId)).toBeUndefined();
  });
});

describe('whom cards and bodies name besides their writers', () => {
  const mention = (userId: string) => ({ type: 'mention', attrs: { userId } });
  const body = {
    type: 'doc',
    content: [
      { type: 'paragraph', content: [mention('u1'), { type: 'text', text: ' and ' }] },
      {
        type: 'bulletList',
        content: [{ type: 'listItem', content: [mention('u2'), mention('')] }],
      },
    ],
  };

  it('is read from the mentions of a body, however deep, and the properties of people', () => {
    expect(mentionedIn(body)).toEqual(['u1', 'u2']);
    expect(mentionedIn(null)).toEqual([]);
    expect(
      authorIds([
        { createdBy: 'a', modifiedBy: 'b', body },
        { createdBy: 'c', modifiedBy: 'c' },
      ]),
    ).toEqual(['a', 'b', 'u1', 'u2', 'c', 'c']);
    const properties = [
      { id: 'one', type: 'person' },
      { id: 'many', type: 'multiPerson' },
      { id: 'text', type: 'text' },
    ];
    const card = {
      fields: { properties: { one: 'u3', many: ['u4', ''], text: 'u5', gone: 'u6' } },
    };
    expect(assignedIds([card], properties)).toEqual(['u3', 'u4']);
  });

  it('is asked for when a board is read: a card set to somebody whose account was deleted names a deleted user', async () => {
    const { world, backend, otherId } = made();
    const boardId = [...world.containers.keys()][0] ?? '';
    const board = world.containers.get(boardId);
    const card = [...world.blocks.values()].find(
      (block) => block.type === 'card' && block.containerId === boardId,
    );
    if (!board || !card) throw new Error('the seed has no board with a card');
    // A property of people on the board, set to somebody who wrote nothing there.
    for (const block of world.blocks.values()) {
      if (block.createdBy === otherId) {
        world.blocks.set(block.id, {
          ...block,
          createdBy: board.createdBy,
          modifiedBy: board.createdBy,
        });
      }
    }
    world.containers.set(boardId, {
      ...board,
      cardProperties: [
        ...board.cardProperties,
        { id: 'who', name: 'Who', type: 'person', options: [] },
      ],
    });
    world.blocks.set(card.id, {
      ...card,
      createdBy: board.createdBy,
      modifiedBy: board.createdBy,
      fields: {
        ...card.fields,
        properties: { ...(card.fields.properties as Record<string, string>), who: otherId },
      },
    });
    eraseUser(world, otherId);

    const workspace = new Workspace({
      backend,
      locale: () => 'en',
      goneUserName: () => 'Deleted user',
    });
    await workspace.load();
    await workspace.openBoard(boardId);
    await vi.waitFor(() => {
      expect(workspace.userName(otherId)).toBe('Deleted user');
    });
  });
});

describe('the session of an account that waits to be deleted', () => {
  const refusal = (reason: string | undefined) =>
    new BackendError(401, { detail: 'no', ...(reason ? { reason } : {}) });

  it('opens nothing: the workspace says so, and listens no more', async () => {
    const { backend } = made();
    backend.failNext('me', refusal('account_deletion_pending'));
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load();
    expect(workspace.status).toBe('leaving');
    expect(workspace.problems).toEqual([]);
  });

  it('is read again once the account waits no more: the workspace opens, and listens', async () => {
    // Taken back in another tab: the waiting screen has the workspace read again.
    const { backend } = made();
    const watch = vi.spyOn(backend, 'watchSelf');
    backend.failNext('me', refusal('account_deletion_pending'));
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load();
    expect(workspace.status).toBe('leaving');
    expect(watch).not.toHaveBeenCalled();

    await workspace.load();
    expect(workspace.status).toBe('ready');
    expect(workspace.me).toBeDefined();
    expect(watch).toHaveBeenCalledTimes(1);

    // Erased meanwhile: the session is over, and the sign-in is shown.
    const erased = new Workspace({ backend, locale: () => 'en' });
    backend.failNext('me', refusal('account_deletion_pending'));
    await erased.load();
    backend.failNext('me', refusal(undefined));
    await erased.load();
    expect(erased.status).toBe('signedOut');
  });

  it('is told apart from one that is over, also while working', async () => {
    const { backend } = made();
    const workspace = new Workspace({ backend, locale: () => 'en' });
    await workspace.load();
    expect(workspace.status).toBe('ready');
    workspace.report('save', refusal('account_deletion_pending'));
    expect(workspace.status).toBe('leaving');

    const other = new Workspace({ backend, locale: () => 'en' });
    await other.load();
    other.report('save', refusal(undefined));
    expect(other.status).toBe('signedOut');
  });
});
