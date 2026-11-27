/**
 * Each account's own space in the backend in memory, as the contract has it
 * ("The user's own space"): listed last with its kind and no name, a
 * workspace like another to what is under it, with its owner alone in it.
 * Of anybody else's it is not there (404), open boards and wikis included,
 * and nothing goes between it and the shared workspace but by archive.
 */
import { describe, expect, it } from 'vitest';

import { firstKanban } from '$lib/state/workspace-entry.ts';
import {
  BackendError,
  confirmPassword,
  passwordChanged,
  setFirstPassword,
  type Change,
} from '$lib/state/index.ts';
import type { MemoryBackend } from '$lib/state/memory/backend.ts';
import { closeOwnSpace } from '$lib/state/memory/spaces.ts';

import { settle, setup } from './support.ts';

async function statusOf(promise: Promise<unknown>): Promise<number | 'ok'> {
  try {
    await promise;
    return 'ok';
  } catch (error) {
    if (error instanceof BackendError) return error.status;
    throw error;
  }
}

async function ownSpace(backend: MemoryBackend): Promise<string> {
  const own = (await backend.workspaces()).find((workspace) => workspace.kind === 'personal');
  if (!own) throw new Error('no own space');
  return own.id;
}

async function boardIn(backend: MemoryBackend, workspaceId: string, title: string, open = false) {
  const made = await backend.createBoard(
    workspaceId,
    { title, ...(open ? { visibility: 'open' as const } : {}) },
    firstKanban([], 'Board'),
  );
  return made.board;
}

