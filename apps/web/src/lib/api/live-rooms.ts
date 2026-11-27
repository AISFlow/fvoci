/**
 * The rooms of the page's one connection (`live.ts`): which of the server's
 * `doc*` messages and binary frames goes to which room, and the commands a
 * room sends (`contracts/websocket.md`, section 6).
 *
 * A room is named by a number the server gives when it lets a connection
 * in, another on every joining. The answer to a join names the card, not a
 * number, and answers come in the order of the joins: so the joins of a card
 * wait in a line. A card has one room on a connection: joining it again
 * takes the place of the number before. So a page opens a card's room once
 * at a time: a second opening of the same card is refused here
 * (`already_open`, tried again by the room as any refusal for now) while the
 * first is open.
 */
import { decodeCaretFrame, decodeDocFrame } from '../logic/doc-frame.ts';
import type { RoomHeld, RoomHears, RoomLine } from '../logic/doc-room-types.ts';

interface Seat {
  key: string;
  workspaceId: string;
  cardId: string;
  hears: RoomHears;
  doc: number | undefined;
  closed: boolean;
  /**
   * A number of the card that this seat's join, once let in, takes the place
   * of (the room of a seat closed meanwhile): left if the join is refused.
   */
  replaces: number | undefined;
}

/** What goes out; false when there is no connection to send on. */
export interface RoomSends {
  text(command: object): boolean;
  bytes(frame: Uint8Array<ArrayBuffer>): boolean;
}

const isMode = (value: unknown): value is 'write' | 'read' => value === 'write' || value === 'read';

const DOC_MESSAGE_TYPES = new Set([
  'docJoined',
  'docRefused',
  'docAck',
  'docNack',
  'docModeChanged',
  'docReset',
  'docLeft',
]);

/**
 * The most bytes of a state vector a join names: in base64 it stays well
 * under what the server takes of a command (64 KiB). A document so many
 * have written in that its state vector is larger is asked for whole.
 */
const NAMED_MOST = 32 * 1024;

/** What a `joinDoc` says of the document held: its epoch, and its state vector in base64. */
function saidOf(held: RoomHeld | undefined): { epoch?: number; stateVector?: string } {
  if (!held || held.stateVector.length > NAMED_MOST) return {};
  let binary = '';
  for (const byte of held.stateVector) binary += String.fromCharCode(byte);
  return { epoch: held.epoch, stateVector: btoa(binary) };
}

export class LiveRooms {
  readonly #sends: RoomSends;
  /** Told when the first room is opened and the last closed: the connection is kept for them. */
  readonly #wanted: () => void;
  readonly #seats = new Set<Seat>();
  readonly #byDoc = new Map<number, Seat>();
  /** The joins of each card that have no answer yet, oldest first. */
  readonly #joins = new Map<string, Seat[]>();
  #up = false;

  constructor(sends: RoomSends, wanted: () => void) {
    this.#sends = sends;
    this.#wanted = wanted;
  }

  /** How many rooms are open. */
  get size(): number {
    return this.#seats.size;
  }

