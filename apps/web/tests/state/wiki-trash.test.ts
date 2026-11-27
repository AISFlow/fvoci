/**
 * The trash of a wiki's pages: the backend in memory keeps the server's rules
 * (`crates/store/src/page_trash.rs`: the top of each deletion listed with how
 * many went with it, brought back with them, last at the top where its
 * parent is gone, held to the limits), and an open wiki undoes a deletion by
 * bringing the page back, lists its trash and follows a page brought back.
 */
import { describe, expect, it } from 'vitest';

import type { DocNode } from '$lib/editor/schema.ts';
import { pageRows, pageTree } from '$lib/logic/page-tree.ts';
import { BackendError, pageTrash, WikiStore, type WikiPage } from '$lib/state/index.ts';

import { settle, setup } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

async function openWiki() {
  const made = setup();
  await made.workspace.load();
  const workspaceId = made.world.workspace.id;
  const wiki = await made.backend.wiki.createWiki(workspaceId, {
    title: 'Handbook',
    visibility: 'private',
  });
  const page = async (title: string, parentId = '') =>
    (await made.backend.wiki.createPage(wiki.id, { title, parentId })).id;
  const store = new WikiStore(made.workspace, wiki.id);
  return { ...made, wikiId: wiki.id, page, store };
}

const titles = (pages: readonly WikiPage[]): string[] =>
  pageRows<WikiPage>(pageTree(pages), () => true).map(
    (row) => `${'  '.repeat(row.depth)}${row.page.title}`,
  );

describe('the trash of pages in memory', () => {
  it('lists the top of each deletion and brings it back with the pages below it', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    const a = await made.page('A');
    const b = await made.page('B', a);
    await made.page('C', b);
    const d = await made.page('D');
    await wikis.deletePage(made.wikiId, d);
    await wikis.deletePage(made.wikiId, a);
    const listed = await wikis.deletedPages(made.wikiId);
    expect(listed.map((item) => [item.title, item.below])).toEqual([
      ['A', 2],
      ['D', 0],
    ]);
    expect(listed[0]?.deletedBy).toBe(made.workspace.me?.id);
    expect((await wikis.deletedPages(made.wikiId, { page: 1, perPage: 1 })).length).toBe(1);
    // One that went with the page it was below comes back with that page only.
    expect((await rejection(wikis.restorePage(made.wikiId, b))).status).toBe(404);
    const back = await wikis.restorePage(made.wikiId, a);
    expect(back.id).toBe(a);
    expect(titles(await wikis.pages(made.wikiId))).toEqual(['A', '  B', '    C']);
    expect((await rejection(wikis.restorePage(made.wikiId, a))).status).toBe(404);
  });

  it('brings a page whose parent is gone back last at the top, held to the limits', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    const a = await made.page('A');
    const b = await made.page('B', a);
    await made.page('C', b);
    await made.page('D');
    await wikis.deletePage(made.wikiId, b);
    await wikis.deletePage(made.wikiId, a);
    wikis.limits = { depth: 10, pages: 2 };
    const full = await rejection(wikis.restorePage(made.wikiId, b));
    expect([full.reason, full.limit]).toEqual(['too_many_pages', 2]);
    wikis.limits = { depth: 1, pages: 100 };
    const deep = await rejection(wikis.restorePage(made.wikiId, b));
    expect([deep.reason, deep.limit]).toEqual(['page_too_deep', 1]);
    wikis.limits = { depth: 10, pages: 100 };
    await wikis.restorePage(made.wikiId, b);
    expect(titles(await wikis.pages(made.wikiId))).toEqual(['D', 'B', '  C']);
  });

  it('gives a page whose place was taken meanwhile a place of its own', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    await made.page('A');
    const b = await made.page('B');
    await wikis.deletePage(made.wikiId, b);
    await made.page('C');
    await wikis.restorePage(made.wikiId, b);
    const top = await wikis.pages(made.wikiId);
    expect(new Set(top.map((page) => page.order)).size).toBe(3);
    expect(titles(top)[0]).toBe('A');
  });
});