describe('the own space in memory', () => {
  it('is listed last, of its kind and without a name, one for each account', async () => {
    const { backend, other, world } = setup();
    const mine = await backend.workspaces();
    expect(mine.map((workspace) => workspace.kind)).toEqual([world.workspace.kind, 'personal']);
    expect(world.workspace.kind).not.toBe('personal');
    expect(mine[0]?.id).toBe(world.workspace.id);
    expect(mine[1]?.title).toBe('');
    expect(await backend.workspaces()).toEqual(mine);
    const theirs = await other.workspaces();
    expect(theirs[1]?.id).not.toBe(mine[1]?.id);
  });

  it('lists what is made in it there alone, filed under its own default category', async () => {
    const { backend, world } = setup();
    const own = await ownSpace(backend);
    const told: Change[] = [];
    backend.watchSelf((change) => told.push(change));
    const board = await boardIn(backend, own, 'Mine');
    const wiki = await backend.wiki.createWiki(own, { title: 'Notes', visibility: 'private' });
    expect((await backend.boards(own)).map((one) => one.id)).toEqual([board.id]);
    expect((await backend.wiki.wikis(own)).map((one) => one.id)).toEqual([wiki.id]);
    expect((await backend.boards(world.workspace.id)).map((one) => one.id)).not.toContain(board.id);
    const categories = await backend.categories(own);
    expect(categories).toHaveLength(1);
    expect(categories[0]?.type).toBe('system');
    expect(categories[0]?.containerIds).toEqual([board.id, wiki.id]);
    expect(
      (await backend.categories(world.workspace.id)).flatMap((one) => one.containerIds),
    ).not.toContain(board.id);
    // What the sidebar is told is the categories of one workspace.
    for (const change of told) {
      if (change.kind !== 'categories') continue;
      expect(change.categories.every((one) => one.workspaceId === own)).toBe(true);
    }
  });

  it('has its owner alone in it', async () => {
    const { backend } = setup();
    const own = await ownSpace(backend);
    const me = await backend.me();
    expect((await backend.users(own)).map((user) => user.id)).toEqual([me.id]);
    expect((await backend.searchUsers(own, '')).map((user) => user.id)).toEqual([me.id]);
    const board = await boardIn(backend, own, 'Mine');
    const someone = (await backend.users((await backend.workspaces())[0]?.id ?? '')).find(
      (user) => user.id !== me.id,
    );
    if (!someone) throw new Error('the seed has one person only');
    expect(await statusOf(backend.addMember(board.id, someone.id, 'editor'))).toBe(400);
    expect(await backend.usersById(own, [someone.id])).toEqual([]);
  });

  it('is not there for anybody else, its open boards and wikis included', async () => {
    const { backend, other } = setup();
    const own = await ownSpace(backend);
    const board = await boardIn(backend, own, 'Open', true);
    const wiki = await backend.wiki.createWiki(own, { title: 'Open notes', visibility: 'open' });
    for (const asked of [
      other.boards(own),
      other.users(own),
      other.usersById(own, []),
      other.categories(own),
      other.templates(own),
      other.searchBoards(own, 'Open'),
      other.deletedContainers(own),
      other.createBoard(own, { title: 'In' }, firstKanban([], 'Board')),
      other.onboard(own),
      other.wiki.wikis(own),
      other.wiki.createWiki(own, { title: 'In', visibility: 'private' }),
      other.search.find(own, { query: 'Open' }),
      other.references.resolve(own, [board.id]),
      other.exportWorkspace(own),
      other.joinContainer(board.id),
      other.views(board.id),
      other.cards(board.id),
      other.wiki.pages(wiki.id),
    ]) {
      expect(await statusOf(asked)).toBe(404);
    }
  });

  it('takes nothing from the shared workspace but a built-in template, and gives nothing to it', async () => {
    const { backend, world } = setup();
    const own = await ownSpace(backend);
    const shared = (await backend.boards(world.workspace.id))[0];
    if (!shared) throw new Error('no shared board');
    expect(await statusOf(backend.duplicateContainer(shared.id, false, own))).toBe(404);
    const mine = await boardIn(backend, own, 'Mine');
    expect(await statusOf(backend.duplicateContainer(mine.id, false, world.workspace.id))).toBe(
      404,
    );
    const copied = await backend.duplicateContainer(mine.id, false, own);
    expect(copied.workspaceId).toBe(own);
    // A built-in template of the server, the welcome board among them, goes into any workspace.
    const builtIn = world.containers.get(shared.id);
    if (!builtIn) throw new Error('no shared board');
    Object.assign(builtIn, { isTemplate: true, isBuiltin: true, properties: { onboarding: '1' } });
    expect((await backend.templates(own)).map((one) => one.id)).toEqual([shared.id]);
    expect((await backend.duplicateContainer(shared.id, false, own)).workspaceId).toBe(own);
    const welcome = await backend.onboard(own);
    expect((await backend.container(welcome)).workspaceId).toBe(own);
  });

  it('brings a deleted board back into its own category, and tells that workspace’s', async () => {
    const { backend } = setup();
    const own = await ownSpace(backend);
    const board = await boardIn(backend, own, 'Back');
    await backend.deleteContainer(board.id);
    expect((await backend.deletedContainers(own)).map((item) => item.id)).toEqual([board.id]);
    const told: Change[] = [];
    backend.watchSelf((change) => told.push(change));
    await backend.restoreContainer(board.id);
    expect((await backend.categories(own))[0]?.containerIds).toEqual([board.id]);
    const categories = told.flatMap((change) =>
      change.kind === 'categories' ? change.categories : [],
    );
    expect(categories.length).toBeGreaterThan(0);
    expect(categories.every((one) => one.workspaceId === own)).toBe(true);
  });

  it('links to nothing across spaces: a card of the shared workspace is not found from it', async () => {
    const { backend, world } = setup();
    const own = await ownSpace(backend);
    const shared = await boardIn(backend, world.workspace.id, 'Shared');
    const card = await backend.createCard(shared.id, { title: 'Crossing card' });
    const found = await backend.references.search(world.workspace.id, 'Crossing');
    expect(found.map((one) => one.id)).toEqual([card.id]);
    expect(await backend.references.search(own, 'Crossing')).toEqual([]);
    expect(await backend.references.resolve(own, [card.id])).toEqual([]);
    expect(await statusOf(backend.references.to(own, card.id))).toBe(404);
  });
});

