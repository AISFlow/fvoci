/**
 * What the rooms of the backend in memory (rooms.ts) keep of a reader in a
 * room, of a room, and what the rooms of two tabs say to each other.
 */
import type * as Y from 'yjs';

import type { DocMode, RoomHears } from '../../logic/doc-room-types.ts';

import type { ToldCaret } from './room-carets.ts';

/** Who goes through a room, as the backend in memory acts for them. */
export interface RoomUser {
  readonly userId: string;
  /** Milliseconds every answer waits. */
  readonly latency: number;
}

export interface Seat {
  user: RoomUser;
  hears: RoomHears;
  /** The room's number while the seat is in it. */
  doc: number | undefined;
  mode: DocMode;
  /** In the room's view: until what the seat said before leaving has been taken. */
  up: boolean;
  /** The seat's client still listens: nothing is said to it once it has closed. */
  hearing: boolean;
  /** When the last thing each way arrives, and what is on its way: nothing overtakes what was sent before it. */
  inAt: number;
  outAt: number;
  coming: (() => void)[];
  going: (() => void)[];
}

export interface Room {
  containerId: string;
  cardId: string;
  doc: Y.Doc;
  epoch: number;
  seats: Set<Seat>;
  /** Who changed it last: the body is written in their name. */
  lastWriter: string | undefined;
  quiet: ReturnType<typeof setTimeout> | undefined;
  longest: ReturnType<typeof setTimeout> | undefined;
  /** What another tab's room has to be to be this one (`tabKey`). */
  tab: string;
}

/** What the worlds of two tabs say of a room; of a room another than this tab's (`tab`) it is not heard. */
export type TabMessage =
  | { kind: 'update'; cardId: string; tab: string; update: Uint8Array }
  | { kind: 'ask'; cardId: string; tab: string; vector: Uint8Array }
  | { kind: 'caret'; cardId: string; tab: string; caret: ToldCaret };