describe('the trash of pages in an open wiki', () => {
  it('undoes a deletion by bringing the page back with the pages below it', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B', a);
    await made.page('C');
    await made.store.load();
    expect(await made.store.deletePage(a)).toBe(true);
    expect(titles(made.store.pages)).toEqual(['C']);
    await made.workspace.undoLast();
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B', 'C']);
    expect(await made.backend.wiki.deletedPages(made.wikiId)).toEqual([]);
    await made.workspace.redoLast();
    expect(titles(made.store.pages)).toEqual(['C']);
    expect(titles(await made.backend.wiki.pages(made.wikiId))).toEqual(['C']);
  });

  it('lists the trash and shows a page brought back from it', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B', a);
    await made.store.load();
    await made.store.deletePage(a);
    const trash = pageTrash(made.store);
    await trash.read();
    expect(trash.items.map((item) => [item.title, item.below])).toEqual([['A', 1]]);
    expect(await trash.restore(a)).toBe(true);
    expect(trash.items).toEqual([]);
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B']);
  });

  it('lists a page whose deletion the backend has not answered yet', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B', a);
    await made.store.load();
    // The backend holds the deletion until it is let go.
    const wikis = made.backend.wiki;
    const deletePage = wikis.deletePage.bind(wikis);
    let letGo = (): void => undefined;
    const held = new Promise<void>((resolve) => {
      letGo = resolve;
    });
    wikis.deletePage = async (wikiId, pageId) => {
      await held;
      await deletePage(wikiId, pageId);
    };
    const deleted = made.store.deletePage(a);
    await settle();
    // Off the tree at once; the trash, opened now, is read once the deletion is in it.
    expect(titles(made.store.pages)).toEqual([]);
    const trash = pageTrash(made.store);
    const read = trash.read();
    await settle();
    expect(trash.status).toBe('loading');
    letGo();
    await read;
    expect(trash.items.map((item) => [item.title, item.below])).toEqual([['A', 1]]);
    expect(await deleted).toBe(true);
  });

  it('reads the trash all the same when a deletion under way is refused', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.store.load();
    const wikis = made.backend.wiki;
    let refuse = (): void => undefined;
    const held = new Promise<void>((_resolve, reject) => {
      refuse = () => {
        reject(new BackendError(403, undefined));
      };
    });
    wikis.deletePage = () => held;
    const deleted = made.store.deletePage(a);
    await settle();
    const trash = pageTrash(made.store);
    const read = trash.read();
    refuse();
    await read;
    expect(trash.status).toBe('ready');
    expect(trash.items).toEqual([]);
    expect(await deleted).toBe(false);
    expect(titles(made.store.pages)).toEqual(['A']);
  });

  it('follows a page somebody else brings back, and tells a refusal', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B', a);
    await made.store.load();
    await made.backend.wiki.deletePage(made.wikiId, a);
    await settle();
    expect(made.store.pages).toEqual([]);
    await made.backend.wiki.restorePage(made.wikiId, a);
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B']);

    await made.backend.wiki.deletePage(made.wikiId, a);
    made.backend.wiki.limits = { depth: 10, pages: 0 };
    const trash = pageTrash(made.store);
    await trash.read();
    expect(await trash.restore(a)).toBe(false);
    expect(trash.items.map((item) => item.id)).toEqual([a]);
    expect(made.workspace.problems.at(-1)?.action).toBe('save');
  });

  it('undoes nothing of a deletion brought back meanwhile, and goes on to the step before', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.store.load();
    await made.store.changePage(a, { title: 'Named' });
    await made.store.deletePage(a);
    const trash = pageTrash(made.store);
    await trash.read();
    await trash.restore(a);
    await settle();
    await made.workspace.undoLast();
    expect(made.workspace.problems).toEqual([]);
    expect(titles(made.store.pages)).toEqual(['Named']);
    await made.workspace.undoLast();
    expect(titles(made.store.pages)).toEqual(['A']);
  });

  it('shows the open page again once somebody brings it back', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.store.load();
    const shown = made.store.showPage(a);
    await settle();
    await made.backend.wiki.deletePage(made.wikiId, a);
    await settle();
    expect(shown?.missing).toBe(true);
    await made.backend.wiki.restorePage(made.wikiId, a);
    await settle();
    expect([shown?.missing, shown?.status]).toEqual([false, 'ready']);
  });

  it('keeps out of the tree what a reading begun before a deletion still has', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B', a);
    await made.page('C');
    await made.store.load();
    await made.store.deletePage(a);
    // The tree read anew after the undo answers only once it is done again.
    const wikis = made.backend.wiki;
    const read = wikis.pages.bind(wikis);
    let release = (): void => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    wikis.pages = async (wikiId) => {
      const pages = await read(wikiId);
      await held;
      return pages;
    };
    // And what the undo brought back is heard only after its answer, as from a server.
    const tell = made.world.tellContainer.bind(made.world);
    const late: Parameters<typeof tell>[] = [];
    made.world.tellContainer = (...told) => {
      if (told[1].kind === 'pages') late.push(told);
      else tell(...told);
    };
    await made.workspace.undoLast();
    made.world.tellContainer = tell;
    for (const told of late) tell(...told);
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B', 'C']);
    await made.workspace.redoLast();
    await settle();
    release();
    await settle();
    expect(titles(made.store.pages)).toEqual(['C']);
  });
});

