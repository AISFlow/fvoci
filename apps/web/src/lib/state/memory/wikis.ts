/**
 * The wikis of the backend in memory (`Backend.wiki`), as the server keeps
 * them: a wiki is a board of the kind `wiki` (so the board's operations read,
 * change, delete and bring it back), its pages are blocks of the type `page`
 * with their icon and place among their `fields`.
 *
 * The rules are the server's (`crates/store/src/pages.rs`): a page goes below
 * a page of the same wiki or at its top, among its siblings where `after`
 * says (`orderBetween`); its pages are for the wiki's members, written by its
 * editors; a body is written with the `updatedAt` it was read at (409 when the
 * page changed since, what its room took counted first) and kept as a
 * version around the write, as a card's is; deleting a page takes every page
 * below it along, and their comments, and tells of the page alone; bringing
 * it back brings them, below the page it was below or, that one gone, last at
 * the top (`crates/store/src/page_trash.rs`). The comments on a page are
 * `page-comments.ts`'.
 */
import type { DocNode } from '../../editor/schema.ts';
import { PAGE_TITLE_MAX } from '../../logic/page-tree.ts';
import { createBoard, type Container, type ContainerKind } from '../../logic/wire.ts';
import { BackendError, type DeletedItem, type Paging } from '../backend.ts';
import type {
  PageDraft,
  PageMove,
  PagePatch,
  WikiDraft,
  WikiPage,
  WikiPageDetail,
  Wikis,
} from '../wiki-types.ts';

import { fileAsNew } from './categories.ts';
import { memberRole } from './container-access.ts';
import { checkedWhole } from './bodies.ts';
import { MemoryBodyVersions, writtenWhole } from './body-versions.ts';
import { MemoryPageComments, type OnPage } from './page-comments.ts';
import { DEFAULT_LIMITS, PageTree, type PageLimits, type Place } from './page-tree.ts';
import { fail } from './refuse.ts';
import { commentOf, copy, memberOf, orderOf, pageOf, scramble } from './shapes.ts';
import { checkWorkspace } from './spaces.ts';
import { deletedPagesOf, deletion } from './trash.ts';
import type { StoredBlock, StoredCard, World } from './world.ts';

/** What the wikis share with the backend they are of. */
export interface WikiHost {
  readonly world: World;
  readonly userId: string;
  /** Milliseconds every call waits before it answers. */
  readonly latency: number;
}

const isWiki = (board: Container | undefined): board is Container => board?.kind === 'wiki';

function detailOf(block: StoredBlock): WikiPageDetail {
  return {
    ...pageOf(block),
    body: copy(block.body ?? null),
    createdBy: block.createdBy,
    modifiedBy: block.modifiedBy,
  };
}

/** One grapheme at most, as the server takes an icon. */
function checkedIcon(icon: string): string {
  const graphemes = [...new Intl.Segmenter(undefined, { granularity: 'grapheme' }).segment(icon)];
  if (graphemes.length > 1) fail(400, 'an icon is one grapheme', { field: 'icon' });
  return icon;
}

function checkedTitle(title: string): string {
  // In characters (code points), as the server counts them.
  if (Array.from(title).length > PAGE_TITLE_MAX) {
    fail(400, 'the title is too long', { field: 'title', limit: PAGE_TITLE_MAX });
  }
  return title;
}

export class MemoryWikis extends MemoryPageComments implements Wikis {
  readonly #host: WikiHost;
  readonly #failures = new Map<keyof Wikis, BackendError>();
  /** The limits of the tree: the server's defaults, lowered by a test to reach them. */
  limits: PageLimits = DEFAULT_LIMITS;
  /** Made when first asked: the host's world is not there yet while this is made. */
  #bodyVersions: MemoryBodyVersions | undefined;

  constructor(host: WikiHost) {
    super();
    this.#host = host;
  }

