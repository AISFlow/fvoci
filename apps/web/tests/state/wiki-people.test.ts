/**
 * The people of a wiki (`state/wiki.svelte.ts`, `state/member-writes.ts`)
 * over the backend in memory: an open wiki is read by whoever works in the
 * workspace without joining it, and their first write joins it; made private,
 * it is gone for who read it so; its admins open and close it, set the role
 * of who joins, and add, change and take off its members, a refused change
 * put back.
 */
import { describe, expect, it } from 'vitest';

import { BackendError, WikiStore, Workspace } from '$lib/state/index.ts';
import { acceptedRoute } from '../../src/app/accepted-route.ts';

import { settle, setup } from './support.ts';

/** A wiki of the reader's, with a page, and the other person's workspace beside. */
async function wikiOf(visibility: 'open' | 'private') {
  const made = setup();
  await made.workspace.load();
  const workspaceId = made.world.workspace.id;
  const wiki = await made.backend.wiki.createWiki(workspaceId, { title: 'Handbook', visibility });
  const page = await made.backend.wiki.createPage(wiki.id, { title: 'Welcome' });
  const theirs = new Workspace({ backend: made.other, locale: () => 'en' });
  await theirs.load();
  return { ...made, workspaceId, wikiId: wiki.id, pageId: page.id, theirs };
}

describe('an open wiki, read without joining', () => {
  it('is read as its viewers read it, and written as its members do: the first write joins it', async () => {
    const made = await wikiOf('open');
    const store = new WikiStore(made.theirs, made.wikiId);
    await store.load();
    expect([store.status, store.joinable, store.role, store.canEdit]).toEqual([
      'ready',
      true,
      '',
      true,
    ]);
    expect(store.pages.map((page) => page.title)).toEqual(['Welcome']);
    await expect(made.other.wiki.createPage(made.wikiId, { title: 'Mine' })).rejects.toThrow(
      BackendError,
    );

    // What its members write reaches who reads it unjoined.
    await made.backend.wiki.patchPage(made.wikiId, made.pageId, { title: 'Welcome all' });
    await settle();
    expect(store.page(made.pageId)?.title).toBe('Welcome all');

    const mine = await store.createPage({ title: 'Mine' });
    expect(mine?.title).toBe('Mine');
    expect([store.joinable, store.role, store.canEdit]).toEqual([false, 'editor', true]);
    expect(made.theirs.problems).toHaveLength(0);
    store.close();
  });

  it('whose least role only views is read only, and joined by its banner', async () => {
    const made = await wikiOf('open');
    await made.backend.patchContainer(made.wikiId, { minimumRole: 'viewer' });
    const store = new WikiStore(made.theirs, made.wikiId);
    await store.load();
    expect([store.joinable, store.joinsOnWrite, store.canEdit]).toEqual([true, false, false]);
    expect(await store.join()).toBe(true);
    expect([store.joinable, store.role]).toEqual([false, 'viewer']);
    store.close();
  });

  it('has its pages named by references and found by their search; a private one’s not', async () => {
    for (const [visibility, seen] of [
      ['open', [true, true]],
      ['private', [false, false]],
    ] as const) {
      const made = await wikiOf(visibility);
      const resolved = await made.other.references.resolve(made.workspaceId, [made.pageId]);
      const found = await made.other.references.search(made.workspaceId, 'Welcome');
      expect(
        [resolved, found].map((list) => list.some((block) => block.id === made.pageId)),
      ).toEqual(seen);
    }
  });

  it('made private, is gone for who read it unjoined, and stays for its members', async () => {
    const made = await wikiOf('open');
    const store = new WikiStore(made.theirs, made.wikiId);
    await store.load();
    const own = new WikiStore(made.workspace, made.wikiId);
    await own.load();
    await own.setOpen(false);
    await settle();
    expect([store.missing, store.status]).toEqual([true, 'failed']);
    expect([own.missing, own.wiki?.visibility]).toEqual([false, 'private']);
    store.close();
    own.close();
  });

  it('taken off it, a member reads it on as whoever works there, and writing joins it again', async () => {
    const made = await wikiOf('open');
    await made.backend.addMember(made.wikiId, made.other.userId, 'editor');
    const store = new WikiStore(made.theirs, made.wikiId);
    await store.load();
    expect(store.role).toBe('editor');
    await made.backend.removeMember(made.wikiId, made.other.userId);
    await settle();
    expect([store.missing, store.joinable, store.joinsOnWrite]).toEqual([false, true, true]);
    store.close();
  });
  it('tells who watches it unjoined each change, that it is gone once when made private, and nothing after', async () => {
    const made = await wikiOf('open');
    const told: string[] = [];
    const stop = made.other.watchContainer(made.wikiId, (change) => told.push(change.kind));
    await made.backend.wiki.patchPage(made.wikiId, made.pageId, { title: 'Once' });
    await settle();
    expect(told).toEqual(['pages']);
    await made.backend.patchContainer(made.wikiId, { visibility: 'private' });
    await made.backend.wiki.patchPage(made.wikiId, made.pageId, { title: 'Twice' });
    await settle();
    expect(told).toEqual(['pages', 'containerDeleted']);
    stop();
  });
});

