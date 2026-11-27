/**
 * The store of an open wiki (`state/wiki.svelte.ts`) over the backend in
 * memory: it reads a wiki and its pages, follows what somebody else does to
 * them, takes the pages below a deleted page along, shows its writes at once
 * and takes them back when refused, and undoes making a page and renaming one.
 */
import { describe, expect, it } from 'vitest';

import { pageRows } from '$lib/logic/page-tree.ts';
import { BackendError, WikiStore, Workspace, type WikiPage } from '$lib/state/index.ts';

import { settle, setup, text } from './support.ts';

async function openWiki(visibility: 'open' | 'private' = 'private') {
  const made = setup();
  await made.workspace.load();
  const workspaceId = made.world.workspace.id;
  const wiki = await made.backend.wiki.createWiki(workspaceId, { title: 'Handbook', visibility });
  const store = new WikiStore(made.workspace, wiki.id);
  await store.load();
  return { ...made, workspaceId, wikiId: wiki.id, store };
}

const titles = (store: WikiStore): string[] =>
  pageRows<WikiPage>(store.tree, () => true).map(
    (row) => `${'  '.repeat(row.depth)}${row.page.title}`,
  );

describe('an open wiki', () => {
  it('reads the wiki, its pages as a tree and what the reader may do', async () => {
    const made = setup();
    await made.workspace.load();
    const wiki = await made.backend.wiki.createWiki(made.world.workspace.id, {
      title: 'Handbook',
      visibility: 'private',
    });
    const top = await made.backend.wiki.createPage(wiki.id, { title: 'Top' });
    await made.backend.wiki.createPage(wiki.id, { title: 'Below', parentId: top.id });
    const store = new WikiStore(made.workspace, wiki.id);
    await store.load();
    expect(store.status).toBe('ready');
    expect(store.wiki?.title).toBe('Handbook');
    expect(titles(store)).toEqual(['Top', '  Below']);
    expect([store.role, store.canEdit, store.canManage]).toEqual(['admin', true, true]);
  });

  it('is not there at the address of a board, nor a board at a wiki’s', async () => {
    const { workspace, backend, world } = setup();
    await workspace.load();
    const [board] = await backend.boards(world.workspace.id);
    const store = new WikiStore(workspace, board?.id ?? '');
    await store.load();
    expect(store.missing).toBe(true);
    expect(store.status).toBe('failed');
  });

  it('reads an open wiki unjoined and offers it to join, to write in it', async () => {
    const made = await openWiki('open');
    // The other person's workspace over the same world.
    const otherWorkspace = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWorkspace.load();
    const store = new WikiStore(otherWorkspace, made.wikiId);
    await store.load();
    expect([store.joinable, store.missing, store.status]).toEqual([true, false, 'ready']);
    expect(await store.join()).toBe(true);
    expect([store.joinable, store.status, store.role]).toEqual([false, 'ready', 'editor']);
    store.close();
  });

  it('follows the pages somebody else makes, renames and deletes, with those below', async () => {
    const made = await openWiki();
    await made.backend.addMember(made.wikiId, made.other.userId, 'editor');
    const top = await made.other.wiki.createPage(made.wikiId, { title: 'Top' });
    const below = await made.other.wiki.createPage(made.wikiId, {
      title: 'Below',
      parentId: top.id,
    });
    await made.other.wiki.createPage(made.wikiId, { title: 'Deeper', parentId: below.id });
    await made.other.wiki.createPage(made.wikiId, { title: 'Other' });
    await settle();
    expect(titles(made.store)).toEqual(['Top', '  Below', '    Deeper', 'Other']);

    await made.other.wiki.patchPage(made.wikiId, top.id, { title: 'Renamed' });
    await settle();
    expect(titles(made.store)[0]).toBe('Renamed');

    const shown = made.store.showPage(below.id);
    await settle();
    expect(shown?.status).toBe('ready');
    await made.other.wiki.deletePage(made.wikiId, top.id);
    await settle();
    expect(titles(made.store)).toEqual(['Other']);
    expect(shown?.missing).toBe(true);
    expect(shown?.removed).toEqual({ by: 'others', title: 'Below' });
  });

  it('a page shown and found gone first (its room let the reader go) is told gone by others when heard', async () => {
    const made = await openWiki();
    await made.backend.addMember(made.wikiId, made.other.userId, 'editor');
    const page = await made.other.wiki.createPage(made.wikiId, { title: 'Theirs' });
    await settle();
    const shown = made.store.showPage(page.id);
    await settle();
    shown?.gone();
    expect([shown?.missing, shown?.removed]).toEqual([true, undefined]);
    await made.other.wiki.deletePage(made.wikiId, page.id);
    await settle();
    expect(shown?.removed).toEqual({ by: 'others', title: 'Theirs' });
  });

  it('undoes making a page by deleting it, and the page shown says the reader did', async () => {
    const made = await openWiki();
    const page = await made.store.createPage({ title: 'Draft' });
    if (!page) throw new Error('no page');
    const shown = made.store.showPage(page.id);
    await settle();
    expect(titles(made.store)).toEqual(['Draft']);
    await made.workspace.undoLast();
    expect(titles(made.store)).toEqual([]);
    expect(shown?.removed).toEqual({ by: 'reader', title: 'Draft' });
    expect(await made.backend.wiki.pages(made.wikiId)).toEqual([]);
  });

  it('shows a new title at once, puts the old one back on undo, and on a refusal', async () => {
    const made = await openWiki();
    const page = await made.store.createPage({ title: 'First' });
    if (!page) throw new Error('no page');
    const renaming = made.store.changePage(page.id, { title: 'Second' });
    expect(made.store.page(page.id)?.title).toBe('Second');
    await renaming;
    await made.workspace.undoLast();
    expect(made.store.page(page.id)?.title).toBe('First');
    expect((await made.backend.wiki.page(made.wikiId, page.id)).title).toBe('First');

    made.backend.wiki.failNext('patchPage', new BackendError(500, {}));
    await made.store.changePage(page.id, { icon: '📘' });
    expect(made.store.page(page.id)?.icon).toBe('');
    expect(made.workspace.problems.at(-1)?.action).toBe('save');
  });

  it('takes a deleted page and those below off at once, and back when refused', async () => {
    const made = await openWiki();
    const top = await made.store.createPage({ title: 'Top' });
    if (!top) throw new Error('no page');
    await made.store.createPage({ title: 'Below', parentId: top.id });
    made.backend.wiki.failNext('deletePage', new BackendError(500, {}));
    const shown = made.store.showPage(top.id);
    await settle();
    expect(await made.store.deletePage(top.id)).toBe(false);
    expect(titles(made.store)).toEqual(['Top', '  Below']);
    expect(shown?.missing).toBe(false);
    expect(await made.store.deletePage(top.id)).toBe(true);
    expect(titles(made.store)).toEqual([]);
    expect(shown?.removed).toEqual({ by: 'reader', title: 'Top' });
  });

  it('keeps a page made when undone after it was written in, or has pages below it', async () => {
    const made = await openWiki();
    const written = await made.store.createPage({ title: 'Written' });
    if (!written) throw new Error('no page');
    await made.backend.wiki.patchPage(made.wikiId, written.id, {
      body: text('kept'),
      readAt: written.updatedAt,
    });
    const parent = await made.store.createPage({ title: 'Parent' });
    if (!parent) throw new Error('no page');
    await made.backend.wiki.createPage(made.wikiId, { title: 'Child', parentId: parent.id });
    await settle();
    await made.workspace.undoLast();
    await made.workspace.undoLast();
    expect(titles(made.store)).toEqual(['Written', 'Parent', '  Child']);
    expect(made.workspace.problems).toEqual([]);
    // Done again, they are the same pages: none is made anew.
    await made.workspace.redoLast();
    await made.workspace.redoLast();
    await settle();
    expect(titles(made.store)).toEqual(['Written', 'Parent', '  Child']);
    expect(await made.backend.wiki.pages(made.wikiId)).toHaveLength(3);
  });

  it('undoes a deletion by bringing the page back, and then what came before it', async () => {
    const made = await openWiki();
    const page = await made.store.createPage({ title: 'Draft' });
    if (!page) throw new Error('no page');
    await made.store.changePage(page.id, { title: 'Named' });
    expect(await made.store.deletePage(page.id)).toBe(true);
    await made.workspace.undoLast();
    expect(titles(made.store)).toEqual(['Named']);
    await made.workspace.undoLast();
    expect(made.workspace.problems).toEqual([]);
    expect(titles(made.store)).toEqual(['Draft']);
  });

  it('changes the wiki’s title, also in the sidebar, and undoes it', async () => {
    const made = await openWiki();
    await settle();
    await made.store.changeWiki({ title: 'Guide' });
    expect(made.store.wiki?.title).toBe('Guide');
    expect(made.workspace.containers.find((board) => board.id === made.wikiId)?.title).toBe(
      'Guide',
    );
    await made.workspace.undoLast();
    expect(made.store.wiki?.title).toBe('Handbook');
  });
});

