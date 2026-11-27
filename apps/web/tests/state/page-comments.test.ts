/**
 * The comments on a wiki's page and the versions of its body: the backend in
 * memory keeps the server's rules (`crates/server/src/api/page_comments.rs`,
 * `page_versions.rs`), and the page shown (`PageStore.comments`) follows what
 * somebody else writes. A file going up for the body of the page shown is
 * given up when the page is left, as a card's is when it closes.
 */
import { describe, expect, it } from 'vitest';

import { pageVersions, VersionsReading } from '$lib/features/card/body-versions.svelte.ts';
import type { MemberRole } from '$lib/logic/wire.ts';
import { BackendError, WikiStore, Workspace, type Change } from '$lib/state/index.ts';

import { settle, setup, text } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<number> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error.status;
    throw error;
  }
  throw new Error('the call did not reject');
};

/** A wiki of the reader with a page, the other person on it with `role`. */
async function aPage(role: Exclude<MemberRole, '' | 'admin'> = 'commenter') {
  const made = setup();
  const workspaceId = made.world.workspace.id;
  const wiki = await made.backend.wiki.createWiki(workspaceId, {
    title: 'Handbook',
    visibility: 'open',
  });
  await made.backend.patchContainer(wiki.id, { minimumRole: role });
  await made.other.joinContainer(wiki.id);
  const page = await made.backend.wiki.createPage(wiki.id, { title: 'Page' });
  return { ...made, workspaceId, wikiId: wiki.id, pageId: page.id };
}

