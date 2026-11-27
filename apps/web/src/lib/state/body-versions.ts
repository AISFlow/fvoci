/**
 * The versions the server keeps of a card's body: when the last of those
 * who wrote it leave, at an interval while it is written on (ten minutes,
 * unless the server is set otherwise), around a body written whole, and
 * when an editor asks (`keep`). The newest are kept (fifty, unless the
 * server is set otherwise): the lowest numbers go as new versions come.
 */
import type { DocNode } from '../editor/schema.ts';

/**
 * How a version came to be kept: `manual`, asked for by an editor; `restored`,
 * the body a version was gone back to; `agent`, the body after an agent wrote
 * in it through a personal API token (`agentTokenName`); `auto`, by the server
 * on its own (as its writers leave, by the interval, around a body written
 * whole, before an agent writes, and every version kept before versions said how).
 */
export type BodyVersionCause = 'auto' | 'manual' | 'restored' | 'agent';

export interface BodyVersion {
  /** Its place among the card's, counted from 1. */
  version: number;
  /** When it was kept; of a body nobody is known to have written, when the card was last changed. */
  keptAt: number;
  /** Who wrote the body up to it since the version before; none for a body nobody is known to have written. */
  editors: string[];
  /**
   * Its body is the one the card has now. Of versions with the same body,
   * the newest; of none while what was typed is not written to the card yet.
   */
  isCurrent: boolean;
  cause: BodyVersionCause;
  /** The name of the token an agent wrote it through, for `agent`. */
  agentTokenName?: string | undefined;
}

export interface BodyVersionContent extends BodyVersion {
  /** The body of the card then; `null` if it had none. */
  body: DocNode | null;
}

/**
 * What became of a body an editor asked to keep now: `kept`, as `version`;
 * `unchanged`, it is the newest version already (`version`, or none for no
 * body and no version); `recent`, a version was kept by hand within the last
 * minute (`version`), and none is kept now.
 */
export interface KeptBodyVersion {
  outcome: 'kept' | 'unchanged' | 'recent';
  version: BodyVersion | null;
}

export interface BodyVersions {
  /** The versions of a card's body, the newest first, for the board's members. */
  list(boardId: string, cardId: string): Promise<BodyVersion[]>;
  /** One version with its body; 404 for one the card has no more. */
  read(boardId: string, cardId: string, version: number): Promise<BodyVersionContent>;
  /**
   * Writes a version's body to its card again, for who edits the board: the
   * card's room gets it as a new document, the body before stays a version,
   * and a card that has that body already is left as it is. 409 for a
   * version that cannot be stored any more.
   */
  restore(boardId: string, cardId: string, version: number): Promise<unknown>;
  /**
   * Keeps the card's body as a version now, for who edits the board: what
   * its room's editors typed is in it first, and the editors' documents are
   * left as they are.
   */
  keep(boardId: string, cardId: string): Promise<KeptBodyVersion>;
}

/** Where a backend keeps the versions of the bodies of its cards. */
export interface VersionsOfBodies {
  readonly bodyVersions: BodyVersions;
}