describe('the backend in memory keeps a wiki from the operations of cards', () => {
  it('as the server answers them: not there', async () => {
    const made = await openWiki();
    const page = await made.backend.wiki.createPage(made.wikiId, { title: 'Page' });
    const status = async (call: Promise<unknown>): Promise<number> =>
      call.then(
        () => 200,
        (error: unknown) => (error instanceof BackendError ? error.status : -1),
      );
    expect(await status(made.backend.createCard(made.wikiId, {}))).toBe(404);
    expect(
      await status(made.backend.createView(made.wikiId, { title: 'v', fields: {} as never })),
    ).toBe(404);
    expect(await status(made.backend.deleteBlock(made.wikiId, page.id))).toBe(404);
    // Copied whole as a board is, but as no template.
    expect(await status(made.backend.duplicateContainer(made.wikiId, true, made.workspaceId))).toBe(
      400,
    );
    expect(await status(made.backend.deletedCards(made.wikiId))).toBe(404);
    // Saved as an archive as a board is.
    expect(await status(made.backend.exportContainer(made.wikiId))).toBe(200);
    // A refused change of a page changes nothing of it.
    expect(
      await status(made.backend.wiki.patchPage(made.wikiId, page.id, { title: 'New', icon: 'ab' })),
    ).toBe(400);
    expect((await made.backend.wiki.page(made.wikiId, page.id)).title).toBe('Page');
  });
});