describe('the comments on a page, in memory', () => {
  it('are written by commenters, changed and deleted by their author, anyone’s by the admins', async () => {
    const made = await aPage('commenter');
    const [mine, theirs] = [made.backend.wiki, made.other.wiki];
    const at = [made.wikiId, made.pageId] as const;
    const admins = await mine.createComment(...at, text('the admin’s'));
    const own = await theirs.createComment(...at, text('mine'));
    expect([own.parentId, own.containerId, own.createdBy]).toEqual([
      made.pageId,
      made.wikiId,
      made.other.userId,
    ]);
    const changed = await theirs.patchComment(...at, own.id, {
      body: text('mine, again'),
      readAt: own.updatedAt,
    });
    expect(changed.editedAt).toBeGreaterThan(0);
    const before = await theirs.commentVersions(...at, own.id);
    expect(before.map((version) => version.body)).toEqual([text('mine')]);
    const write = { body: text('no'), readAt: admins.updatedAt };
    expect(await rejection(theirs.patchComment(...at, admins.id, write))).toBe(403);
    expect(await rejection(theirs.deleteComment(...at, admins.id))).toBe(403);
    // A stale text and an empty one.
    expect(
      await rejection(
        theirs.patchComment(...at, own.id, { body: text('x'), readAt: own.updatedAt }),
      ),
    ).toBe(409);
    expect(await rejection(theirs.createComment(...at, { type: 'doc', content: [] }))).toBe(400);
    // The admin's way with somebody else's.
    await mine.patchComment(...at, own.id, { body: text('moderated'), readAt: changed.updatedAt });
    const another = await theirs.createComment(...at, text('mine, deleted'));
    await theirs.deleteComment(...at, another.id);
    await mine.deleteComment(...at, own.id);
    const left = await mine.comments(...at);
    expect(left.map((comment) => comment.id)).toEqual([admins.id]);
    // Deleted on its own, what it said is still there for the members.
    expect((await theirs.commentVersions(...at, own.id)).length).toBe(2);
  });

  it('are read by a viewer, who writes none', async () => {
    const made = await aPage('viewer');
    const at = [made.wikiId, made.pageId] as const;
    const comment = await made.backend.wiki.createComment(...at, text('read me'));
    expect((await made.other.wiki.comments(...at)).map((one) => one.id)).toEqual([comment.id]);
    expect(await rejection(made.other.wiki.createComment(...at, text('no')))).toBe(403);
    expect(await rejection(made.other.wiki.deleteComment(...at, comment.id))).toBe(403);
  });

  it('are reached through their own page only, and go to the trash with it', async () => {
    const made = await aPage();
    const { wiki } = made.backend;
    const other = await wiki.createPage(made.wikiId, { title: 'Other' });
    const comment = await wiki.createComment(made.wikiId, made.pageId, text('here'));
    expect(await rejection(wiki.commentVersions(made.wikiId, other.id, comment.id))).toBe(404);
    expect(await rejection(wiki.deleteComment(made.wikiId, other.id, comment.id))).toBe(404);
    expect(await rejection(wiki.commentVersions(made.wikiId, made.pageId, other.id))).toBe(404);
    const [board] = await made.backend.boards(made.workspaceId);
    expect(await rejection(wiki.comments(board?.id ?? '', made.pageId))).toBe(404);
    await wiki.deletePage(made.wikiId, made.pageId);
    expect(made.world.blocks.has(comment.id)).toBe(false);
    expect(await rejection(wiki.comments(made.wikiId, made.pageId))).toBe(404);
  });

  it('are told to whoever listens to the wiki', async () => {
    const made = await aPage();
    const told: Change[] = [];
    made.backend.watchContainer(made.wikiId, (change) => told.push(change));
    const comment = await made.other.wiki.createComment(made.wikiId, made.pageId, text('hi'));
    await made.other.wiki.deleteComment(made.wikiId, made.pageId, comment.id);
    await settle();
    expect(told.map((change) => change.kind)).toEqual(['blocks', 'blocksDeleted']);
  });

  it('come to the page shown as somebody else writes, changes and deletes them', async () => {
    const made = await aPage();
    await made.workspace.load();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const page = store.showPage(made.pageId);
    await settle();
    const comments = page?.comments;
    if (!comments) throw new Error('no page shown');
    expect([comments.canComment, comments.canEditComment(made.other.userId)]).toEqual([true, true]);
    const at = [made.wikiId, made.pageId] as const;
    const theirs = await made.other.wiki.createComment(...at, text('from them'));
    await settle();
    expect(comments.comments.map((comment) => comment.body)).toEqual([text('from them')]);
    await made.other.wiki.patchComment(...at, theirs.id, {
      body: text('changed'),
      readAt: theirs.updatedAt,
    });
    await settle();
    expect(comments.comment(theirs.id)?.body).toEqual(text('changed'));
    await made.other.wiki.deleteComment(...at, theirs.id);
    await settle();
    expect(comments.comments).toEqual([]);
    // Brought back by them, it is back here too.
    await made.other.wiki.restoreComment(...at, theirs.id);
    await settle();
    expect(comments.comments.map((comment) => comment.id)).toEqual([theirs.id]);
    await made.other.wiki.deleteComment(...at, theirs.id);
    await settle();
    expect(await comments.addComment(text('mine'))).toBe(true);
    expect(comments.comments.map((comment) => comment.createdBy)).toEqual([made.backend.userId]);
    // Deleted here, "Undo" brings it back, and "Redo" takes it away again.
    const [mine] = comments.comments;
    if (!mine) throw new Error('no comment of mine');
    await comments.deleteComment(mine.id);
    expect(comments.comments).toEqual([]);
    await made.workspace.undo.undo();
    await settle();
    expect(comments.comments.map((comment) => comment.id)).toEqual([mine.id]);
    await made.workspace.undo.redo();
    await settle();
    expect(comments.comments).toEqual([]);
    store.close();
  });

  it('are not taken back from news older than their deletion, and are once they are back', async () => {
    const made = await aPage();
    await made.workspace.load();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const comments = store.showPage(made.pageId)?.comments;
    await settle();
    if (!comments) throw new Error('no page shown');
    const at = [made.wikiId, made.pageId] as const;
    const theirs = await made.other.wiki.createComment(...at, text('theirs'));
    await settle();
    await made.other.wiki.deleteComment(...at, theirs.id);
    await settle();
    // News of it from before its deletion, heard after: read again, it is gone.
    comments.receive(theirs);
    await settle();
    await settle();
    expect(comments.comments).toEqual([]);
    // Brought back by them: back here too, and a reading keeps it.
    await made.other.wiki.restoreComment(...at, theirs.id);
    await settle();
    await settle();
    expect(comments.comments.map((comment) => comment.id)).toEqual([theirs.id]);
    await comments.reread();
    expect(comments.comments.map((comment) => comment.id)).toEqual([theirs.id]);
    store.close();
  });

  it('come back onto their page by who may delete them, as a card’s', async () => {
    const made = await aPage('commenter');
    const [mine, theirs] = [made.backend.wiki, made.other.wiki];
    const at = [made.wikiId, made.pageId] as const;
    const own = await theirs.createComment(...at, text('theirs'));
    await theirs.deleteComment(...at, own.id);
    expect((await theirs.restoreComment(...at, own.id)).id).toBe(own.id);
    expect(await rejection(theirs.restoreComment(...at, own.id))).toBe(404);
    // Deleted by the admin: not the author's to bring back; the admin's own neither.
    await mine.deleteComment(...at, own.id);
    expect(await rejection(theirs.restoreComment(...at, own.id))).toBe(403);
    const admins = await mine.createComment(...at, text('the admin’s'));
    await mine.deleteComment(...at, admins.id);
    expect(await rejection(theirs.restoreComment(...at, admins.id))).toBe(403);
    await mine.restoreComment(...at, own.id);
    // Under another page, not there; gone with its page, it comes back with the page only.
    const other = await mine.createPage(made.wikiId, { title: 'Other' });
    expect(await rejection(mine.restoreComment(made.wikiId, other.id, admins.id))).toBe(404);
    await mine.deletePage(made.wikiId, made.pageId);
    expect(await rejection(mine.restoreComment(...at, admins.id))).toBe(404);
    const [listed] = await mine.deletedPages(made.wikiId);
    expect(listed?.below).toBe(0);
    await mine.restorePage(made.wikiId, made.pageId);
    expect((await mine.comments(...at)).map((comment) => comment.id)).toEqual([own.id]);
  });

  it('are the commenter’s own to change, as the wiki says', async () => {
    const made = await aPage('commenter');
    const theirs = new Workspace({ backend: made.other, locale: () => 'en' });
    await theirs.load();
    const store = new WikiStore(theirs, made.wikiId);
    await store.load();
    const comments = store.showPage(made.pageId)?.comments;
    await settle();
    if (!comments) throw new Error('no page shown');
    expect(comments.canComment).toBe(true);
    expect(comments.canEditComment(made.other.userId)).toBe(true);
    expect(comments.canEditComment(made.backend.userId)).toBe(false);
    expect(comments.canReadCommentHistory).toBe(true);
    store.close();
  });
});

