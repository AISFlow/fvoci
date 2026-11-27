/**
 * One room of a card's body edited together, as its client sees it: a state
 * machine with no input or output of its own (`contracts/websocket.md`,
 * section 6). It is told what happens (`take`) and answers what is to be
 * done (`RoomEffect`); whoever runs it owns the connection, the timers and
 * the Y document, of which this knows only bytes.
 *
 * The phases:
 *
 * - `offline`: no connection. A document held is kept, to go on from.
 * - `joining`: asked to be let in, no document held yet.
 * - `joined`: in the room, with the document; `mode` says whether it may write.
 * - `receiving`: a document is held and another whole one is on its way
 *   (let in again, or the server made the document anew). Nothing is sent.
 * - `out`: not in the room, with the server's reason. Some reasons are tried
 *   again after a wait; `not_found` is not.
 *
 * The document is received again in four cases. `docReset`: a new `epoch`
 * is another document (the one held is dropped), the same is merged.
 * `docNack`: the refused change is in the reader's document alone, so the
 * room is joined once more and the document dropped for the server's (a
 * change the server only could not take just now is kept and offered
 * again). `docLeft`, and a connection that came back: joined again, saying
 * which document is held, and one of the same `epoch` goes on: the server
 * sends its state vector and what the reader lacks, or the whole, and what
 * it lacks of the reader's goes to it.
 * Whenever a document is dropped with changes of the reader the server never
 * took, that is said (`discarded`).
 */

import { packFitting, SYNC_STEP_1, SYNC_STEP_2, SYNC_UPDATE } from './doc-frame.ts';
import {
  ANSWER_WAIT,
  FINAL,
  FLUSH_WAIT,
  PACE,
  PACE_WAIT,
  RESET_WAIT,
  RETRY_WAITS,
  ROOM_LIMITS,
  type RoomLimits,
} from './doc-room-waits.ts';
import type {
  DocMode,
  RoomEffect,
  RoomEvent,
  RoomFrame,
  RoomMessage,
  RoomPhase,
  RoomStatus,
  RoomTimer,
} from './doc-room-types.ts';

export type * from './doc-room-types.ts';
export {
  FLUSH_WAIT,
  PACE_WAIT,
  RESET_WAIT,
  ROOM_LIMITS,
  type RoomLimits,
} from './doc-room-waits.ts';

export class DocRoom {
  readonly #merge: (updates: Uint8Array[]) => Uint8Array;
  readonly #limits: RoomLimits;
  #up = false;
  #phase: RoomPhase = 'offline';
  #mode: DocMode = 'read';
  #reason: string | undefined;
  /** The room's number on this connection, while it has one. */
  #doc: number | undefined;
  /** The epoch of the document held. */
  #epoch: number | undefined;
  /** The next frames of the room are the document of this epoch: the whole, or the server's state vector and what the one held lacks. */
  #expect: number | undefined;
  /** The server's state vector, sent before what the document held lacks. */
  #theirs: Uint8Array | undefined;
  /** A whole document of the epoch held is merged into it; otherwise it takes its place. */
  #keep = true;
  /** Why the document held is not kept. */
  #why = '';
  /** A refusal as out of step waits for its `docReset`. */
  #waiting = false;
  /** Changes not yet put into a frame, frames not yet sent, frames not yet answered. */
  #buffer: Uint8Array[] = [];
  #outbox: Uint8Array[] = [];
  #unacked = 0;
  #pending = false;
  /** A document read only was changed here (the editor's own repair): it is not the server's any more. */
  #tainted = false;
  #tries = 0;
  #retrying = false;
  /** A `joinDoc` is on its way: another is not sent until it is answered. */
  #joinSent = false;
  /** Answers still to come for frames given up while the room's number stayed: they are about nothing now. */
  #stale = 0;
  #effects: RoomEffect[] = [];

  constructor(merge: (updates: Uint8Array[]) => Uint8Array, limits: RoomLimits = ROOM_LIMITS) {
    this.#merge = merge;
    this.#limits = limits;
  }

  get status(): RoomStatus {
    return {
      phase: this.#phase,
      mode: this.#mode,
      reason: this.#reason,
      held: this.#epoch !== undefined,
      writable: this.#phase === 'joined' && this.#mode === 'write' && !this.#waiting,
      pending: this.#pending,
      retrying: this.#retrying,
    };
  }

