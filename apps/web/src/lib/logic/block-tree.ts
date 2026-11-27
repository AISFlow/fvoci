/** Working with the blocks of a board as a set: merging updates in, copying a card with its content. */
import {
  createId,
  idKindOfBlockType,
  normalizeBlock,
  remapBlockReferences,
  type Block,
} from './wire.ts';

/**
 * The blocks after an update arrived: updated blocks replace the ones with
 * the same identifier (at the end of the list), deleted ones drop out.
 */
export function mergeBlocks(blocks: readonly Block[], updatedBlocks: readonly Block[]): Block[] {
  const updatedIds = new Set(updatedBlocks.map((block) => block.id));
  return [
    ...blocks.filter((block) => !updatedIds.has(block.id)),
    ...updatedBlocks.filter((block) => block.deletedAt === undefined),
  ];
}

/**
 * A copy of a tree of blocks under new identifiers.
 *
 * `blocks` is the whole tree and `sourceBlockId` its root. Returns the
 * copies, the copy of the root, and the map from old to new identifiers. The
 * copy of the root keeps its parent, so it becomes a sibling of the original;
 * every other reference inside the tree (parents, the content order of a
 * card, the card order of a view) points at the copies.
 */
export function duplicateBlockTree(
  blocks: readonly Block[],
  sourceBlockId: string,
): [Block[], Block, Readonly<Record<string, string>>] {
  const idMap: Record<string, string> = {};
  const now = Date.now();
  const copies = blocks.map((block) => {
    const copy = normalizeBlock(block);
    copy.id = createId(idKindOfBlockType(copy.type));
    copy.createdAt = now;
    copy.updatedAt = now;
    delete copy.deletedAt;
    idMap[block.id] = copy.id;
    return copy;
  });

  const source = blocks.find((block) => block.id === sourceBlockId);
  const sourceCopy = copies.find((block) => block.id === idMap[sourceBlockId]);
  if (!source || !sourceCopy) throw new Error(`block ${sourceBlockId} is not in the tree`);

  // A root that is its own board: the copies belong to the copy of the root.
  const newBoardId = source.containerId === source.id ? sourceCopy.id : undefined;

  for (const copy of copies) {
    if (copy !== sourceCopy && copy.parentId) copy.parentId = idMap[copy.parentId] ?? copy.parentId;
    if (newBoardId) copy.containerId = newBoardId;
    remapBlockReferences(copy, idMap);
  }
  return [copies, sourceCopy, idMap];
}
