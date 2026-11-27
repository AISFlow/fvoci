/**
 * Moving a page of a wiki: the backend in memory keeps the server's rules
 * (`crates/model/src/page_tree.rs`: nowhere below itself, no deeper than the
 * limit, siblings of one order numbered anew), and the store of an open wiki
 * shows a move at once, takes it back when refused, and undoes it to where
 * the page was, or as near as is left.
 */
import { describe, expect, it } from 'vitest';

import { isPlaced, moved, placeOf, stepped } from '$lib/logic/page-moves.ts';
import { pageRows, pageTree } from '$lib/logic/page-tree.ts';
import { BackendError, WikiStore, type WikiPage } from '$lib/state/index.ts';
import { spreadOrders } from '$lib/state/memory/page-tree.ts';

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

describe('the steps of a move', () => {
  const page = (id: string, parentId: string, order: string): WikiPage => ({
    id,
    wikiId: 'w',
    parentId,
    title: id,
    icon: '',
    order,
    createdAt: 0,
    updatedAt: 0,
  });
  // a (b, c), d
  const pages = [page('a', '', 'V'), page('b', 'a', 'V'), page('c', 'a', 'W'), page('d', '', 'W')];

  it('says where a page is and where a step of the keyboard takes it', () => {
    expect(placeOf(pages, 'c')).toEqual({ parentId: 'a', after: 'b' });
    expect(placeOf(pages, 'a')).toEqual({ parentId: '', after: '' });
    expect(stepped(pages, 'c', 'up')).toEqual({ parentId: 'a', after: '' });
    expect(stepped(pages, 'b', 'up')).toBeUndefined();
    expect(stepped(pages, 'b', 'down')).toEqual({ parentId: 'a', after: 'c' });
    expect(stepped(pages, 'c', 'down')).toBeUndefined();
    expect(stepped(pages, 'c', 'in')).toEqual({ parentId: 'b' });
    expect(stepped(pages, 'b', 'in')).toBeUndefined();
    expect(stepped(pages, 'b', 'out')).toEqual({ parentId: '', after: 'a' });
    expect(stepped(pages, 'a', 'out')).toBeUndefined();
  });

  it('shows the page where it goes before the answer comes', () => {
    const after = moved(pages, 'd', { parentId: 'a', after: '' });
    expect(titles(after)).toEqual(['a', '  d', '  b', '  c']);
    expect(isPlaced(after, 'd', { parentId: 'a', after: '' })).toBe(true);
    expect(isPlaced(after, 'd', { parentId: 'a' })).toBe(false);
    // No order between two of one order: last, until the server says.
    const same = [page('p', '', 'V'), page('q', '', 'V'), page('r', '', 'W')];
    expect(titles(moved(same, 'r', { parentId: '', after: 'p' }))).toEqual(['p', 'q', 'r']);
  });
});