  /** What happened; answers what is to be done, in order. */
  take(event: RoomEvent): RoomEffect[] {
    const before = this.status;
    this.#effects = [];
    switch (event.type) {
      case 'up':
        this.#up = true;
        // Out for good stays out; anything else is joined anew on the new connection.
        if (!this.#outForGood()) this.#join();
        break;
      case 'down':
        this.#up = false;
        this.#answered();
        this.#leaveRoom();
        this.#cancel('retry');
        this.#retrying = false;
        if (!this.#outForGood()) this.#phase = 'offline';
        break;
      case 'message':
        this.#message(event.message);
        break;
      case 'frame':
        this.#frame(event.frame);
        break;
      case 'carets': {
        const { doc, epoch, update } = event.carets;
        // Of the document held, in the room this is in; not while another document is on its way.
        if (doc === this.#doc && epoch === this.#epoch && this.#expect === undefined) {
          this.#effects.push({ type: 'carets', update });
        }
        break;
      }
      case 'caret':
        this.#tell(event.update);
        break;
      case 'local':
        this.#local(event.update);
        break;
      case 'missing':
        this.#missing(event.update);
        break;
      case 'timer':
        this.#timer(event.timer);
        break;
      case 'retry':
        this.#tries = 0;
        this.#timer('retry');
        break;
      case 'settle':
        this.#flush(true);
        break;
    }
    const after = this.status;
    const keys = Object.keys(after) as (keyof RoomStatus)[];
    if (keys.some((key) => after[key] !== before[key])) this.#effects.push({ type: 'changed' });
    return this.#effects;
  }

  #outForGood(): boolean {
    return this.#phase === 'out' && FINAL.has(this.#reason ?? '');
  }

  #cancel(timer: RoomTimer): void {
    this.#effects.push({ type: 'cancel', timer });
  }

  /** Asks to be let in. The number held, if any, stays the room's until the answer. */
  #join(): void {
    if (!this.#up || this.#joinSent) return;
    this.#cancel('retry');
    this.#retrying = false;
    this.#expect = undefined;
    this.#phase = this.#epoch === undefined ? 'joining' : 'receiving';
    this.#joinSent = true;
    // A document held and kept is named: the server sends what it lacks, not the whole.
    const held = this.#keep ? this.#epoch : undefined;
    this.#effects.push({ type: 'join', held });
    this.#effects.push({ type: 'timer', timer: 'answer', wait: ANSWER_WAIT });
  }