describe('the comments of an open wiki read without joining', () => {
  it('are read with what they said before, and the first one written joins the wiki', async () => {
    const made = await wikiOf('open');
    const store = new WikiStore(made.theirs, made.wikiId);
    await store.load();
    const comments = store.showPage(made.pageId)?.comments;
    await settle();
    if (!comments) throw new Error('no page shown');
    expect([
      comments.canComment,
      comments.canEditComment(made.other.userId),
      comments.canReadCommentHistory,
    ]).toEqual([true, true, true]);
    await comments.addComment({
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Hello' }] }],
    });
    expect(store.role).toBe('editor');
    expect(comments.comments.map((comment) => comment.createdBy)).toEqual([made.other.userId]);
    store.close();
  });
});

describe('a wiki shared by its admins', () => {
  it('opens and closes, sets the role of who joins, and is undone', async () => {
    const made = await wikiOf('private');
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    await store.setOpen(true);
    await store.setMinimumRole('commenter');
    expect([store.wiki?.visibility, store.wiki?.minimumRole]).toEqual(['open', 'commenter']);
    expect((await made.backend.container(made.wikiId)).minimumRole).toBe('commenter');
    await made.workspace.undoLast();
    await made.workspace.undoLast();
    expect([store.wiki?.visibility, store.wiki?.minimumRole]).toEqual(['private', 'editor']);
    store.close();
  });

  it('keeps a change told while it was read over the older reading', async () => {
    const made = await wikiOf('open');
    const read = made.other.container.bind(made.other);
    let release = (): void => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    made.other.container = async (boardId) => {
      const board = await read(boardId);
      await held;
      return board;
    };
    const store = new WikiStore(made.theirs, made.wikiId);
    const loading = store.load();
    await settle();
    await made.backend.patchContainer(made.wikiId, { minimumRole: 'viewer' });
    await settle();
    release();
    await loading;
    expect([store.status, store.wiki?.minimumRole]).toEqual(['ready', 'viewer']);
    store.close();
  });

  it('adds, changes and takes off its members, a refused change put back and told', async () => {
    const made = await wikiOf('private');
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const otherId = made.other.userId;
    const roleOfOther = () => {
      const member = store.members.find((one) => one.userId === otherId);
      return member ? member.role : 'none';
    };

    await store.addMember(otherId, 'viewer');
    expect(roleOfOther()).toBe('viewer');
    await store.setMemberRole(otherId, 'editor');
    expect(roleOfOther()).toBe('editor');

    made.backend.failNext('setMemberRole', new BackendError(500, {}));
    const problems = made.workspace.problems.length;
    await store.setMemberRole(otherId, 'viewer');
    await settle();
    expect(roleOfOther()).toBe('editor');
    expect(made.workspace.problems.length).toBe(problems + 1);

    await store.removeMember(otherId);
    expect(roleOfOther()).toBe('none');
    expect((await made.backend.members(made.wikiId)).map((one) => one.userId)).toEqual([
      made.backend.userId,
    ]);
    store.close();
  });
});

describe('an invitation accepted', () => {
  it('leads to its wiki at a wiki’s address, and to its board at a board’s', () => {
    const workspaceId = 'w';
    expect(acceptedRoute({ workspaceId, containerId: 'a', containerKind: 'wiki' })).toEqual({
      kind: 'wiki',
      workspaceId,
      wikiId: 'a',
    });
    expect(acceptedRoute({ workspaceId, containerId: 'b', containerKind: 'board' })).toEqual({
      kind: 'board',
      workspaceId,
      boardId: 'b',
    });
    // An answer that does not say: a board's address, which says what there is to say.
    expect(acceptedRoute({ workspaceId, containerId: 'c' })).toEqual({
      kind: 'board',
      workspaceId,
      boardId: 'c',
    });
    expect(acceptedRoute({ workspaceId })).toEqual({ kind: 'workspace', workspaceId });
  });
});