describe('moving a page in memory', () => {
  it('moves it with the pages below it, and refuses below itself or another wiki’s page', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    const b = await made.page('B', a);
    const d = await made.page('D');
    const wikis = made.backend.wiki;
    await wikis.movePage(made.wikiId, a, { parentId: d });
    expect(titles(await wikis.pages(made.wikiId))).toEqual(['D', '  A', '    B']);
    for (const parentId of [a, b]) {
      const refused = await rejection(wikis.movePage(made.wikiId, a, { parentId }));
      expect([refused.status, refused.refusalAt('/parentId') !== undefined]).toEqual([400, true]);
    }
    const other = await wikis.createWiki(made.world.workspace.id, {
      title: 'Other',
      visibility: 'private',
    });
    const elsewhere = await wikis.createPage(other.id, { title: 'Elsewhere' });
    const refused = await rejection(wikis.movePage(made.wikiId, a, { parentId: elsewhere.id }));
    expect(refused.refusalAt('/parentId')).toBeDefined();
    const notThere = await rejection(wikis.movePage(other.id, a, {}));
    expect(notThere.status).toBe(404);
    const after = await rejection(wikis.movePage(made.wikiId, a, { after: b }));
    expect(after.refusalAt('/after')).toBeDefined();
  });

  it('holds the tree to its depth and its count, as the server does', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    wikis.limits = { depth: 2, pages: 3 };
    const a = await made.page('A');
    const b = await made.page('B', a);
    const deep = await rejection(wikis.createPage(made.wikiId, { parentId: b }));
    expect([deep.reason, deep.limit, deep.refusalAt('/parentId') !== undefined]).toEqual([
      'page_too_deep',
      2,
      true,
    ]);
    const c = await made.page('C');
    const tooDeep = await rejection(wikis.movePage(made.wikiId, a, { parentId: c }));
    expect(tooDeep.reason).toBe('page_too_deep');
    const full = await rejection(wikis.createPage(made.wikiId, {}));
    expect([full.reason, full.limit]).toEqual(['too_many_pages', 3]);
  });

  it('numbers anew siblings of one order and tells of each', async () => {
    const made = await openWiki();
    const ids = [await made.page('P'), await made.page('Q'), await made.page('R')];
    const mover = await made.page('M', ids[0]);
    for (const id of ids) {
      const block = made.world.blocks.get(id);
      if (block) block.fields = { ...block.fields, order: 'V' };
    }
    const told: string[] = [];
    const stop = made.backend.watchContainer(made.wikiId, (change) => {
      if (change.kind === 'pages') told.push(...change.pages.map((page) => page.id));
    });
    const listed = (await made.backend.wiki.pages(made.wikiId))
      .filter((page) => page.parentId === '')
      .sort((one, other) => (one.id < other.id ? -1 : 1))
      .map((page) => page.id);
    await made.backend.wiki.movePage(made.wikiId, mover, { after: listed[0] });
    await settle();
    stop();
    const top = (await made.backend.wiki.pages(made.wikiId)).filter((page) => page.parentId === '');
    const orders = top.map((page) => page.order).sort();
    expect(new Set(orders).size).toBe(4);
    expect(told).toEqual(expect.arrayContaining([...ids, mover]));
    expect(spreadOrders(5000).every((order) => order.length <= 3)).toBe(true);
  });
});

describe('the rules of a move in memory, as the server’s', () => {
  it('lets a page deeper than a lowered limit move as long as no deeper, and writes nothing for no move', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    const a = await made.page('A');
    const b = await made.page('B', a);
    const c = await made.page('C', b);
    const sibling = await made.page('D', b);
    const top = await made.page('E');
    // C and D at depth 3, below a limit of 2 lowered since: they move, no deeper.
    wikis.limits = { depth: 2, pages: 100 };
    await wikis.movePage(made.wikiId, c, { parentId: b, after: sibling });
    await wikis.movePage(made.wikiId, c, { parentId: a });
    await wikis.movePage(made.wikiId, sibling, { parentId: c });
    const refused = await rejection(wikis.movePage(made.wikiId, top, { parentId: c }));
    expect(refused.reason).toBe('page_too_deep');
    const told: string[] = [];
    const stop = made.backend.watchContainer(made.wikiId, (change) => {
      if (change.kind === 'pages') told.push(...change.pages.map((page) => page.id));
    });
    const before = (await wikis.page(made.wikiId, c)).updatedAt;
    await wikis.movePage(made.wikiId, c, { parentId: a });
    await settle();
    stop();
    expect([(await wikis.page(made.wikiId, c)).updatedAt, told]).toEqual([before, []]);
  });
});

describe('a page far past a lowered limit', () => {
  it('moves among its siblings, as the server lets it', async () => {
    const made = await openWiki();
    const wikis = made.backend.wiki;
    const a = await made.page('A');
    const b = await made.page('B', a);
    const c = await made.page('C', b);
    const d = await made.page('D', c);
    const e = await made.page('E', c);
    wikis.limits = { depth: 1, pages: 100 };
    await wikis.movePage(made.wikiId, d, { parentId: c, after: e });
    expect(titles(await wikis.pages(made.wikiId))).toEqual([
      'A',
      '  B',
      '    C',
      '      E',
      '      D',
    ]);
  });
});

