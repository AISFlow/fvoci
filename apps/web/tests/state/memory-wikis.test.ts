/**
 * The wikis of the backend in memory keep the server's rules
 * (`crates/store/src/pages.rs`): a wiki is a board of its own kind that the
 * operations of cards do not reach, its pages are a tree whose places are made
 * by the backend, a page goes below a page of the same wiki only, deleting
 * one takes the pages below along, a stale body is refused, and only editors
 * write.
 */
import { describe, expect, it } from 'vitest';

import { comparePlaces, pageTree } from '$lib/logic/page-tree.ts';
import { BackendError, type Change, type WikiPage } from '$lib/state/index.ts';
import { orderBetween } from '$lib/logic/page-order.ts';

import { settle, setup, text } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

async function aWiki(made = setup(), visibility: 'open' | 'private' = 'private') {
  const workspaceId = made.world.workspace.id;
  const wiki = await made.backend.wiki.createWiki(workspaceId, { title: 'Handbook', visibility });
  return { ...made, workspaceId, wiki };
}

/** The titles of a wiki's tree, as nested lists. */
async function shape(made: Awaited<ReturnType<typeof aWiki>>): Promise<unknown[]> {
  const pages = await made.backend.wiki.pages(made.wiki.id);
  const walk = (nodes: ReturnType<typeof pageTree<WikiPage>>): unknown[] =>
    nodes.map((node) =>
      node.children.length > 0 ? [node.page.title, walk(node.children)] : node.page.title,
    );
  return walk(pageTree(pages));
}

describe('a wiki in memory', () => {
  it('is a board of the kind wiki, filed in the maker’s default category, its admin the maker', async () => {
    const made = await aWiki();
    expect(made.wiki.kind).toBe('wiki');
    expect(made.wiki.cardProperties).toEqual([]);
    const categories = await made.backend.categories(made.workspaceId);
    const system = categories.find((category) => category.type === 'system');
    expect(system?.containerIds).toContain(made.wiki.id);
    const [member] = await made.backend.members(made.wiki.id);
    expect(member?.role).toBe('admin');
    expect((await made.backend.container(made.wiki.id)).kind).toBe('wiki');
  });

  it('is among the wikis and not among the boards, the search or the templates', async () => {
    const made = await aWiki();
    const wikis = await made.backend.wiki.wikis(made.workspaceId);
    expect(wikis.map((wiki) => wiki.id)).toEqual([made.wiki.id]);
    const boards = await made.backend.boards(made.workspaceId);
    expect(boards.some((board) => board.id === made.wiki.id)).toBe(false);
    expect(await made.backend.searchBoards(made.workspaceId, 'Handbook')).toEqual([]);
    const templates = await made.backend.templates(made.workspaceId);
    expect(templates.some((board) => board.id === made.wiki.id)).toBe(false);
  });

  it('is copied whole as the server copies it: its tree, under new identifiers; no template', async () => {
    const made = await aWiki();
    const top = await made.backend.wiki.createPage(made.wiki.id, { title: 'Welcome' });
    await made.backend.wiki.createPage(made.wiki.id, { title: 'Below', parentId: top.id });
    await made.backend.wiki.createPage(made.wiki.id, { title: 'After' });
    const copy = await made.backend.duplicateContainer(made.wiki.id, false, made.workspaceId);
    expect(copy).toMatchObject({ kind: 'wiki', title: 'Handbook copy', visibility: 'private' });
    expect(await shape({ ...made, wiki: copy })).toEqual(await shape(made));
    const pages = await made.backend.wiki.pages(copy.id);
    expect(pages.some((page) => page.id === top.id)).toBe(false);
    const refused = rejection(
      made.backend.duplicateContainer(made.wiki.id, true, made.workspaceId),
    );
    expect((await refused).status).toBe(400);
  });

  it('is saved in an archive, with the workspace too, and made again as a wiki', async () => {
    const made = await aWiki();
    const top = await made.backend.wiki.createPage(made.wiki.id, { title: 'Welcome' });
    await made.backend.wiki.createPage(made.wiki.id, { title: 'Below', parentId: top.id });
    const saved = await made.backend.exportContainer(made.wiki.id);
    const all = await made.backend.exportWorkspace(made.workspaceId);
    expect(await all.bytes.text()).toContain(made.wiki.id);
    const file = new File([saved.bytes], saved.name);
    await made.backend.importArchive(made.workspaceId, file);
    const wikis = await made.backend.wiki.wikis(made.workspaceId);
    const again = wikis.find((wiki) => wiki.id !== made.wiki.id);
    expect(again).toMatchObject({ kind: 'wiki', title: 'Handbook' });
    expect(await shape({ ...made, wiki: again ?? made.wiki })).toEqual(await shape(made));
  });

  it('gives a copy files of its own for the pictures of its pages', async () => {
    const made = await aWiki();
    const top = await made.backend.wiki.createPage(made.wiki.id, { title: 'Welcome' });
    const file = {
      fileId: 'f1',
      containerId: made.wiki.id,
      name: 'a.png',
      size: 1,
      contentType: 'image/png',
    };
    made.world.files.set('f1', file);
    const stored = made.world.blocks.get(top.id);
    if (!stored) throw new Error('no page');
    stored.body = { type: 'doc', content: [{ type: 'image', attrs: { fileId: 'f1' } }] };
    const copy = await made.backend.duplicateContainer(made.wiki.id, false, made.workspaceId);
    const [copied] = await made.backend.wiki.pages(copy.id);
    const body = made.world.blocks.get(copied?.id ?? '')?.body;
    const shown = body?.content?.[0]?.attrs?.fileId;
    expect(shown).not.toBe('f1');
    expect(made.world.files.get(String(shown))?.containerId).toBe(copy.id);
    expect(made.world.files.get('f1')?.containerId).toBe(made.wiki.id);
  });

  it('is not reached by the operations of cards, nor a board by those of pages', async () => {
    const made = await aWiki();
    expect((await rejection(made.backend.views(made.wiki.id))).status).toBe(404);
    expect((await rejection(made.backend.cards(made.wiki.id))).status).toBe(404);
    const [board] = await made.backend.boards(made.workspaceId);
    if (!board) throw new Error('no board');
    expect((await rejection(made.backend.wiki.pages(board.id))).status).toBe(404);
    expect((await rejection(made.backend.wiki.createPage(board.id, {}))).status).toBe(404);
  });

  it('is listed and read by another person when it is open, written once they join', async () => {
    const made = await aWiki(setup(), 'open');
    const listed = await made.other.wiki.wikis(made.workspaceId);
    expect(listed.map((wiki) => wiki.id)).toEqual([made.wiki.id]);
    expect(await made.other.wiki.pages(made.wiki.id)).toEqual([]);
    expect((await rejection(made.other.wiki.createPage(made.wiki.id, {}))).status).toBe(403);
    await made.other.joinContainer(made.wiki.id);
    await made.other.wiki.createPage(made.wiki.id, {});
    expect(await made.other.wiki.pages(made.wiki.id)).toHaveLength(1);
  });

  it('is not there for another person when it is private', async () => {
    const made = await aWiki();
    expect(await made.other.wiki.wikis(made.workspaceId)).toEqual([]);
    expect((await rejection(made.other.wiki.pages(made.wiki.id))).status).toBe(404);
  });
});