  /** The join on its way was answered, or will not be. */
  #answered(): void {
    this.#joinSent = false;
    this.#cancel('answer');
  }

  /** The reader's caret goes to the room when they write in it: who reads shows none. */
  #tell(update: Uint8Array): void {
    const doc = this.#doc;
    const epoch = this.#epoch;
    if (this.#phase !== 'joined' || this.#mode !== 'write') return;
    if (doc === undefined || epoch === undefined) return;
    this.#effects.push({ type: 'tell', carets: { doc, epoch, update } });
  }

  /** Forgets the room's number and what waited to go to it: none of it will be answered. */
  #leaveRoom(): void {
    if (this.#doc !== undefined) this.#effects.push({ type: 'unseen' });
    this.#doc = undefined;
    this.#stale = 0;
    this.#expect = undefined;
    this.#theirs = undefined;
    this.#waiting = false;
    this.#cancel('reset');
    this.#unqueue();
  }

  #unqueue(): void {
    this.#buffer = [];
    this.#outbox = [];
    // In the same room, the answers to what was sent still come: they are not about what is sent next.
    if (this.#doc !== undefined) this.#stale += this.#unacked;
    this.#unacked = 0;
    this.#cancel('flush');
  }

  /** Joins again for the server's document, dropping the one held: what it had of the reader's is lost for `why`. */
  #rejoin(why: string): void {
    this.#leaveRoom();
    this.#keep = false;
    this.#why = why;
    if (this.#up) this.#join();
  }

  /** Not in the room, for `reason`: for good, or until the next try. */
  #out(reason: string): void {
    this.#leaveRoom();
    this.#phase = 'out';
    this.#reason = reason;
    if (FINAL.has(reason)) {
      this.#cancel('retry');
      this.#retrying = false;
      this.#drop(reason);
      return;
    }
    const wait = RETRY_WAITS[Math.min(this.#tries, RETRY_WAITS.length - 1)] ?? 30_000;
    this.#tries += 1;
    this.#retrying = true;
    this.#effects.push({ type: 'timer', timer: 'retry', wait });
  }

  /** Lets go of the document held, saying so when changes of the reader go with it. */
  #drop(why: string): void {
    if (this.#epoch === undefined) return;
    if (this.#pending) this.#effects.push({ type: 'discarded', why });
    this.#pending = false;
    this.#tainted = false;
    this.#epoch = undefined;
    this.#keep = true;
    this.#effects.push({ type: 'drop' });
  }

  #message(message: RoomMessage): void {
    if (message.type === 'docJoined') {
      this.#answered();
      this.#leaveRoom();
      this.#doc = message.doc;
      this.#mode = message.mode;
      this.#expect = message.epoch;
      this.#tries = 0;
      // Nothing is written until the whole document has come.
      if (this.#epoch !== undefined) this.#phase = 'receiving';
      return;
    }
    if (message.type === 'docRefused') {
      this.#answered();
      // Still held: an answer to the try of a room that is out, or to joining again.
      if (PACE.has(message.reason)) this.#paced();
      else this.#out(message.reason);
      return;
    }
    // Of a room this is no longer in, or never was.
    if (message.doc !== this.#doc) return;
    switch (message.type) {
      case 'docAck':
        if (this.#answersDropped(message.epoch)) return;
        this.#unacked = Math.max(0, this.#unacked - 1);
        this.#settled();
        break;
      case 'docNack':
        if (this.#answersDropped(message.epoch)) return;
        // Answered: whatever is given up next does not wait for this answer.
        this.#unacked = Math.max(0, this.#unacked - 1);
        this.#refused(message.reason);
        break;
      case 'docModeChanged':
        if (message.mode === 'write' && this.#mode === 'read' && this.#tainted) {
          this.#mode = 'write';
          this.#rejoin('');
        } else if (message.mode === 'read' && this.#buffer.length + this.#outbox.length > 0) {
          // What was gathered and not sent cannot go any more: it is the reader's document alone.
          this.#mode = 'read';
          this.#rejoin('document_read_only');
        } else this.#mode = message.mode;
        break;
      case 'docReset':
        this.#cancel('reset');
        this.#theirs = undefined;
        this.#expect = message.epoch;
        if (message.epoch !== this.#epoch) {
          this.#unqueue();
          // What is still to be answered is of the old epoch, and is told apart by it.
          this.#stale = 0;
          this.#keep = false;
          this.#why = 'document_reset';
          this.#waiting = false;
          this.#phase = 'receiving';
        }
        break;
      case 'docLeft':
        if (PACE.has(message.reason)) this.#paced();
        else this.#out(message.reason);
        break;
    }
  }

  /**
   * Whether an answer of the server is about changes already given up: of
   * another document than the one held, of the one held while another is on
   * its way (`docReset` to a new `epoch`), or while a refusal waits for its
   * `docReset`.
   */
  #answersDropped(epoch: number): boolean {
    // Of another document: told apart by its epoch alone.
    if (epoch !== this.#epoch) return true;
    if (this.#stale > 0) {
      this.#stale -= 1;
      return true;
    }
    if (this.#waiting) return true;
    return this.#expect !== undefined && this.#expect !== this.#epoch;
  }

  /** Put out, or kept out, for the pace of the frames: joined again after a pause, as it was left. */
  #paced(): void {
    this.#leaveRoom();
    this.#phase = this.#epoch === undefined ? 'joining' : 'receiving';
    this.#effects.push({ type: 'timer', timer: 'retry', wait: PACE_WAIT });
  }

  /** The server did not take a change: it is in the document held and nowhere else. */
  #refused(reason: string): void {
    if (reason === 'update_out_of_step') {
      // The document may just have been made anew: its `docReset` is on its way.
      this.#unqueue();
      this.#waiting = true;
      this.#effects.push({ type: 'timer', timer: 'reset', wait: RESET_WAIT });
    } else if (reason === 'document_unavailable') {
      // Nothing is wrong with the change: it is offered again once let in.
      this.#leaveRoom();
      this.#phase = 'receiving';
      this.#effects.push({ type: 'timer', timer: 'retry', wait: PACE_WAIT });
    } else {
      if (reason === 'document_read_only') this.#mode = 'read';
      this.#rejoin(reason);
    }
  }

  #frame(frame: RoomFrame): void {
    if (frame.doc !== this.#doc) return;
    if (this.#expect !== undefined) {
      // Of the document before: the new one comes in the first frames of its epoch.
      if (frame.epoch !== this.#expect) return;
      // The server's state vector first: what follows is what the document held lacks.
      if (frame.sync === SYNC_STEP_1) this.#theirs = frame.payload;
      else if (frame.sync === SYNC_STEP_2) this.#whole(frame.payload, frame.epoch);
      return;
    }
    if (frame.epoch !== this.#epoch) return;
    if (frame.sync === SYNC_STEP_2 || frame.sync === SYNC_UPDATE) {
      this.#effects.push({ type: 'apply', update: frame.payload });
    }
  }

  #whole(update: Uint8Array, epoch: number): void {
    this.#expect = undefined;
    const resumed = this.#waiting;
    this.#waiting = false;
    const theirs = this.#theirs;
    this.#theirs = undefined;
    if (theirs && !(this.#epoch === epoch && this.#keep)) {
      // What a document no longer kept lacks is no document: the whole is asked for.
      this.#rejoin(this.#why);
      return;
    }
    if (this.#epoch === epoch && this.#keep) {
      this.#effects.push({ type: 'apply', update });
      // The same document: what the server lacks of the reader's changes goes to it again,
      // by its state vector if it sent one, else by the whole it sent.
      if (this.#pending && (this.#phase !== 'joined' || resumed)) {
        this.#unqueue();
        this.#effects.push(theirs ? { type: 'answer', theirs } : { type: 'offer', have: update });
      }
    } else {
      if (this.#pending)
        this.#effects.push({ type: 'discarded', why: this.#why || 'document_reset' });
      this.#pending = false;
      this.#tainted = false;
      this.#unqueue();
      this.#epoch = epoch;
      this.#effects.push({ type: 'fresh', update });
    }
    this.#keep = true;
    this.#why = '';
    this.#reason = undefined;
    this.#phase = 'joined';
  }

  #missing(update: Uint8Array | undefined): void {
    if (this.#phase !== 'joined' || this.#waiting) return;
    if (update === undefined) {
      this.#settled();
      return;
    }
    // Let in to read only since: the changes cannot go, and the document is not the server's.
    if (this.#mode === 'read') this.#rejoin('document_read_only');
    else this.#queue(update);
  }

  #local(update: Uint8Array): void {
    if (this.#epoch === undefined) return;
    if (this.#phase === 'joined' && !this.#waiting) {
      if (this.#mode === 'write') this.#queue(update);
      else this.#tainted = true;
      return;
    }
    // Written while nothing can be sent: it goes with the rest when the room is joined again.
    this.#pending = true;
  }

  #queue(update: Uint8Array): void {
    const first = this.#buffer.length === 0 && this.#outbox.length === 0;
    this.#buffer.push(update);
    this.#pending = true;
    if (first) this.#effects.push({ type: 'timer', timer: 'flush', wait: FLUSH_WAIT });
  }

  #settled(): void {
    if (this.#phase !== 'joined' || this.#waiting) return;
    if (this.#unacked === 0 && this.#buffer.length === 0 && this.#outbox.length === 0) {
      this.#pending = false;
    }
  }

  #timer(timer: RoomTimer): void {
    if (timer === 'flush') this.#flush(false);
    else if (timer === 'reset') {
      if (this.#waiting) this.#rejoin('update_out_of_step');
    } else if (timer === 'answer') {
      if (this.#joinSent) {
        this.#answered();
        this.#out('document_unavailable');
      }
    } else if (this.#doc === undefined && this.#phase !== 'joined') this.#join();
  }

  /** Puts what was gathered into frames and sends the next one; with `all`, every one. */
  #flush(all: boolean): void {
    const doc = this.#doc;
    const epoch = this.#epoch;
    if (this.#phase !== 'joined' || this.#mode !== 'write' || this.#waiting) return;
    if (doc === undefined || epoch === undefined) return;
    // One change larger than the server takes cannot stay in the document; what was
    // written before it goes first.
    const packed = packFitting(this.#buffer, this.#limits.updateBytes, this.#merge);
    const tooLarge = packed.tooLarge;
    this.#outbox.push(...packed.payloads);
    this.#buffer = [];
    for (let payload = this.#outbox.shift(); payload; payload = this.#outbox.shift()) {
      this.#effects.push({ type: 'send', frame: { doc, epoch, sync: SYNC_UPDATE, payload } });
      this.#unacked += 1;
      if (!all && !tooLarge) break;
    }
    if (tooLarge) {
      this.#rejoin('update_too_large');
      return;
    }
    if (this.#outbox.length > 0) {
      this.#effects.push({ type: 'timer', timer: 'flush', wait: FLUSH_WAIT });
    }
  }
}
