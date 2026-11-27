/** Rearranging the content blocks of a card by drag and drop. */
import type { ContentOrder } from './wire.ts';

/** Where the dragged block lands relative to the block it is dropped on. */
export type DropPosition = 'left' | 'right' | 'aboveRow' | 'belowRow';

export interface RearrangeData {
  contentOrder: ContentOrder;
  srcBlockId: string;
  /** Row of the dragged block. */
  srcBlockX: number;
  /** Its place in the row, or -1 when it is alone in its row. */
  srcBlockY: number;
  dstBlockId: string;
  dstBlockX: number;
  dstBlockY: number;
  moveTo: DropPosition;
}

/**
 * The content order after a block was dropped. The order passed in is not
 * changed.
 *
 * An entry of the order is a block alone in its row, or a list of the blocks
 * that share a row. Dropping a block to the left or right of another puts
 * them in one row; dropping it above or below a row gives it a row of its
 * own. A row that is left with one block becomes a plain entry again.
 */
export function dragAndDropRearrange(data: RearrangeData): ContentOrder {
  const { srcBlockX, srcBlockY, moveTo, srcBlockId, dstBlockId } = data;
  const order: ContentOrder = data.contentOrder.map((entry) =>
    Array.isArray(entry) ? [...entry] : entry,
  );
  let dstX = data.dstBlockX;
  let dstY = data.dstBlockY;
  const rowAt = (index: number): string[] => order[index] as string[];

  // Take the block out first.
  if (srcBlockY > -1) {
    const row = rowAt(srcBlockX);
    row.splice(srcBlockY, 1);
    const [only] = row;
    if (row.length === 1 && only !== undefined && srcBlockX !== dstX)
      order.splice(srcBlockX, 1, only);
  } else {
    order.splice(srcBlockX, 1);
    if (dstX > srcBlockX) dstX -= 1;
  }

  // Within one row, removing a block before the target shifts the target.
  const sameRow = dstX === srcBlockX && dstY > srcBlockY && srcBlockY > -1;
  switch (moveTo) {
    case 'right':
      if (dstY > -1) {
        if (sameRow) dstY -= 1;
        rowAt(dstX).splice(dstY + 1, 0, srcBlockId);
      } else {
        order.splice(dstX, 1, [dstBlockId, srcBlockId]);
      }
      break;
    case 'left':
      if (dstY > -1) {
        if (sameRow) dstY -= 1;
        rowAt(dstX).splice(dstY, 0, srcBlockId);
      } else {
        order.splice(dstX, 1, [srcBlockId, dstBlockId]);
      }
      break;
    case 'aboveRow':
      order.splice(dstX, 0, srcBlockId);
      break;
    case 'belowRow':
      order.splice(dstX + 1, 0, srcBlockId);
      break;
  }
  return order;
}