describe('the pages of a wiki in memory', () => {
  it('go last, first, after a page, and below one, as asked', async () => {
    const made = await aWiki();
    const { wiki } = made.backend;
    const a = await wiki.createPage(made.wiki.id, { title: 'A' });
    await wiki.createPage(made.wiki.id, { title: 'C' });
    await wiki.createPage(made.wiki.id, { title: 'B', after: a.id });
    await wiki.createPage(made.wiki.id, { title: 'First', after: '' });
    const a1 = await wiki.createPage(made.wiki.id, { title: 'A1', parentId: a.id });
    await wiki.createPage(made.wiki.id, { title: 'A0', parentId: a.id, after: '' });
    await wiki.createPage(made.wiki.id, { title: 'A1a', parentId: a1.id });
    expect(await shape(made)).toEqual(['First', ['A', ['A0', ['A1', ['A1a']]]], 'B', 'C']);
  });

  it('take a parent of the same wiki only, and `after` of the same parent only', async () => {
    const made = await aWiki();
    const other = await made.backend.wiki.createWiki(made.workspaceId, {
      title: 'Other',
      visibility: 'private',
    });
    const elsewhere = await made.backend.wiki.createPage(other.id, { title: 'There' });
    const top = await made.backend.wiki.createPage(made.wiki.id, { title: 'Top' });
    const child = await made.backend.wiki.createPage(made.wiki.id, {
      title: 'Child',
      parentId: top.id,
    });
    const parent = await rejection(
      made.backend.wiki.createPage(made.wiki.id, { parentId: elsewhere.id }),
    );
    expect(parent.status).toBe(400);
    expect(parent.errors[0]?.pointer).toBe('/parentId');
    const after = await rejection(made.backend.wiki.createPage(made.wiki.id, { after: child.id }));
    expect(after.status).toBe(400);
    expect(after.errors[0]?.pointer).toBe('/after');
    // The wiki itself is no page, and no parent.
    const wiki = await rejection(
      made.backend.wiki.createPage(made.wiki.id, { parentId: made.wiki.id }),
    );
    expect(wiki.status).toBe(400);
    expect(wiki.errors[0]?.pointer).toBe('/parentId');
  });

  it('are given the places the server gives, by the server’s own examples', () => {
    // As in `crates/model/src/page.rs`'s tests.
    expect(orderBetween(undefined, undefined)).toBe('V');
    expect(orderBetween(undefined, '1')).toBe('0z');
    expect(orderBetween(undefined, '01')).toBe('00z');
    expect(orderBetween('V', 'W')).toBe('VV');
    expect(orderBetween('z', undefined)).toBe('z1');
    for (const [before, after] of [
      ['1', '2'],
      ['V', 'V1'],
      ['Vz', 'W'],
      ['01', '1'],
      ['z', 'z1'],
    ] as const) {
      const made = orderBetween(before, after);
      expect(before < made && made < after).toBe(true);
    }
  });

  it('are given places that sort by code unit, as the server’s', async () => {
    let order = orderBetween(undefined, undefined);
    const orders = [order];
    for (let at = 0; at < 80; at += 1) {
      order = orderBetween(order, undefined);
      orders.push(order);
    }
    let low = orders[0] ?? '';
    for (let at = 0; at < 40; at += 1) {
      const between = orderBetween(low, orders[1]);
      expect(between > low && between < (orders[1] ?? '')).toBe(true);
      low = between;
    }
    const sorted = [...orders].sort();
    expect(sorted).toEqual(orders);
    expect(orderBetween(undefined, 'V') < 'V').toBe(true);
    expect(() => orderBetween('V', 'V')).toThrow();
    expect(
      comparePlaces({ id: 'b', parentId: '', order: 'a' }, { id: 'a', parentId: '', order: 'Z' }),
    ).toBe(1);
  });

  it('go to the trash with every page below, and only the page is told', async () => {
    const made = await aWiki();
    const { wiki } = made.backend;
    const top = await wiki.createPage(made.wiki.id, { title: 'Top' });
    const child = await wiki.createPage(made.wiki.id, { title: 'Child', parentId: top.id });
    await wiki.createPage(made.wiki.id, { title: 'Grandchild', parentId: child.id });
    await wiki.createPage(made.wiki.id, { title: 'Other' });
    const told: Change[] = [];
    made.backend.watchContainer(made.wiki.id, (change) => told.push(change));
    await wiki.deletePage(made.wiki.id, top.id);
    await settle();
    expect(await shape(made)).toEqual(['Other']);
    expect(told).toEqual([
      { kind: 'blocksDeleted', containerId: made.wiki.id, blockIds: [top.id] },
    ]);
    expect((await rejection(wiki.page(made.wiki.id, child.id))).status).toBe(404);
  });

  it('refuse a body written over one read before a change, and nothing changes', async () => {
    const made = await aWiki();
    const { wiki } = made.backend;
    const page = await wiki.createPage(made.wiki.id, { title: 'Page' });
    const written = await wiki.patchPage(made.wiki.id, page.id, {
      body: text('first'),
      readAt: page.updatedAt,
    });
    expect(written.body).toEqual(text('first'));
    const stale = await rejection(
      wiki.patchPage(made.wiki.id, page.id, { body: text('second'), readAt: page.updatedAt }),
    );
    expect(stale.status).toBe(409);
    expect((await wiki.page(made.wiki.id, page.id)).body).toEqual(text('first'));
    const without = await rejection(wiki.patchPage(made.wiki.id, page.id, { body: text('x') }));
    expect(without.status).toBe(400);
  });

  it('change their title and icon, told to whoever listens to the wiki', async () => {
    const made = await aWiki();
    const page = await made.backend.wiki.createPage(made.wiki.id, { title: 'Page' });
    const told: Change[] = [];
    made.backend.watchContainer(made.wiki.id, (change) => told.push(change));
    await made.backend.wiki.patchPage(made.wiki.id, page.id, { title: 'Renamed', icon: '📘' });
    await settle();
    const [change] = told;
    expect(change?.kind).toBe('pages');
    if (change?.kind !== 'pages') return;
    expect(change.pages.map(({ title, icon }) => ({ title, icon }))).toEqual([
      { title: 'Renamed', icon: '📘' },
    ]);
    const icon = await rejection(
      made.backend.wiki.patchPage(made.wiki.id, page.id, { icon: 'ab' }),
    );
    expect(icon.status).toBe(400);
  });

  it('are written by the wiki’s editors only', async () => {
    const made = await aWiki(setup(), 'open');
    const page = await made.backend.wiki.createPage(made.wiki.id, { title: 'Page' });
    await made.backend.patchContainer(made.wiki.id, { minimumRole: 'viewer' });
    await made.other.joinContainer(made.wiki.id);
    expect((await made.other.wiki.page(made.wiki.id, page.id)).title).toBe('Page');
    expect((await rejection(made.other.wiki.createPage(made.wiki.id, {}))).status).toBe(403);
    expect(
      (await rejection(made.other.wiki.patchPage(made.wiki.id, page.id, { title: 'No' }))).status,
    ).toBe(403);
    expect((await rejection(made.other.wiki.deletePage(made.wiki.id, page.id))).status).toBe(403);
  });

  it('come back with their wiki from the trash', async () => {
    const made = await aWiki();
    const page = await made.backend.wiki.createPage(made.wiki.id, { title: 'Kept' });
    await made.backend.deleteContainer(made.wiki.id);
    const trash = await made.backend.deletedContainers(made.workspaceId);
    expect(trash.find((item) => item.id === made.wiki.id)?.kind).toBe('wiki');
    await made.backend.restoreContainer(made.wiki.id);
    expect((await made.backend.wiki.page(made.wiki.id, page.id)).title).toBe('Kept');
  });
});
