/** The changes the backend tells as they happen, part of the data boundary (`backend.ts`). */
import type { Container, ContainerMember, BoardView } from '../logic/wire.ts';

import type { AccountSettings, Attachment, CardSummary, Category, Comment } from './backend.ts';
import type { WikiPage } from './wiki-types.ts';
import type { DiaryEntry } from './diary-types.ts';
import type { CourseUnit } from './course-types.ts';
import type { Reaction } from './reaction-types.ts';

/**
 * A change that happened on the server, whoever made it: the reader in this
 * tab, the reader in another one, or somebody else. Stores apply these the
 * same way they apply the answer to their own write, so applying one twice
 * does nothing.
 */
export type Change =
  | { kind: 'container'; container: Container }
  | { kind: 'containerDeleted'; containerId: string }
  /** Blocks that were added or changed. A card comes as its summary. */
  | {
      kind: 'blocks';
      containerId: string;
      blocks: (BoardView | CardSummary | Comment | Attachment)[];
    }
  /**
   * Blocks deleted: only the one the deletion was asked for, not what went
   * with it (the cards of a view do not go; the pages below a page do).
   */
  | { kind: 'blocksDeleted'; containerId: string; blockIds: string[] }
  /** Pages of a wiki (`containerId`) that were made or changed, without their bodies. */
  | { kind: 'pages'; containerId: string; pages: WikiPage[] }
  /** Entries of a diary (`containerId`) that were made or changed, without their bodies. */
  | { kind: 'entries'; containerId: string; entries: DiaryEntry[] }
  /** Chapters and units of a course (`containerId`) that were made or changed, without their bodies. */
  | { kind: 'units'; containerId: string; units: CourseUnit[] }
  | { kind: 'members'; containerId: string; members: ContainerMember[] }
  /** All the reactions to a block of the container as they are now (`reactionsChanged`). */
  | { kind: 'reactions'; containerId: string; blockId: string; reactions: readonly Reaction[] }
  | { kind: 'categories'; categories: Category[] }
  | { kind: 'settings'; settings: AccountSettings }
  /** Somebody's name changed: whoever holds them reads them again. */
  | { kind: 'user'; userId: string }
  /** What the reader was told changed: whoever shows it reads it again. */
  | { kind: 'notifications' }
  /** The reader's stars in the workspace changed, here or elsewhere: whoever shows them reads them again. */
  | { kind: 'favorites'; workspaceId: string }
  /** Somebody's workspace role changed (`admin`, `member`, `guest`, or `''` on removal). */
  | { kind: 'workspaceRole'; workspaceId: string; userId: string; role: string }
  /**
   * Changes may have been missed (the connection to the server dropped and
   * is back): read again what is shown. The memory backend never sends it.
   */
  | { kind: 'resync' };

export type ChangeListener = (change: Change) => void;

type ChangedBlock = Extract<Change, { kind: 'blocks' }>['blocks'][number];

/** Whether a changed block is a card. The types of the blocks overlap, so `type` has to be asked. */
export function isCardBlock(block: ChangedBlock): block is CardSummary {
  return block.type === 'card';
}

export function isViewBlock(block: ChangedBlock): block is BoardView {
  return block.type === 'view';
}

/** Whether a changed block is a comment, of a card or of a page. */
export function isCommentBlock(block: ChangedBlock): block is Comment {
  return block.type === 'comment';
}