describe('moving a page in an open wiki', () => {
  it('shows the move at once, and undoes it back to where the page was', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B');
    const c = await made.page('C');
    await made.store.load();
    const moving = made.store.movePage(c, { parentId: a });
    expect(titles(made.store.pages)).toEqual(['A', '  C', 'B']);
    expect(await moving).toBe(true);
    await made.workspace.undoLast();
    expect(titles(made.store.pages)).toEqual(['A', 'B', 'C']);
    expect(titles(await made.backend.wiki.pages(made.wikiId))).toEqual(['A', 'B', 'C']);
    await made.workspace.redoLast();
    expect(titles(await made.backend.wiki.pages(made.wikiId))).toEqual(['A', '  C', 'B']);
  });

  it('counts the moves not answered yet, refused or taken, one after another', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    const b = await made.page('B');
    const c = await made.page('C');
    await made.store.load();
    expect(made.store.moving).toBe(0);
    made.backend.wiki.failNext('movePage', new BackendError(500, {}));
    const first = made.store.movePage(c, { parentId: a });
    const second = made.store.movePage(b, { parentId: a });
    // Shown before the backend has either: the tree says it is settling until both are answered.
    expect(made.store.moving).toBe(2);
    expect(await first).toBe(false);
    expect(made.store.moving).toBe(1);
    expect(await second).toBe(true);
    expect(made.store.moving).toBe(0);
  });

  it('tells of a first move over a page deleted meanwhile, and takes it off the tree with those below it', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('A1', a);
    const b = await made.page('B');
    await made.store.load();
    // Deleted by somebody else, and not heard of yet: the backend answers the move 404.
    made.backend.wiki.failNext('movePage', new BackendError(404, {}));
    expect(await made.store.movePage(a, { parentId: '', after: b })).toBe(false);
    expect(titles(made.store.pages)).toEqual(['B']);
    expect(made.workspace.problems.at(-1)).toMatchObject({
      action: 'movePage',
      error: { status: 404 },
    });
    // A move that did not happen is nothing to undo.
    expect(made.workspace.undo.canUndo).toBe(false);
  });

  it('says nothing of a redo over a page deleted since, and counts an undo and a redo as settling', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    await made.page('B');
    const c = await made.page('C');
    await made.store.load();
    await made.store.movePage(c, { parentId: a });
    made.backend.latency = 5;
    const undoing = made.workspace.undoLast();
    // Under way from the asking, though it is no move of `movePage`'s.
    expect(made.store.settling).toBe(true);
    await undoing;
    expect(made.store.settling).toBe(false);
    made.backend.wiki.failNext('movePage', new BackendError(404, {}));
    const redoing = made.workspace.redoLast();
    expect(made.store.settling).toBe(true);
    await redoing;
    expect(made.store.settling).toBe(false);
    expect(made.workspace.problems).toEqual([]);
  });

  it('takes a refused move back, and undoes to as near as is left', async () => {
    const made = await openWiki();
    const a = await made.page('A');
    const b = await made.page('B');
    const c = await made.page('C');
    await made.store.load();
    made.backend.wiki.failNext('movePage', new BackendError(500, {}));
    expect(await made.store.movePage(c, { parentId: a })).toBe(false);
    expect(titles(made.store.pages)).toEqual(['A', 'B', 'C']);

    // Two moves of one page asked for at once: the first refused, the second taken.
    made.backend.wiki.failNext('movePage', new BackendError(500, {}));
    const first = made.store.movePage(c, { parentId: a });
    const second = made.store.movePage(c, { parentId: b });
    // The second waits for the first, which shows at once.
    expect(titles(made.store.pages)).toEqual(['A', '  C', 'B']);
    await Promise.all([first, second]);
    expect(titles(made.store.pages)).toEqual(['A', 'B', '  C']);
    await made.workspace.undoLast();
    expect(titles(made.store.pages)).toEqual(['A', 'B', 'C']);
    // Both refused: back where it was before both, whatever was guessed between.
    made.backend.wiki.failNext('movePage', new BackendError(500, {}));
    const refusedFirst = made.store.movePage(c, { parentId: a });
    const refusedSecond = made.store.movePage(c, { parentId: c });
    await Promise.all([refusedFirst, refusedSecond]);
    expect(titles(made.store.pages)).toEqual(['A', 'B', 'C']);

    // C was after B; B is moved away meanwhile: back last at the top.
    await made.store.movePage(c, { parentId: a });
    await made.backend.wiki.movePage(made.wikiId, b, { parentId: a });
    await settle();
    await made.workspace.undoLast();
    expect(titles(await made.backend.wiki.pages(made.wikiId))).toEqual(['A', '  B', 'C']);
  });
});
