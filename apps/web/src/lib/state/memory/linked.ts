/**
 * What bodies name, for the backend in memory, as the server answers it
 * (`resolveReferences`, `getReferencesTo`, `searchReferences`,
 * `getCardByNumber`): a card is told
 * to whoever has a role on its board, a page to whoever reads its wiki (a role
 * on it, or work in the workspace of an open one), and neither to anybody else,
 * whatever is asked. A view of a board, which a board view shows, is resolved
 * as a card is told, and is nothing else here.
 */
import type { DocNode } from '../../editor/schema.ts';
import type { ViewType } from '../../logic/wire.ts';
import { REFERENCES_AT_ONCE, type LinkedBlock, type References } from '../references.ts';
import type { Paging } from '../trash-reads.ts';

import { reads } from './container-access.ts';
import { pageOf } from './paging.ts';
import { fail } from './refuse.ts';
import { checkWorkspace } from './spaces.ts';
import type { StoredBlock, World } from './world.ts';

/** The most cards a search finds (the server's). */
const FOUND_AT_MOST = 20;

/** The identifiers the references of a body hold, in the order they stand. */
export function namedIn(node: DocNode | null | undefined): string[] {
  if (!node) return [];
  const own =
    node.type === 'reference' && typeof node.attrs?.id === 'string' ? [node.attrs.id] : [];
  return [...own, ...(node.content ?? []).flatMap(namedIn)];
}

/** The body with what its references name renamed, where `rename` has another name. */
export function renameNamed(node: DocNode, rename: (id: string) => string): DocNode {
  const attrs =
    node.type === 'reference' && typeof node.attrs?.id === 'string'
      ? { attrs: { ...node.attrs, id: rename(node.attrs.id) } }
      : {};
  const content = node.content
    ? { content: node.content.map((each) => renameNamed(each, rename)) }
    : {};
  return { ...node, ...attrs, ...content };
}

/** Who asks, and of which world. */
interface Asker {
  readonly world: World;
  readonly userId: string;
}

export class MemoryReferences implements References {
  readonly #asker: Asker;
  readonly #enter: () => Promise<void>;

  constructor(asker: Asker, enter: () => Promise<void>) {
    this.#asker = asker;
    this.#enter = enter;
  }

  /**
   * The view of a board as a board view shows it, if it is of the workspace
   * and the reader reads its board, as a card is told.
   */
  #view(block: StoredBlock | undefined, workspaceId: string): LinkedBlock | undefined {
    const { world, userId } = this.#asker;
    if (block?.type !== 'view') return undefined;
    const board = world.containers.get(block.containerId);
    // A container of no kind is a board, as it was before there were others.
    if (!board || (board.kind ?? 'board') !== 'board' || board.workspaceId !== workspaceId) {
      return undefined;
    }
    if (!reads(world, userId, block.containerId)) return undefined;
    const viewType = block.fields.viewType;
    return {
      id: block.id,
      kind: 'view',
      title: block.title,
      icon: '',
      containerId: board.id,
      containerKind: 'board',
      containerTitle: board.title,
      viewType: typeof viewType === 'string' ? (viewType as ViewType) : 'board',
    };
  }

  /**
   * The card or the page as a reference shows it, if it is of the workspace
   * and the reader is on its board or its wiki: nothing reaches across spaces.
   */
  #shown(block: StoredBlock | undefined, workspaceId: string): LinkedBlock | undefined {
    const { world, userId } = this.#asker;
    if (block?.type !== 'card' && block?.type !== 'page' && block?.type !== 'unit') {
      return undefined;
    }
    if (world.containers.get(block.containerId)?.workspaceId !== workspaceId) return undefined;
    if (!reads(world, userId, block.containerId)) return undefined;
    const icon = typeof block.fields.icon === 'string' ? block.fields.icon : '';
    const { id, title, containerId } = block;
    if (block.type === 'page')
      return { id, kind: 'page', title, icon, containerId, containerKind: 'wiki' };
    if (block.type === 'unit')
      return { id, kind: 'unit', title, icon: '', containerId, containerKind: 'course' };
    const numbered =
      block.fields.isTemplate === true || block.number === undefined
        ? {}
        : { number: block.number };
    return { id, kind: 'card', title, icon, containerId, containerKind: 'board', ...numbered };
  }

  #check(workspaceId: string): void {
    checkWorkspace(this.#asker.world, this.#asker.userId, workspaceId);
  }

  async resolve(workspaceId: string, ids: readonly string[]): Promise<LinkedBlock[]> {
    await this.#enter();
    this.#check(workspaceId);
    if (ids.length > REFERENCES_AT_ONCE) {
      fail(400, `at most ${String(REFERENCES_AT_ONCE)} references are resolved at once`);
    }
    const blocks = this.#asker.world.blocks;
    return [...new Set(ids)].flatMap((id) => {
      const block = blocks.get(id);
      return this.#shown(block, workspaceId) ?? this.#view(block, workspaceId) ?? [];
    });
  }

  async to(workspaceId: string, blockId: string, paging?: Paging): Promise<LinkedBlock[]> {
    await this.#enter();
    this.#check(workspaceId);
    const blocks = this.#asker.world.blocks;
    if (!this.#shown(blocks.get(blockId), workspaceId)) fail(404, 'no such block');
    const naming = new Map<string, StoredBlock>();
    for (const block of blocks.values()) {
      if (!namedIn(block.body).includes(blockId)) continue;
      // What a comment names, the card or the page it is on names.
      const source = block.type === 'comment' ? blocks.get(block.parentId) : block;
      if (source && source.id !== blockId) naming.set(source.id, source);
    }
    const shown = [...naming.values()]
      .sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id))
      .flatMap((block) => this.#shown(block, workspaceId) ?? []);
    return pageOf(shown, paging);
  }

  async search(workspaceId: string, term: string): Promise<LinkedBlock[]> {
    await this.#enter();
    this.#check(workspaceId);
    const words = term
      .toLowerCase()
      .split(/\s+/)
      .filter((word) => word !== '');
    const { world } = this.#asker;
    return [...world.blocks.values()]
      .filter((block) => block.fields.isTemplate !== true)
      .filter((block) => world.containers.get(block.containerId)?.isTemplate !== true)
      .filter((block) => words.every((word) => block.title.toLowerCase().includes(word)))
      .sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id))
      .flatMap((block) => this.#shown(block, workspaceId) ?? [])
      .slice(0, FOUND_AT_MOST);
  }

  /** A card in the trash, or a template, has no number here: it is none. */
  async numbered(
    workspaceId: string,
    boardId: string,
    number: number,
  ): Promise<LinkedBlock | undefined> {
    await this.#enter();
    this.#check(workspaceId);
    const card = this.#asker.world
      .blocksOf(boardId)
      .find(
        (block) =>
          block.type === 'card' && block.fields.isTemplate !== true && block.number === number,
      );
    return this.#shown(card, workspaceId);
  }
}
