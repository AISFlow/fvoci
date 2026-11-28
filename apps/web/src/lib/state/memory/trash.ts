/**
 * The trash of the world in memory, as the server keeps its own: the cards
 * and boards deleted, with who deleted them and when, listed for whoever may
 * bring them back; and the places a deleted board had in its people's
 * sidebars, to file it there again when it comes back.
 */
import type { Container, ContainerMember } from '../../logic/wire.ts';
import type { DeletedItem, Paging } from '../backend.ts';

import { categoriesIn, tellCategories } from './categories.ts';
import { checkCardRestore } from './card-hierarchy.ts';
import { mayRemoveComments, type Role } from './comments.ts';
import { checkDiaryRoom } from './diary-places.ts';
import { pageOf } from './paging.ts';
import { fail } from './refuse.ts';
import type { StoredBlock, World } from './world.ts';

/** Where a board was filed for one person: the category, the place in it, hidden or not. */
export interface ContainerPlace {
  userId: string;
  categoryId: string;
  index: number;
  hidden: boolean;
}

const latestFirst = (first: DeletedItem, second: DeletedItem): number =>
  second.deletedAt - first.deletedAt;

/**
 * The cards of a board in the trash: those deleted by themselves. A card
 * that went with the block it was on comes back with that block, and is not
 * listed. The last deleted first. Only for who may manage the board's cards.
 */
export function deletedCardsOf(
  world: World,
  userId: string,
  boardId: string,
  paging: Paging | undefined,
): DeletedItem[] {
  mayManageCards(world, userId, boardId, 'access denied to the trash of the board');
  const items: DeletedItem[] = [];
  for (const [id, deleted] of world.deletedBlocks) {
    const [first] = deleted.blocks;
    if (deleted.containerId !== boardId || first?.id !== id || first.type !== 'card') continue;
    items.push({
      id,
      title: first.title,
      deletedAt: deleted.deletedAt,
      deletedBy: deleted.deletedBy,
    });
  }
  return pageOf(items.sort(latestFirst), paging);
}

/**
 * The pages of a wiki in the trash at the top of what went with them, each
 * with how many pages below it went along. A page that went with the page it
 * was below comes back with that page, and is not listed. The last deleted
 * first, then by identifier, as the server lists them.
 */
export function deletedPagesOf(
  world: World,
  wikiId: string,
  paging: Paging | undefined,
): DeletedItem[] {
  const items: DeletedItem[] = [];
  for (const [id, deleted] of world.deletedBlocks) {
    const [first] = deleted.blocks;
    if (deleted.containerId !== wikiId || first?.id !== id || first.type !== 'page') continue;
    items.push({
      id,
      title: first.title,
      deletedAt: deleted.deletedAt,
      deletedBy: deleted.deletedBy,
      // The pages that went with it: its comments and theirs went too, and are no pages.
      below: deleted.blocks.filter((block) => block.type === 'page').length - 1,
    });
  }
  const order = (one: DeletedItem, other: DeletedItem): number =>
    latestFirst(one, other) || (one.id < other.id ? -1 : one.id > other.id ? 1 : 0);
  return pageOf(items.sort(order), paging);
}

/**
 * The chapters and units of a course in the trash at the top of each deletion,
 * newest first then by identifier. A unit deleted with its chapter is not listed
 * separately and counts toward the chapter's `below`.
 */
export function deletedUnitsOf(
  world: World,
  courseId: string,
  paging: Paging | undefined,
): DeletedItem[] {
  const items: DeletedItem[] = [];
  for (const [id, deleted] of world.deletedBlocks) {
    const [first] = deleted.blocks;
    const isNode = first?.type === 'unit' || first?.type === 'chapter';
    if (deleted.containerId !== courseId || first?.id !== id || !isNode) continue;
    items.push({
      id,
      title: first.title,
      deletedAt: deleted.deletedAt,
      deletedBy: deleted.deletedBy,
      below:
        first.type === 'chapter'
          ? deleted.blocks.filter((block) => block.type === 'unit').length
          : 0,
    });
  }
  const order = (one: DeletedItem, other: DeletedItem): number =>
    latestFirst(one, other) || (one.id < other.id ? -1 : one.id > other.id ? 1 : 0);
  return pageOf(items.sort(order), paging);
}

/** Whether a member was an admin of the board, which bringing it back takes. */
export const wasAdmin = (members: readonly ContainerMember[], userId: string): boolean =>
  members.some(
    (member) => !member.synthetic && member.userId === userId && member.role === 'admin',
  );

/** The boards and wikis of the workspace in the trash that the user was an admin of, the last deleted first. */
export function deletedContainersOf(
  world: World,
  userId: string,
  workspaceId: string,
  paging: Paging | undefined,
): DeletedItem[] {
  const items = [...world.deletedContainers.values()]
    .filter(
      ({ container, members }) =>
        container.workspaceId === workspaceId && wasAdmin(members, userId),
    )
    .map(({ container, deletedAt, deletedBy }) => ({
      id: container.id,
      title: container.title,
      deletedAt,
      deletedBy,
      kind: container.kind ?? 'board',
    }));
  return pageOf(items.sort(latestFirst), paging);
}