describe('the versions of a page’s body, in memory', () => {
  it('are kept around a body written whole, read by members and restored by editors', async () => {
    const made = await aPage('commenter');
    const { wiki } = made.backend;
    let read = await wiki.page(made.wikiId, made.pageId);
    read = await wiki.patchPage(made.wikiId, made.pageId, {
      body: text('first'),
      readAt: read.updatedAt,
    });
    await wiki.patchPage(made.wikiId, made.pageId, {
      body: text('second'),
      readAt: read.updatedAt,
    });
    const listed = await made.other.wiki.bodyVersions.list(made.wikiId, made.pageId);
    expect(listed.map((version) => version.isCurrent)).toEqual([true, false]);
    const [, older] = listed;
    if (!older) throw new Error('one version');
    const content = await made.other.wiki.bodyVersions.read(
      made.wikiId,
      made.pageId,
      older.version,
    );
    expect(content.body).toEqual(text('first'));
    expect(
      await rejection(
        made.other.wiki.bodyVersions.restore(made.wikiId, made.pageId, older.version),
      ),
    ).toBe(403);
    const told: Change[] = [];
    made.backend.watchContainer(made.wikiId, (change) => told.push(change));
    await wiki.bodyVersions.restore(made.wikiId, made.pageId, older.version);
    await settle();
    expect((await wiki.page(made.wikiId, made.pageId)).body).toEqual(text('first'));
    expect(told.map((change) => change.kind)).toContain('pages');
    // A card's versions are no page's.
    expect(await rejection(made.backend.bodyVersions.list(made.wikiId, made.pageId))).toBe(404);
  });

  it('are read for the page shown, as a card’s are for the card', async () => {
    const made = await aPage();
    await made.workspace.load();
    const read = await made.backend.wiki.page(made.wikiId, made.pageId);
    await made.backend.wiki.patchPage(made.wikiId, made.pageId, {
      body: text('kept'),
      readAt: read.updatedAt,
    });
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const page = store.showPage(made.pageId);
    if (!page) throw new Error('no page shown');
    const reading = new VersionsReading(pageVersions(page));
    await settle();
    expect(reading.status).toBe('ready');
    expect(reading.versions.length).toBe(1);
    store.close();
  });

  it('are restored by an editor of the wiki, and refused to a reader in the server’s order', async () => {
    const made = await aPage('editor');
    const { wiki } = made.backend;
    const read = await wiki.page(made.wikiId, made.pageId);
    const first = await wiki.patchPage(made.wikiId, made.pageId, {
      body: text('first'),
      readAt: read.updatedAt,
    });
    await wiki.patchPage(made.wikiId, made.pageId, {
      body: text('second'),
      readAt: first.updatedAt,
    });
    const [, older] = await made.other.wiki.bodyVersions.list(made.wikiId, made.pageId);
    if (!older) throw new Error('two versions');
    await made.other.wiki.bodyVersions.restore(made.wikiId, made.pageId, older.version);
    expect((await wiki.page(made.wikiId, made.pageId)).body).toEqual(text('first'));
    // A reader is refused before the page is looked for, as the server asks the wiki first.
    const viewer = await aPage('viewer');
    expect(
      await rejection(viewer.other.wiki.bodyVersions.restore(viewer.wikiId, 'no-such-page', 1)),
    ).toBe(403);
  });
});

describe('the body of the page shown', () => {
  it('gives up a file going up for it when the page is left: the stack tells that until dismissed', async () => {
    const made = await aPage();
    const store = new WikiStore(made.workspace, made.wikiId);
    await store.load();
    const page = store.showPage(made.pageId);
    if (!page) throw new Error('no page shown');
    made.backend.latency = 5;
    const told = made.workspace.problems.length;
    const uploading = page.uploadFile(new File(['%PDF'], 'minutes.pdf'));
    expect(made.workspace.transfers.list).toMatchObject([{ name: 'minutes.pdf', finished: false }]);
    store.showPage(undefined);
    await expect(uploading).rejects.toThrow('minutes.pdf did not go up');
    expect(made.workspace.transfers.list).toMatchObject([{ name: 'minutes.pdf', failure: 'left' }]);
    expect(made.workspace.problems).toHaveLength(told);
  });
});