describe('a page copied with the pages below it', () => {
  it('goes right after the page in memory, held to the limits', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    const a = await made.page('A');
    await made.page('B', a);
    await made.page('C');
    const copy = await wikis.duplicatePage(made.wikiId, a, 'A (copy)');
    expect(copy.title).toBe('A (copy)');
    expect(titles(await wikis.pages(made.wikiId))).toEqual(['A', '  B', 'A (copy)', '  B', 'C']);
    wikis.limits = { depth: 10, pages: 6 };
    const full = await rejection(wikis.duplicatePage(made.wikiId, a));
    expect([full.reason, full.limit]).toEqual(['too_many_pages', 6]);
    wikis.limits = { depth: 1, pages: 100 };
    const deep = await rejection(wikis.duplicatePage(made.wikiId, a));
    expect(deep.reason).toBe('page_too_deep');
  });

  it('shows the copy in an open wiki, undone to the trash and done again from it', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B', a);
    await made.store.load();
    const copy = await made.store.duplicatePage(a, 'A (copy)');
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B', 'A (copy)', '  B']);
    await made.workspace.undoLast();
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B']);
    expect((await made.backend.wiki.deletedPages(made.wikiId)).map((item) => item.id)).toEqual([
      copy?.id,
    ]);
    await made.workspace.redoLast();
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B', 'A (copy)', '  B']);
    expect(made.workspace.problems).toEqual([]);
  });

  it('copies the icon and the body, and is done again as it is when brought back meanwhile', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    const a = await made.page('A');
    const read = await wikis.page(made.wikiId, a);
    const body: DocNode = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'hello' }] }],
    };
    await wikis.patchPage(made.wikiId, a, { icon: '📘', body, readAt: read.updatedAt });
    await made.store.load();
    const copy = await made.store.duplicatePage(a, 'A (copy)');
    if (!copy) throw new Error('no copy');
    expect(copy.icon).toBe('📘');
    expect((await wikis.page(made.wikiId, copy.id)).body).toEqual(body);
    await made.workspace.undoLast();
    await wikis.restorePage(made.wikiId, copy.id);
    await made.workspace.redoLast();
    await settle();
    expect(made.workspace.problems).toEqual([]);
    expect(titles(made.store.pages)).toEqual(['A', 'A (copy)']);
  });

  it('tells a refused copy once and leaves nothing to undo', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.store.load();
    made.backend.wiki.limits = { depth: 10, pages: 1 };
    expect(await made.store.duplicatePage(a, 'A (copy)')).toBeUndefined();
    expect(made.workspace.problems.map((problem) => problem.action)).toEqual(['save']);
    expect(titles(made.store.pages)).toEqual(['A']);
    expect(made.workspace.canUndo).toBe(false);
  });

  it('keeps out the copies below a copy undone before they were heard', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B', a);
    await made.store.load();
    // The copies are heard, and the tree read anew, only once the copy is undone.
    const wikis = made.backend.wiki;
    const read = wikis.pages.bind(wikis);
    let release = (): void => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    wikis.pages = async (wikiId) => {
      const pages = await read(wikiId);
      await held;
      return pages;
    };
    const tell = made.world.tellContainer.bind(made.world);
    const late: Parameters<typeof tell>[] = [];
    made.world.tellContainer = (...told) => {
      if (told[1].kind === 'pages') late.push(told);
      else tell(...told);
    };
    await made.store.duplicatePage(a, 'A (copy)');
    await made.workspace.undoLast();
    made.world.tellContainer = tell;
    for (const told of late) tell(...told);
    release();
    await settle();
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B']);
  });
});

describe('what the tree keeps out of a page taken off', () => {
  it('shows again what is made below a page whose deletion was refused', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.store.load();
    made.backend.wiki.failNext('deletePage', new BackendError(500, { detail: 'down' }));
    expect(await made.store.deletePage(a)).toBe(false);
    await made.store.createPage({ title: 'B', parentId: a });
    await settle();
    expect(titles(made.store.pages)).toEqual(['A', '  B']);
  });

  it('takes off a page moved out from below a page somebody else deleted first', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    const p = await made.page('P', a);
    await made.page('C');
    await made.store.load();
    const deleting = made.backend.wiki.deletePage(made.wikiId, a);
    const moving = made.store.movePage(p, { parentId: '' });
    await deleting;
    await moving;
    await settle();
    expect(titles(made.store.pages)).toEqual(['C']);
  });
});
