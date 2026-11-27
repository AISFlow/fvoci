/**
 * References: what the body of a card names of the cards and pages of its
 * workspace. A body holds an identifier and nothing else; what it names, and
 * whether the reader may see it, is asked of the backend for the reader, as
 * things are then.
 */
import type { ContainerKind } from '../logic/wire.ts';
import type { VersionsOfBodies } from './body-versions.ts';
import type { Paging } from './trash-reads.ts';

/** A card, page or unit as a reference shows it: what it is, what it is called, and where it is. */
export interface LinkedBlock {
  id: string;
  kind: 'card' | 'page' | 'unit';
  title: string;
  /** Its icon; empty for none. */
  icon: string;
  /** The board, wiki or course it is on. No reference names an entry of a diary. */
  containerId: string;
  containerKind: ContainerKind;
  /** A card's number on its board; none for a page or a unit. */
  number?: number;
}

/** How many references are resolved in one request (the server's limit). */
export const REFERENCES_AT_ONCE = 100;

export interface References {
  /**
   * The cards and pages among `ids` (at most `REFERENCES_AT_ONCE`) that the
   * reader may see now. Whatever else an identifier names is left out, and
   * nothing says why: nothing there, in the trash, or not the reader's to see.
   */
  resolve(workspaceId: string, ids: readonly string[]): Promise<LinkedBlock[]>;
  /**
   * The cards and pages the reader may see whose body, or a comment on which,
   * names the block: the oldest first, a page of them. 404 for a block the
   * reader may not see, as for none.
   */
  to(workspaceId: string, blockId: string, paging?: Paging): Promise<LinkedBlock[]>;
  /**
   * The cards and pages the reader may see whose title has every word of
   * `term`, twenty at most, the one changed last first; for an empty term the
   * ones changed last.
   */
  search(workspaceId: string, term: string): Promise<LinkedBlock[]>;
  /**
   * The card of the board whose number it is (`#12`), if the reader may see
   * it; nothing for none, as for one in the trash, a template, or a board the
   * reader may not see.
   */
  numbered(workspaceId: string, boardId: string, number: number): Promise<LinkedBlock | undefined>;
}

/** What a backend keeps of the bodies of its cards: their versions, and what they name. */
export interface OfBodies extends VersionsOfBodies {
  readonly references: References;
}
