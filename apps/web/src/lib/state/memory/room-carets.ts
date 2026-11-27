/**
 * The carets of the rooms of the backend in memory (rooms.ts), kept as the
 * server keeps them (`contracts/websocket.md`, section 6): where each
 * reader who writes is, told to the room's others under the room's own
 * word of whose it is.
 *
 * - Of what a seat sends only the two places are read (`caret-frame.ts`):
 *   the user is the seat's, the number and the clock the room's.
 * - A seat that withdraws its caret, leaves, is put out or reads from now
 *   on has none: the others are told it is gone.
 * - Who comes in is told the carets there are, but those nothing was heard
 *   of for thirty seconds.
 * - A document made anew leaves none.
 *
 * The rooms of the other tabs of the browser are told too, and theirs are
 * told here (`shareRooms`); a tab whose room comes to be later is told the
 * carets there are. A tab that goes away says nothing, nor one whose
 * document was made anew: its carets are let go by who holds them, thirty
 * seconds after its last word.
 *
 * Unlike the server, nothing is counted against a rate.
 */
import { CARET_BYTES, caretSaid, caretState, caretTold } from './caret-frame.ts';

/** How long a caret nothing was heard of is still told to who comes in, in milliseconds. */
const TOLD_FOR = 30_000;

/** A caret as a room tells it: `state` is `null` once it is gone. */
export interface ToldCaret {
  number: number;
  clock: number;
  state: string | null;
}

interface Caret extends ToldCaret {
  heard: number;
}

interface Ports<Seat, Room> {
  /** The user a seat acts for. */
  userOf: (seat: Seat) => string;
  /** Sends an awareness update to a seat of the room. */
  tell: (room: Room, to: Seat, update: Uint8Array) => void;
  /** Tells the rooms of the other tabs. */
  post: (room: Room, caret: ToldCaret) => void;
}

export class MemoryCarets<Seat, Room extends { seats: Set<Seat> }> {
  readonly #ports: Ports<Seat, Room>;
  readonly #rooms = new WeakMap<Room, { seats: Map<Seat, Caret>; tabs: Map<number, Caret> }>();
  /**
   * The last number given: past what the client of a Y document has for a
   * number (32 bits), as the server's are, and from a start of this tab's
   * own, drawn among 2^36, with 65,536 numbers to each: two tabs give one
   * number only by that chance.
   */
  #numbers = 2 ** 32 + Math.floor(Math.random() * 2 ** 36) * 2 ** 16;

  constructor(ports: Ports<Seat, Room>) {
    this.#ports = ports;
  }

  #of(room: Room): { seats: Map<Seat, Caret>; tabs: Map<number, Caret> } {
    let carets = this.#rooms.get(room);
    if (!carets) {
      carets = { seats: new Map(), tabs: new Map() };
      this.#rooms.set(room, carets);
    }
    return carets;
  }

  /** A seat that writes said something of its caret, in an awareness update. */
  said(room: Room, seat: Seat, update: Uint8Array): void {
    if (update.length > CARET_BYTES) return;
    const cursor = caretSaid(update);
    if (cursor === undefined) return;
    if (cursor === null) {
      this.gone(room, seat);
      return;
    }
    const { seats } = this.#of(room);
    let caret = seats.get(seat);
    if (!caret) {
      this.#numbers += 1;
      caret = { number: this.#numbers, clock: 0, state: null, heard: 0 };
      seats.set(seat, caret);
    }
    caret.clock += 1;
    caret.state = caretState(this.#ports.userOf(seat), cursor);
    caret.heard = Date.now();
    this.#spread(room, seat, caret);
  }

  /** A seat shows no caret any more: it withdrew it, left, was put out, or reads from now on. */
  gone(room: Room, seat: Seat): void {
    const caret = this.#rooms.get(room)?.seats.get(seat);
    if (typeof caret?.state !== 'string') return;
    caret.clock += 1;
    caret.state = null;
    this.#spread(room, seat, caret);
  }

  /** A seat is no longer in the room: its caret is gone, and nothing is kept of the seat. */
  left(room: Room, seat: Seat): void {
    this.gone(room, seat);
    this.#rooms.get(room)?.seats.delete(seat);
  }

  /** Another tab's room came to be: it is told the carets of this tab's readers, as who comes in here is. */
  tellTabs(room: Room): void {
    const since = Date.now() - TOLD_FOR;
    for (const { number, clock, state, heard } of this.#rooms.get(room)?.seats.values() ?? []) {
      if (state !== null && heard >= since) this.#ports.post(room, { number, clock, state });
    }
  }

  /** Tells a caret to the room's seats but the one it is of, and to the other tabs. */
  #spread(room: Room, from: Seat | undefined, caret: ToldCaret): void {
    const update = caretTold(caret.number, caret.clock, caret.state);
    for (const seat of room.seats) if (seat !== from) this.#ports.tell(room, seat, update);
    if (from !== undefined) {
      this.#ports.post(room, { number: caret.number, clock: caret.clock, state: caret.state });
    }
  }

  /** Who came into the room is told where the carets are. */
  joined(room: Room, seat: Seat): void {
    const carets = this.#rooms.get(room);
    if (!carets) return;
    const since = Date.now() - TOLD_FOR;
    const others = [...carets.seats].filter(([other]) => other !== seat).map(([, caret]) => caret);
    for (const caret of [...others, ...carets.tabs.values()]) {
      if (caret.state === null || caret.heard < since) continue;
      this.#ports.tell(room, seat, caretTold(caret.number, caret.clock, caret.state));
    }
  }

  /**
   * Another tab's room told a caret of one of its seats. A number that is no
   * room's (not past 2^32: it could be a reader's own in their document) is
   * not passed on.
   */
  fromTab(room: Room, told: ToldCaret): void {
    const { tabs } = this.#of(room);
    const { number, clock, state } = told;
    const known = tabs.get(number);
    if (!Number.isSafeInteger(number) || number <= 2 ** 32 || (known && known.clock >= clock)) {
      return;
    }
    // One that is gone is kept only as far as its clock, against an older word of it.
    tabs.set(number, { number, clock, state, heard: Date.now() });
    this.#spread(room, undefined, { number, clock, state });
  }

  /** The room's document was made anew: every caret was a place in the one before. */
  forget(room: Room): void {
    this.#rooms.delete(room);
  }
}
