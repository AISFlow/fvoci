/**
 * A card's body as a document edited together, on a way into its room
 * (`RoomLine`): the page's connection to the server, or the rooms of the
 * backend in memory. It runs the room's state machine (`logic/doc-room.ts`):
 * holds the Y document the machine speaks of only as bytes, keeps its
 * timers, and sends what it says.
 *
 * The document is the server's: nothing is ever made here from a body. This
 * file brings Yjs with it, so it is fetched when a body is first opened
 * (`doc-opener.ts`) and is no part of the first screen.
 */
import * as Y from 'yjs';

import { registerActiveBodySettler } from '../chunk-reload.svelte.ts';
import { encodeCaretFrame, encodeDocFrame } from '../logic/doc-frame.ts';
import {
  DocRoom,
  type RoomEffect,
  type RoomEvent,
  type RoomLine,
  type RoomPort,
  type RoomTimer,
} from '../logic/doc-room.ts';

import type { BodyDoc, BodyDocHears, BodyDocState } from './backend.ts';
import { Carets } from './doc-carets.ts';

/**
 * How long after the last sign of a syllable being composed what the room
 * sent is held back, in milliseconds: a composition left open (a keyboard
 * that composes whole words, a reader who stopped mid-syllable) does not
 * keep the others' writing away.
 */
export const HELD_IDLE = 2000;

/** How long leaving a card waits for the server to take what was written, in milliseconds. */
const SETTLE_WAIT = 3000;

class RoomClient implements BodyDoc {
  readonly #room = new DocRoom((updates) => Y.mergeUpdates(updates));
  readonly #hears: BodyDocHears;
  readonly #port: RoomPort;
  readonly #timers = new Map<RoomTimer, ReturnType<typeof setTimeout>>();
  /** Whoever waits for the server to have everything (`settle`). */
  #settling: (() => void)[] = [];
  #doc: Y.Doc | undefined;
  /** The carets of the document held. */
  #carets: Carets | undefined;
  /** The reader's caret could be told to the room when it was last looked at. */
  #tells = false;
  /** The room took the caret it was just told. */
  #told = false;
  /** The changes the room sent while the reader composes a syllable, in their order; undefined while they do not. */
  #held: (() => void)[] | undefined;
  #heldTimer: ReturnType<typeof setTimeout> | undefined;
  /** What has the editor read the selection of the page (`BodyDocState.readsFirst`). */
  #read: (() => void) | undefined;
  readonly #unregisterSettler: () => void;
  #closed = false;

  /** How long the room's changes are held after the last stroke (`HELD_IDLE`). */
  readonly #heldIdle: number;

