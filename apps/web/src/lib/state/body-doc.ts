/**
 * A card's body opened in its room (`Backend.openDoc`): the shapes the
 * backends and the screens share. The document is Yjs's; only its type is
 * named here, so that nothing of Yjs comes with the backends.
 */
import type { Awareness } from 'y-protocols/awareness';
import type { Doc as YDoc } from 'yjs';

/**
 * How an opened body stands (`Backend.openDoc`):
 *
 * - `joining`: asking to be let into the card's room; no document yet.
 * - `joined`: in the room with the server's document (`mode` says whether
 *   the reader may write in it).
 * - `receiving`: a document is held and the server sends it again (let in
 *   again, or the document was made anew); nothing can be written meanwhile.
 * - `offline`: no connection to the server; it is joined again once there is.
 * - `out`: not in the room, for `reason` (a reason of the server's, or
 *   `not_loaded` when the code of the room could not be fetched). It is
 *   tried again by itself when `retrying`.
 */
export type BodyDocStatus = 'joining' | 'joined' | 'receiving' | 'offline' | 'out';

export interface BodyDocState {
  status: BodyDocStatus;
  /** The server's document, once one has arrived; another one after it was made anew. */
  doc: YDoc | undefined;
  /**
   * The carets of that document: where the reader's own is is written into
   * it, and the others' are read from it. Another one with another document.
   */
  carets: Awareness | undefined;
  /**
   * Says that the reader composes a syllable with an input method in that
   * document (said again with every stroke of it), or no longer does. The
   * changes the room sends meanwhile are taken into the document after it,
   * or once the composition has been quiet for two seconds: a change in the
   * line being composed would end the composition.
   */
  composing: (composing: boolean) => void;
  /**
   * Takes what has the editor of that document read the selection of the
   * page, and gives what takes it back. It is called before a change the
   * room sent is taken into the document: a move of the reader's caret that
   * the editor has not read yet would be taken back by the change.
   */
  readsFirst: (read: () => void) => () => void;
  /** `write` or `read`: as the server let the reader in. */
  mode: 'write' | 'read';
  /** Why it is `out`. */
  reason: string | undefined;
  /** The reader may change the document now. */
  writable: boolean;
  /** The reader changed the document and the server is not known to have it all yet. */
  pending: boolean;
  /** Being `out`, it is tried again by itself. */
  retrying: boolean;
}

/** What the opener of a body is told. */
export interface BodyDocHears {
  /** The state changed (`BodyDoc.state` is the new one). */
  state(state: BodyDocState): void;
  /**
   * What the reader wrote and the server never took was dropped with the
   * document it was in; `why` is the server's reason, or `document_reset`
   * when the server made the document anew.
   */
  discarded(why: string): void;
}

/** A card's body, open in its room. */
export interface BodyDoc {
  readonly state: BodyDocState;
  /** Asks to be let in again now, after being put out. */
  retry(): void;
  /** Sends what waits at once; settles when the server has it, or cannot have it now. */
  settle(): Promise<void>;
  /** Leaves the room; the document is let go. */
  close(): void;
}

/** The rooms of a backend that has them (`Backend.openDoc`). */
export interface DocRooms {
  /**
   * Opens a card's body in its room, to be read and written together with
   * whoever else has it open. The document is the server's: the reader gets
   * none until it arrives (`hears.state`). A backend without rooms (the one
   * of a reader by a public link) has none of this: the body is read as it
   * was stored.
   */
  openDoc?(workspaceId: string, containerId: string, cardId: string, hears: BodyDocHears): BodyDoc;
}
