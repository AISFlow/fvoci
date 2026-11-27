/** Reading the trash, part of the data boundary (`backend.ts`). */
import type { ContainerKind } from '../logic/wire.ts';

import type { Mood } from './diary-types.ts';

/** A card or a board in the trash: enough to decide whether to bring it back. */
export interface DeletedItem {
  id: string;
  /** Its title when it was deleted. */
  title: string;
  deletedAt: number;
  /** Who deleted it, who may no longer exist; empty if not known. */
  deletedBy: string;
  /** In the trash of boards: a board or a wiki (a board when left out). */
  kind?: ContainerKind;
  /** In the trash of a wiki's pages: how many pages below it went with it, to come back with it. */
  below?: number;
  /** In the trash of a diary, whose entries are titled with their day: the mood of the day. */
  mood?: Mood | null;
}

/** A page of a list: `page` counts from 0, `perPage` from 1 to 1000 (`page-reads.ts`). */
export interface Paging {
  page: number;
  perPage: number;
}

/** What the backend lists of the trash, for whoever may bring it back. */
export interface TrashReads {
  /**
   * The workspace's boards and wikis in the trash that the reader was an admin
   * of (who may bring them back): a page, the last deleted first.
   */
  deletedContainers(workspaceId: string, paging?: Paging): Promise<DeletedItem[]>;
  /**
   * The board's cards in the trash, for its editors: a page, the last deleted
   * first. A card that went with the block it was on is not listed; it comes
   * back with that block.
   */
  deletedCards(boardId: string, paging?: Paging): Promise<DeletedItem[]>;
}