describe('the own space, once it takes the password alone', () => {
  it('is not entered by the list that says so: nothing of it is read, and the session goes on', async () => {
    const { backend, world, workspace } = setup();
    const own = await ownSpace(backend);
    const kept = world.ownSpaces.get(backend.userId);
    if (!kept) throw new Error('no own space');
    kept.passwordRequired = true;
    await workspace.load(own);
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('password');
    expect(workspace.closed?.id).toBe(own);
    expect(workspace.info).toBeUndefined();
    expect(workspace.containers).toEqual([]);
    // No people of it are read: the reader is named by their own reading.
    const reader = world.users.get(backend.userId);
    expect(workspace.userName(backend.userId)).toContain(`@${reader?.username ?? ''}`);
  });

  it('closes when the backend refuses it to the session, which is not over', async () => {
    const { backend, world, workspace } = setup();
    const own = await ownSpace(backend);
    await boardIn(backend, own, 'Mine');
    await workspace.load(own);
    expect(workspace.entered).toBe('ready');
    const kept = world.ownSpaces.get(backend.userId);
    if (!kept) throw new Error('no own space');
    kept.passwordRequired = true;
    const refused = await backend.boards(own).catch((error: unknown) => error);
    expect(refused).toMatchObject({ status: 401, reason: 'password_sign_in_required' });
    workspace.report('load', refused);
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('password');
    expect(workspace.containers).toEqual([]);
    expect(workspace.workspaces.find((one) => one.id === own)?.passwordRequired).toBe(true);
  });

  it('closes when it is read after the list said it was open, the session still good', async () => {
    const { backend, world, workspace } = setup();
    const own = await ownSpace(backend);
    await workspace.load(world.workspace.id);
    const kept = world.ownSpaces.get(backend.userId);
    if (!kept) throw new Error('no own space');
    kept.passwordRequired = true;
    expect(workspace.workspaces.find((one) => one.id === own)?.passwordRequired).toBeFalsy();
    await workspace.enter(own);
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('password');
    expect(workspace.closed?.id).toBe(own);
    expect(workspace.workspaces.find((one) => one.id === own)?.passwordRequired).toBe(true);
  });

  it('closes when what was missed is read again and refused, as after the socket lets it go', async () => {
    const { backend, world, workspace } = setup();
    const own = await ownSpace(backend);
    await boardIn(backend, own, 'Mine');
    await workspace.load(own);
    expect(workspace.containers.length).toBeGreaterThan(0);
    const kept = world.ownSpaces.get(backend.userId);
    if (!kept) throw new Error('no own space');
    kept.passwordRequired = true;
    // workspaceUnsubscribed is answered by asking again and reading again (`resync`).
    world.tellUsers([backend.userId], { kind: 'resync' });
    await settle();
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('password');
    expect(workspace.containers).toEqual([]);
  });

  it('stays open on the page that gave the account its first password: the session shows it', async () => {
    const { backend, world, workspace } = setup();
    const reader = world.users.get(backend.userId);
    if (!reader) throw new Error('no reader');
    world.users.set(reader.id, { ...reader, hasPassword: false });
    const own = await ownSpace(backend);
    const board = await boardIn(backend, own, 'Mine');
    await workspace.load(own);
    const between: string[] = [];
    const set = async (password: string): Promise<void> => {
      // As the server does to the provider's session that sets it: before it answers, it
      // stops telling the session of the space, and the page reads again and is refused.
      closeOwnSpace(world, backend.userId, password);
      world.tellUsers([backend.userId], { kind: 'resync' });
      await settle();
      between.push(workspace.entered);
    };
    await setFirstPassword(workspace, set, 'a long password');
    // Not closed on the way: the reader is shown no locked space before the open one.
    expect(between).toEqual(['ready']);
    expect(workspace.showing).toBe(false);
    expect(workspace.me?.hasPassword).toBe(true);
    expect(workspace.entered).toBe('ready');
    expect(workspace.info?.id).toBe(own);
    expect(workspace.containers.map((one) => one.id)).toEqual([board.id]);
    expect(workspace.workspaces.some((one) => one.passwordRequired)).toBe(false);
  });

  it('leaves the page as it was where the server refuses the first password', async () => {
    const { backend, workspace } = setup();
    const own = await ownSpace(backend);
    await workspace.load(own);
    const refused = new BackendError(403, { reason: 'fresh_sign_in_required' });
    await expect(
      setFirstPassword(workspace, () => Promise.reject(refused), 'a long password'),
    ).rejects.toBe(refused);
    expect(workspace.showing).toBe(false);
    expect(workspace.entered).toBe('ready');
    expect(workspace.me?.hasPassword).not.toBe(true);
  });

  it('closes on the page that gave the account its first password where the session cannot show it', async () => {
    const { backend, world, workspace } = setup();
    const own = await ownSpace(backend);
    await workspace.load(own);
    // Another password than the account has by then: set again elsewhere meanwhile.
    const set = (): Promise<void> => {
      closeOwnSpace(world, backend.userId, 'a long password');
      return Promise.resolve();
    };
    await setFirstPassword(workspace, set, 'the one before it');
    expect(workspace.showing).toBe(false);
    expect(workspace.me?.hasPassword).toBe(true);
    expect(workspace.entered).toBe('password');
    expect(workspace.closed?.id).toBe(own);
    // The shared workspace is as it was.
    expect(
      workspace.workspaces.find((one) => one.id === world.workspace.id)?.passwordRequired,
    ).toBe(undefined);
  });

  it('shows a changed password where the own space asked the session for it, and enters no other workspace for it', async () => {
    const { backend, world, workspace } = setup();
    const own = await ownSpace(backend);
    closeOwnSpace(world, backend.userId, 'the new password');
    await workspace.load(own);
    expect(workspace.entered).toBe('password');
    await passwordChanged(workspace, 'the new password');
    expect(workspace.entered).toBe('ready');
    expect(workspace.info?.id).toBe(own);

    closeOwnSpace(world, backend.userId, 'a third password');
    await workspace.enter(world.workspace.id);
    const entered = workspace.info;
    await passwordChanged(workspace, 'a third password');
    expect(workspace.info).toBe(entered);
    expect(workspace.workspaces.some((one) => one.passwordRequired)).toBe(false);
  });

  it('opens where it is to the session that gives the password, and to no other password', async () => {
    const { backend, other, world, workspace } = setup();
    const own = await ownSpace(backend);
    const board = await boardIn(backend, own, 'Mine');
    closeOwnSpace(world, backend.userId, 'a long password');
    await workspace.load(own);
    expect(workspace.entered).toBe('password');

    const wrong: unknown = await confirmPassword(workspace, 'not it at all').catch(
      (error: unknown) => error,
    );
    expect(wrong).toMatchObject({ status: 400, reason: 'invalid_credentials' });
    expect(wrong).toHaveProperty('errors.0.pointer', '/password');
    expect(workspace.entered).toBe('password');
    expect(await statusOf(backend.boards(own))).toBe(401);

    await confirmPassword(workspace, 'a long password');
    // The same page and the same reader: the space is entered, and read.
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('ready');
    expect(workspace.info?.id).toBe(own);
    expect(workspace.closed).toBeUndefined();
    expect(workspace.containers.map((one) => one.id)).toEqual([board.id]);
    expect(workspace.workspaces.find((one) => one.id === own)?.passwordRequired).toBe(false);
    // What it opened before, it opens still; and it is nobody else's password.
    expect(await statusOf(backend.boards(world.workspace.id))).toBe('ok');
    expect(await statusOf(other.confirmPassword('a long password'))).toBe(409);
    expect(await statusOf(other.boards(own))).toBe(404);
  });

  it('signs out instead of locking the space when the session truly ends during setFirstPassword or passwordChanged', async () => {
    const { backend, workspace } = setup();
    const own = await ownSpace(backend);
    await workspace.load(own);
    const expired = new BackendError(401, { detail: 'session expired' });
    const original = backend.workspaces.bind(backend);
    backend.workspaces = () => Promise.reject(expired);
    try {
      await setFirstPassword(workspace, () => Promise.resolve(), 'a long password');
    } finally {
      backend.workspaces = original;
    }
    expect(workspace.showing).toBe(false);
    expect(workspace.status).toBe('signedOut');

    // If set() itself fails with sessionOver, status updates AND callers receive the rejection.
    const second = setup();
    await second.workspace.load(await ownSpace(second.backend));
    await expect(
      setFirstPassword(second.workspace, () => Promise.reject(expired), 'a long password'),
    ).rejects.toBe(expired);
    expect(second.workspace.showing).toBe(false);
    expect(second.workspace.status).toBe('signedOut');

    // passwordChanged also reports genuine session loss (including account_deletion_pending).
    const third = setup();
    await third.workspace.load(await ownSpace(third.backend));
    const leaving = new BackendError(401, { reason: 'account_deletion_pending' });
    third.backend.workspaces = () => Promise.reject(leaving);
    await passwordChanged(third.workspace, 'a long password');
    expect(third.workspace.status).toBe('leaving');
  });
});