  line(workspaceId: string, cardId: string): RoomLine {
    return (hears) => {
      const seat: Seat = {
        key: `${workspaceId}\n${cardId}`,
        workspaceId,
        cardId,
        hears,
        doc: undefined,
        closed: false,
        replaces: undefined,
      };
      this.#seats.add(seat);
      this.#wanted();
      const up = (): boolean => this.#up;
      return {
        get connected() {
          return up();
        },
        join: (held) => {
          if (!this.#up || seat.closed) return;
          if (this.#another(seat)) {
            queueMicrotask(() => {
              if (!seat.closed) seat.hears.message({ type: 'docRefused', reason: 'already_open' });
            });
            return;
          }
          const waiting = this.#joins.get(seat.key) ?? [];
          waiting.push(seat);
          this.#joins.set(seat.key, waiting);
          this.#sends.text({ type: 'joinDoc', workspaceId, cardId, ...saidOf(held) });
        },
        send: (frame) => {
          if (this.#up && !seat.closed) this.#sends.bytes(frame);
        },
        close: () => {
          if (seat.closed) return;
          seat.closed = true;
          this.#seats.delete(seat);
          this.#leave(seat);
          this.#wanted();
        },
      };
    };
  }

  /** Another open seat of the same card. */
  #another(seat: Seat): boolean {
    for (const other of this.#seats) if (other !== seat && other.key === seat.key) return true;
    return false;
  }

  #leave(seat: Seat): void {
    if (seat.doc === undefined) return;
    this.#byDoc.delete(seat.doc);
    if (this.#up) this.#sends.text({ type: 'leaveDoc', doc: seat.doc });
    seat.doc = undefined;
  }

  /** The connection opened. */
  up(): void {
    this.#up = true;
    for (const seat of [...this.#seats]) seat.hears.up();
  }

  /** The connection is gone: the server has taken its rooms away. */
  down(): void {
    if (!this.#up) return;
    this.#up = false;
    this.#byDoc.clear();
    this.#joins.clear();
    for (const seat of [...this.#seats]) {
      seat.doc = undefined;
      seat.hears.down();
    }
  }

  /** A text message: true when it was one of the rooms'. */
  text(message: Record<string, unknown>): boolean {
    const { type, doc, epoch, mode, reason } = message;
    if (typeof type !== 'string' || !DOC_MESSAGE_TYPES.has(type)) return false;
    if (type === 'docJoined' || type === 'docRefused') {
      const key = `${String(message.workspaceId)}\n${String(message.cardId)}`;
      const waiting = this.#joins.get(key);
      const seat = waiting?.shift();
      if (waiting?.length === 0) this.#joins.delete(key);
      if (!seat) return true;
      if (type === 'docRefused') {
        // The room before, if there was one, is still the server's: it is left.
        this.#leave(seat);
        if (seat.replaces !== undefined && this.#up) {
          this.#sends.text({ type: 'leaveDoc', doc: seat.replaces });
        }
        seat.replaces = undefined;
        if (!seat.closed) seat.hears.message({ type, reason: String(reason) });
        return true;
      }
      if (typeof doc !== 'number' || typeof epoch !== 'number' || !isMode(mode)) return true;
      // The number before is no room any more.
      if (seat.doc !== undefined) this.#byDoc.delete(seat.doc);
      seat.doc = doc;
      seat.replaces = undefined;
      if (seat.closed) {
        // Let in after it was closed: left, unless a later join of the card takes this one's place.
        const next = waiting?.[0];
        if (next) {
          seat.doc = undefined;
          next.replaces = doc;
        } else this.#leave(seat);
        return true;
      }
      this.#byDoc.set(doc, seat);
      seat.hears.message({ type, doc, epoch, mode });
      return true;
    }
    if (typeof doc !== 'number') return true;
    const seat = this.#byDoc.get(doc);
    // A room that was left: what was on its way is dropped.
    if (!seat) return true;
    if (type === 'docAck' && typeof epoch === 'number') {
      seat.hears.message({ type, doc, epoch });
    } else if (type === 'docNack' && typeof epoch === 'number') {
      seat.hears.message({ type, doc, epoch, reason: String(reason) });
    } else if (type === 'docReset' && typeof epoch === 'number') {
      seat.hears.message({ type, doc, epoch });
    } else if (type === 'docModeChanged' && isMode(mode)) {
      seat.hears.message({ type, doc, mode });
    } else if (type === 'docLeft') {
      this.#byDoc.delete(doc);
      seat.doc = undefined;
      seat.hears.message({ type, doc, reason: String(reason) });
    }
    return true;
  }

  /** A binary frame. One that is no sync message and no carets, or of no room of this connection, is dropped. */
  bytes(data: Uint8Array): void {
    const frame = decodeDocFrame(data);
    if (frame) {
      this.#byDoc.get(frame.doc)?.hears.frame(frame);
      return;
    }
    const carets = decodeCaretFrame(data);
    if (carets) this.#byDoc.get(carets.doc)?.hears.carets(carets);
  }
}