  constructor(line: RoomLine, hears: BodyDocHears, heldIdle: number) {
    this.#hears = hears;
    this.#heldIdle = heldIdle;
    this.#unregisterSettler = registerActiveBodySettler(() => this.settle());
    this.#port = line({
      up: () => {
        this.#take({ type: 'up' });
      },
      down: () => {
        this.#take({ type: 'down' });
      },
      message: (message) => {
        this.#take({ type: 'message', message });
      },
      frame: (frame) => {
        this.#take({ type: 'frame', frame });
      },
      carets: (carets) => {
        this.#take({ type: 'carets', carets });
      },
    });
    if (this.#port.connected) this.#take({ type: 'up' });
  }

  get state(): BodyDocState {
    const { phase, mode, reason, writable, pending, retrying } = this.#room.status;
    const doc = this.#doc;
    const carets = this.#carets?.awareness;
    const [composing, readsFirst] = [this.#composing, this.#readsFirst];
    const told = { status: phase, doc, carets, composing, readsFirst, mode, reason };
    return { ...told, writable, pending, retrying };
  }

  retry(): void {
    this.#take({ type: 'retry' });
  }

  /**
   * Sends what waits now, and answers once the server has everything the
   * reader wrote, or cannot be given it for now (no connection, not in the
   * room), or after a while.
   */
  settle(): Promise<void> {
    this.#take({ type: 'settle' });
    if (!this.#waitsForAnswers()) return Promise.resolve();
    return new Promise((resolve) => {
      const done = (): void => {
        clearTimeout(timer);
        this.#settling = this.#settling.filter((other) => other !== done);
        resolve();
      };
      const timer = setTimeout(done, SETTLE_WAIT);
      this.#settling.push(done);
    });
  }

  /** The editor's reading of the page, until it is taken back (`BodyDocState.readsFirst`). */
  readonly #readsFirst = (read: () => void): (() => void) => {
    this.#read = read;
    return () => {
      if (this.#read === read) this.#read = undefined;
    };
  };

  /** The reader composes a syllable, or no longer does (`BodyDocState.composing`). */
  readonly #composing = (composing: boolean): void => {
    if (this.#closed) return;
    clearTimeout(this.#heldTimer);
    this.#heldTimer = undefined;
    if (composing) {
      this.#held ??= [];
      // Said again with every stroke of the syllable: quiet for a while, it is held no longer.
      this.#heldTimer = setTimeout(() => {
        this.#composing(false);
      }, this.#heldIdle);
      return;
    }
    const held = this.#held ?? [];
    this.#held = undefined;
    for (const take of held) take();
  };

  /**
   * Takes in a change the room sent: now, or after the syllable being
   * composed (a change in its line would end the composition).
   */
  #later(take: () => void): void {
    if (this.#held) this.#held.push(take);
    else take();
  }

  #waitsForAnswers(): boolean {
    const { phase, pending } = this.#room.status;
    return !this.#closed && pending && phase === 'joined';
  }

  close(): void {
    if (this.#closed) return;
    // What was gathered goes first: the server takes it in order, before the leaving.
    this.#take({ type: 'settle' });
    this.#closed = true;
    this.#unregisterSettler();
    this.#port.close();
    for (const timer of this.#timers.values()) clearTimeout(timer);
    this.#timers.clear();
    clearTimeout(this.#heldTimer);
    this.#held = undefined;
    this.#hold(undefined);
    for (const done of [...this.#settling]) done();
  }

  #isClosed(): boolean {
    return this.#closed;
  }

  #take(event: RoomEvent): void {
    if (this.#closed) return;
    const held = this.#doc;
    let changed = false;
    for (const effect of this.#room.take(event)) {
      // Closed by whoever was told something (`discarded`): the rest is not done.
      if (this.#isClosed()) return;
      if (effect.type === 'changed') changed = true;
      else this.#run(effect);
    }
    this.#tellAgain();
    if (changed || held !== this.#doc) {
      this.#hears.state(this.state);
      if (!this.#waitsForAnswers()) for (const done of [...this.#settling]) done();
    }
  }

  /** In the room to write, where it was not before: the room is told where the reader's caret is. */
  #tellAgain(): void {
    const { phase, mode } = this.#room.status;
    const tells = phase === 'joined' && mode === 'write';
    if (tells && !this.#tells) this.#carets?.again();
    this.#tells = tells;
  }

  /** Tells the room the reader's caret; whether it could be told now. */
  #tellCaret(update: Uint8Array): boolean {
    this.#told = false;
    this.#take({ type: 'caret', update });
    return this.#told;
  }

  #run(effect: Exclude<RoomEffect, { type: 'changed' }>): void {
    switch (effect.type) {
      case 'join': {
        const doc = effect.held === undefined ? undefined : this.#doc;
        this.#port.join(doc && { epoch: effect.held ?? 0, stateVector: Y.encodeStateVector(doc) });
        break;
      }
      case 'answer': {
        // A state vector that cannot be read says nothing of what the server holds: all of it goes.
        const doc = this.#doc;
        const update = doc && (lackedBy(doc, effect.theirs) ?? Y.encodeStateAsUpdate(doc));
        if (update) this.#take({ type: 'missing', update });
        break;
      }
      case 'send':
        this.#port.send(encodeDocFrame(effect.frame));
        break;
      case 'apply': {
        // Into the document it was sent for: one let go meanwhile takes nothing.
        const doc = this.#doc;
        this.#later(() => {
          if (!doc || doc !== this.#doc) return;
          // Where the reader's caret went is read first: the change would take an unread move back.
          this.#read?.();
          this.#apply(doc, effect.update);
        });
        break;
      }
      case 'tell':
        this.#port.send(encodeCaretFrame(effect.carets));
        this.#told = true;
        break;
      // A caret does not wait for a syllable: it ends no composition, and one whose place is in
      // a change that waits is drawn once that change is in.
      case 'carets':
        this.#carets?.take(effect.update);
        break;
      case 'unseen':
        this.#carets?.away();
        break;
      case 'fresh': {
        const doc = new Y.Doc();
        this.#apply(doc, effect.update);
        this.#hold(doc);
        break;
      }
      case 'drop':
        this.#hold(undefined);
        break;
      case 'offer': {
        const doc = this.#doc;
        if (!doc) break;
        this.#take({ type: 'missing', update: missingFrom(doc, effect.have) });
        break;
      }
      case 'discarded':
        this.#hears.discarded(effect.why);
        break;
      case 'timer':
        clearTimeout(this.#timers.get(effect.timer));
        this.#timers.set(
          effect.timer,
          setTimeout(() => {
            this.#timers.delete(effect.timer);
            this.#take({ type: 'timer', timer: effect.timer });
          }, effect.wait),
        );
        break;
      case 'cancel':
        clearTimeout(this.#timers.get(effect.timer));
        this.#timers.delete(effect.timer);
        break;
    }
  }

  /** Merges what the server sent; bytes that are no update are dropped (the server sends none). */
  #apply(doc: Y.Doc, update: Uint8Array): void {
    try {
      Y.applyUpdate(doc, update, this);
    } catch {
      // Nothing of it was taken in.
    }
  }

  readonly #changed = (update: Uint8Array, origin: unknown): void => {
    // What came from the room is not sent back to it.
    if (origin !== this) this.#take({ type: 'local', update });
  };

  /** Holds `doc` in place of the one before, which is let go. */
  #hold(doc: Y.Doc | undefined): void {
    const before = this.#doc;
    if (before === doc) return;
    before?.off('update', this.#changed);
    this.#carets?.destroy();
    // What waited for a syllable was the document before's; its editor goes with it.
    clearTimeout(this.#heldTimer);
    this.#heldTimer = undefined;
    this.#held = undefined;
    this.#doc = doc;
    this.#carets = doc ? new Carets(doc, (update) => this.#tellCaret(update)) : undefined;
    // A new document's carets are told once the editor says where the reader is.
    this.#tells = false;
    doc?.on('update', this.#changed);
    before?.destroy();
  }
}

/**
 * What `doc` has that the document of `whole` (the server's, as it sent it)
 * lacks; undefined when nothing. A deletion made here alone shows in no
 * state vector, so the two are compared as snapshots (states and deletions).
 */
export function missingFrom(doc: Y.Doc, whole: Uint8Array): Uint8Array | undefined {
  const theirs = new Y.Doc();
  try {
    Y.applyUpdate(theirs, whole);
    const before = Y.snapshot(theirs);
    const update = Y.encodeStateAsUpdate(doc, Y.encodeStateVector(theirs));
    Y.applyUpdate(theirs, update);
    return Y.equalSnapshots(before, Y.snapshot(theirs)) ? undefined : update;
  } catch {
    return undefined;
  } finally {
    theirs.destroy();
  }
}

/**
 * What `doc` has that a holder of the state vector `theirs` lacks, with
 * every deletion `doc` knows of; undefined when `theirs` is no state vector.
 */
function lackedBy(doc: Y.Doc, theirs: Uint8Array): Uint8Array | undefined {
  try {
    return Y.encodeStateAsUpdate(doc, theirs);
  } catch {
    return undefined;
  }
}

/**
 * Opens a card's body on a way into its room. `heldIdle` is how long the
 * room's changes wait after the last stroke of a syllable: `HELD_IDLE`
 * unless the backend in memory is told otherwise (`World.heldIdle`).
 */
export function openRoom(line: RoomLine, hears: BodyDocHears, heldIdle = HELD_IDLE): BodyDoc {
  return new RoomClient(line, hears, heldIdle);
}