/** Where everybody had filed the board or wiki, read before it goes. */
export function placesOf(world: World, containerId: string): ContainerPlace[] {
  const places: ContainerPlace[] = [];
  for (const [userId, categories] of world.categories) {
    for (const category of categories) {
      const index = category.containerIds.indexOf(containerId);
      if (index < 0) continue;
      places.push({
        userId,
        categoryId: category.id,
        index,
        hidden: category.hiddenContainerIds.includes(containerId),
      });
    }
  }
  return places;
}

/**
 * Files a board that came back where each person had it: the same category
 * and place, hidden or not; their default category if that one went
 * meanwhile. A private board only for its members. Answers the people whose
 * sidebar changed.
 */
export function putBack(
  world: World,
  board: Container,
  places: readonly ContainerPlace[],
): string[] {
  const memberIds = (world.members.get(board.id) ?? []).map((member) => member.userId);
  const told: string[] = [];
  for (const place of places) {
    if (board.visibility !== 'open' && !memberIds.includes(place.userId)) continue;
    const theirs = categoriesIn(world, place.userId, board.workspaceId);
    const kept = theirs.find((category) => category.id === place.categoryId);
    const category = kept ?? theirs.find((each) => each.type === 'system');
    if (!category || category.containerIds.includes(board.id)) continue;
    if (kept)
      category.containerIds.splice(
        Math.min(place.index, category.containerIds.length),
        0,
        board.id,
      );
    else category.containerIds.push(board.id);
    if (place.hidden) category.hiddenContainerIds.push(board.id);
    told.push(place.userId);
  }
  return told;
}

/** Who deletes and when, as the trash keeps it with what was deleted. */
export function deletion(by: { world: World; userId: string }): {
  deletedAt: number;
  deletedBy: string;
} {
  return { deletedAt: by.world.now(), deletedBy: by.userId };
}

/**
 * As the server: the trash of a board's cards, and bringing one back, are
 * for its editors and admins; a private board the user is no member of is
 * not there for them.
 */
export function mayManageCards(
  world: World,
  userId: string,
  boardId: string,
  refusal: string,
): void {
  const board = world.containers.get(boardId);
  // Only boards have cards: wikis, diaries and courses answer as not found.
  if (!board || (board.kind ?? 'board') !== 'board') fail(404, 'no such board');
  const member = world.members.get(boardId)?.find((each) => each.userId === userId);
  if (!member && board.visibility !== 'open') fail(404, 'no such board');
  if (member?.role !== 'admin' && member?.role !== 'editor') fail(403, refusal);
}

/**
 * Takes a board or a wiki, its blocks and its members out of the trash, as the server
 * does: for who was its admin (403 otherwise), and no such board for who
 * was no member (404). Each person who had filed it finds it where it was,
 * and is told. Answers the board.
 */
export function takeContainerBack(world: World, userId: string, containerId: string): Container {
  const deleted = world.deletedContainers.get(containerId);
  if (!deleted?.members.some((member) => member.userId === userId)) fail(404, 'no such board');
  if (!wasAdmin(deleted.members, userId)) fail(403, 'access denied to undelete board');
  // As the server's (`insert_container_row`): a diary comes back only where there is room for it.
  if (deleted.container.kind === 'diary') {
    checkDiaryRoom(world, userId, deleted.container.workspaceId, 1);
  }
  world.deletedContainers.delete(containerId);
  world.containers.set(containerId, deleted.container);
  world.members.set(containerId, deleted.members);
  for (const block of deleted.blocks) world.blocks.set(block.id, block);
  for (const person of putBack(world, deleted.container, deleted.places)) {
    tellCategories(world, person, deleted.container.workspaceId);
  }
  return deleted.container;
}

/** Collects `root` and every block below it in `world`, parents before children. */
export function subtreeOf(world: World, root: StoredBlock): StoredBlock[] {
  const gone: StoredBlock[] = [];
  const collect = (parent: StoredBlock): void => {
    gone.push(parent);
    for (const child of world.childrenOf(parent.id)) collect(child);
  };
  collect(root);
  return gone;
}

/**
 * Takes a block and what went with it out of the trash, as the server does:
 * a comment at the top by the comment's rule, anything else by an editor's,
 * only under a living parent. Marks each block as modified by `userId` now.
 */
export function takeBlockBack(
  world: World,
  userId: string,
  boardId: string,
  blockId: string,
  memberRole: () => Role,
  ensureBoard: () => void,
): StoredBlock[] {
  const deleted = world.deletedBlocks.get(blockId);
  if (deleted?.containerId !== boardId) fail(404, 'no such block in the trash');
  const top = deleted.blocks[0];
  if (top?.type !== 'comment') {
    mayManageCards(world, userId, boardId, 'access denied to undelete block');
  } else if (!mayRemoveComments(memberRole(), userId, deleted)) {
    fail(403, "access denied to undelete other users' comments");
  }
  let reroot = false;
  if (top?.type === 'card') {
    reroot = checkCardRestore(world, boardId, deleted.blocks);
  } else if (top?.parentId && !world.blocks.has(top.parentId)) {
    fail(404, 'the parent of the block is gone');
  }
  ensureBoard();
  world.deletedBlocks.delete(blockId);
  if (reroot && top) top.parentId = '';
  for (const block of deleted.blocks) {
    block.updatedAt = world.now();
    block.modifiedBy = userId;
    world.blocks.set(block.id, block);
  }
  return deleted.blocks;
}