  get #world(): World {
    return this.#host.world;
  }

  get bodyVersions(): MemoryBodyVersions {
    this.#bodyVersions ??= new MemoryBodyVersions(
      this.#world,
      this.#host.userId,
      () => this.#enter('bodyVersions'),
      'page',
    );
    return this.#bodyVersions;
  }

  protected enter(method: keyof Wikis): Promise<void> {
    return this.#enter(method);
  }

  protected on(wikiId: string, pageId: string): OnPage {
    return { world: this.#world, userId: this.#host.userId, wikiId, pageId };
  }

  /** Makes the next call of `method` reject with `error`, once. */
  failNext(method: keyof Wikis, error: BackendError): void {
    this.#failures.set(method, error);
  }

  async #enter(method: keyof Wikis): Promise<void> {
    if (this.#host.latency > 0) {
      await new Promise((resolve) => {
        setTimeout(resolve, this.#host.latency);
      });
    } else {
      await Promise.resolve();
    }
    const failure = this.#failures.get(method);
    if (failure) {
      this.#failures.delete(method);
      throw failure;
    }
  }

  #role(wikiId: string, kind: ContainerKind = 'wiki'): string {
    return memberRole(this.#world, this.#host.userId, wikiId, kind);
  }

  /** For the wiki's editors: its admins and editors. */
  #mayWrite(wikiId: string): void {
    const role = this.#role(wikiId);
    if (role !== 'admin' && role !== 'editor') fail(403, 'access denied to the wiki');
  }

  #page(wikiId: string, pageId: string): StoredBlock {
    const block = this.#world.blocks.get(pageId);
    if (block?.type !== 'page' || block.containerId !== wikiId) fail(404, 'no such page');
    return block;
  }

  #pagesOf(wikiId: string): StoredBlock[] {
    return this.#world.blocksOf(wikiId).filter((block) => block.type === 'page');
  }

  #tellPages(wikiId: string, blocks: readonly StoredBlock[]): void {
    this.#world.tellContainer(wikiId, {
      kind: 'pages',
      containerId: wikiId,
      pages: blocks.map(pageOf),
    });
  }

  /** As the server: the reader's wikis and the workspace's open ones, in no order to count on. */
  async wikis(workspaceId: string): Promise<Container[]> {
    await this.#enter('wikis');
    checkWorkspace(this.#world, this.#host.userId, workspaceId);
    const userId = this.#host.userId;
    const theirs = [...this.#world.containers.values()].filter(
      (board) =>
        isWiki(board) &&
        board.workspaceId === workspaceId &&
        (board.visibility === 'open' ||
          (this.#world.members.get(board.id) ?? []).some((member) => member.userId === userId)),
    );
    return scramble(theirs).map(copy);
  }

  async createWiki(workspaceId: string, draft: WikiDraft): Promise<Container> {
    await this.#enter('createWiki');
    checkWorkspace(this.#world, this.#host.userId, workspaceId);
    const world = this.#world;
    const userId = this.#host.userId;
    const now = world.now();
    const wiki: Container = {
      ...createBoard(),
      id: world.newId(),
      workspaceId,
      kind: 'wiki',
      createdBy: userId,
      modifiedBy: userId,
      visibility: draft.visibility,
      minimumRole: draft.minimumRole ?? 'editor',
      title: draft.title,
      description: draft.description ?? '',
      icon: draft.icon ?? '',
      cardProperties: [],
      createdAt: now,
      updatedAt: now,
    };
    world.containers.set(wiki.id, wiki);
    world.members.set(wiki.id, [memberOf(wiki.id, userId, 'admin')]);
    world.tellContainer(wiki.id, { kind: 'container', container: copy(wiki) });
    world.tellUsers([userId], { kind: 'container', container: copy(wiki) });
    // Filed under the maker's default category, as a board is.
    fileAsNew(world, userId, wiki);
    return copy(wiki);
  }

  async pages(wikiId: string): Promise<WikiPage[]> {
    await this.#enter('pages');
    this.#role(wikiId);
    return scramble(this.#pagesOf(wikiId)).map(pageOf);
  }

  async page(wikiId: string, pageId: string): Promise<WikiPageDetail> {
    await this.#enter('page');
    this.#role(wikiId);
    return detailOf(this.#page(wikiId, pageId));
  }

  /** The pages of the wiki as a tree, for its decisions (`page-tree.ts`). */
  #tree(wikiId: string): PageTree {
    return new PageTree(
      this.#pagesOf(wikiId).map((block) => ({
        id: block.id,
        parentId: block.parentId,
        order: orderOf(block),
      })),
    );
  }

  /** Gives `page` the order of `place`, and its siblings theirs: those numbered anew, changed. */
  #writePlace(page: StoredBlock, place: Place): StoredBlock[] {
    const now = this.#world.now();
    const renumbered = place.renumbered.flatMap(([id, order]) => {
      const sibling = this.#world.blocks.get(id);
      if (!sibling) return [];
      sibling.fields = { ...sibling.fields, order };
      // As the server: the order alone, who changed it last stays.
      sibling.updatedAt = now;
      return [sibling];
    });
    page.fields = { ...page.fields, order: place.order };
    return renumbered;
  }

  async createPage(wikiId: string, draft: PageDraft): Promise<WikiPageDetail> {
    await this.#enter('createPage');
    this.#mayWrite(wikiId);
    // The title and the icon first, as the server checks them.
    const title = checkedTitle(draft.title ?? '');
    const icon = checkedIcon(draft.icon ?? '');
    const parentId = draft.parentId ?? '';
    if (parentId !== '') {
      const parent = this.#world.blocks.get(parentId);
      if (parent?.type !== 'page' || parent.containerId !== wikiId) {
        fail(400, 'the parent is no page of the wiki', { field: 'parentId' });
      }
    }
    const tree = this.#tree(wikiId);
    tree.checkRoom(parentId, 1, 1, this.limits);
    const place = tree.place(parentId, draft.after);
    const now = this.#world.now();
    const userId = this.#host.userId;
    const page: StoredBlock = {
      id: this.#world.newId(),
      containerId: wikiId,
      parentId,
      createdBy: userId,
      modifiedBy: userId,
      type: 'page',
      title,
      fields: { icon },
      body: null,
      createdAt: now,
      updatedAt: now,
    };
    const renumbered = this.#writePlace(page, place);
    this.#world.blocks.set(page.id, page);
    this.#tellPages(wikiId, [...renumbered, page]);
    return detailOf(page);
  }

  async movePage(wikiId: string, pageId: string, to: PageMove): Promise<WikiPageDetail> {
    await this.#enter('movePage');
    this.#mayWrite(wikiId);
    const page = this.#page(wikiId, pageId);
    const parentId = to.parentId ?? '';
    const tree = this.#tree(wikiId);
    tree.checkMove(pageId, parentId, this.limits);
    // Where it is already: nothing is written, and nobody is told.
    if (page.parentId === parentId && tree.isAt(pageId, parentId, to.after)) return detailOf(page);
    const place = tree.place(parentId, to.after, pageId);
    const renumbered = this.#writePlace(page, place);
    page.parentId = parentId;
    page.updatedAt = this.#world.now();
    page.modifiedBy = this.#host.userId;
    this.#tellPages(wikiId, [...renumbered, page]);
    return detailOf(page);
  }

  /** As the server: the page and the pages below it copied right after it, held to the limits. */
  async duplicatePage(wikiId: string, pageId: string, title?: string): Promise<WikiPageDetail> {
    await this.#enter('duplicatePage');
    this.#mayWrite(wikiId);
    const page = this.#page(wikiId, pageId);
    const copied = title === undefined ? page.title : checkedTitle(title);
    const tree = this.#tree(wikiId);
    const family = tree.below(pageId);
    tree.checkRoom(page.parentId, tree.height(pageId), family.length, this.limits);
    const place = tree.place(page.parentId, pageId);
    const world = this.#world;
    // What is typed in the bodies now is copied too.
    for (const id of family) world.bodyFlushed?.(id);
    const now = world.now();
    const userId = this.#host.userId;
    const ids = new Map(family.map((id) => [id, world.newId()]));
    const copies = family.flatMap((id) => {
      const source = world.blocks.get(id);
      if (!source) return [];
      const { deletedAt: _deletedAt, ...rest } = copy(source);
      const made: StoredBlock = {
        ...rest,
        id: ids.get(id) ?? world.newId(),
        parentId: ids.get(source.parentId) ?? source.parentId,
        createdBy: userId,
        modifiedBy: userId,
        createdAt: now,
        updatedAt: now,
      };
      return [made];
    });
    const [top] = copies;
    if (!top) fail(404, 'no such page');
    top.title = copied;
    const renumbered = this.#writePlace(top, place);
    for (const one of copies) world.blocks.set(one.id, one);
    this.#tellPages(wikiId, [...renumbered, ...copies]);
    return detailOf(top);
  }

  async patchPage(wikiId: string, pageId: string, patch: PagePatch): Promise<WikiPageDetail> {
    await this.#enter('patchPage');
    this.#mayWrite(wikiId);
    const page = this.#page(wikiId, pageId);
    // The whole request is checked before anything changes, as the server does.
    const title = patch.title === undefined ? undefined : checkedTitle(patch.title);
    const icon = patch.icon === undefined ? undefined : checkedIcon(patch.icon);
    if (patch.body !== undefined && patch.readAt === undefined) {
      fail(400, 'a body is written with the time it was read', { field: 'readAt' });
    }
    // What its room took is written into the page first: it counts against `readAt`.
    const body: DocNode | null | undefined =
      patch.body === undefined ? undefined : checkedWhole(this.#world, wikiId, pageId, patch.body);
    if (patch.readAt !== undefined && patch.readAt !== page.updatedAt) {
      throw new BackendError(409, { detail: 'the page changed since it was read' });
    }
    const write = (): void => {
      if (title !== undefined) page.title = title;
      if (icon !== undefined) page.fields = { ...page.fields, icon };
      if (body !== undefined) page.body = body;
      page.updatedAt = this.#world.now();
      page.modifiedBy = this.#host.userId;
    };
    // A body written whole is kept as a version, after the one it is written over.
    if (body === undefined) write();
    else writtenWhole(this.#world, page as StoredCard, body, this.#host.userId, write);
    if (body !== undefined) this.#world.bodyWritten?.(pageId);
    this.#tellPages(wikiId, [page]);
    return detailOf(page);
  }

  async deletePage(wikiId: string, pageId: string): Promise<void> {
    await this.#enter('deletePage');
    this.#mayWrite(wikiId);
    const page = this.#page(wikiId, pageId);
    const gone = [page];
    // Each page found brings the pages and the comments below it: the list grows as it is walked.
    for (const above of gone) gone.push(...this.#world.childrenOf(above.id));
    this.#world.deletedBlocks.set(pageId, {
      containerId: wikiId,
      blocks: gone,
      ...deletion({ world: this.#world, userId: this.#host.userId }),
    });
    for (const block of gone) this.#world.blocks.delete(block.id);
    this.#world.tellContainer(wikiId, {
      kind: 'blocksDeleted',
      containerId: wikiId,
      blockIds: [pageId],
    });
  }

  async deletedPages(wikiId: string, paging?: Paging): Promise<DeletedItem[]> {
    await this.#enter('deletedPages');
    this.#mayWrite(wikiId);
    return deletedPagesOf(this.#world, wikiId, paging);
  }

  /**
   * As the server: the page at the top of a deletion comes back with all that
   * went with it, held to the limits; at its place, unless a page made since
   * has it (then right after the pages before it), or, its parent gone, last
   * at the top.
   */
  async restorePage(wikiId: string, pageId: string): Promise<WikiPageDetail> {
    await this.#enter('restorePage');
    this.#mayWrite(wikiId);
    const world = this.#world;
    const deleted = world.deletedBlocks.get(pageId);
    const [top] = deleted?.blocks ?? [];
    if (deleted?.containerId !== wikiId || top?.id !== pageId || top.type !== 'page') {
      fail(404, 'no such page in the trash');
    }
    const { blocks } = deleted;
    // The comments that went with the pages come back with them; the limits count pages.
    const pages = blocks.filter((block) => block.type === 'page');
    const tree = this.#tree(wikiId);
    const coming = new PageTree(
      pages.map((block) => ({ id: block.id, parentId: block.parentId, order: orderOf(block) })),
    );
    const parentId = top.parentId !== '' && !tree.has(top.parentId) ? '' : top.parentId;
    tree.checkRoom(parentId, coming.height(pageId), pages.length, this.limits);
    const order = orderOf(top);
    const siblings = tree.siblings(parentId);
    let renumbered: StoredBlock[] = [];
    if (parentId !== top.parentId) {
      // Last at the top of the wiki.
      renumbered = this.#writePlace(top, tree.place('', undefined));
    } else if (siblings.some((sibling) => sibling.order === order)) {
      const before = siblings.filter(
        (sibling) => sibling.order < order || (sibling.order === order && sibling.id < pageId),
      );
      renumbered = this.#writePlace(top, tree.place(parentId, before.at(-1)?.id ?? ''));
    }
    top.parentId = parentId;
    world.deletedBlocks.delete(pageId);
    const now = world.now();
    for (const block of blocks) {
      [block.updatedAt, block.modifiedBy] = [now, this.#host.userId];
      world.blocks.set(block.id, block);
    }
    this.#tellPages(wikiId, [...renumbered, ...pages]);
    // And the comments that came back with them, as the server tells them.
    const comments = blocks.filter((block) => block.type === 'comment');
    if (comments.length > 0) {
      world.tellContainer(wikiId, {
        kind: 'blocks',
        containerId: wikiId,
        blocks: comments.map(commentOf),
      });
    }
    return detailOf(top);
  }
}
