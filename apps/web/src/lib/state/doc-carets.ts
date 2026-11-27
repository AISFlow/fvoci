/**
 * The carets of a body edited together, as the room's client holds them
 * (`doc-client.ts`): y-protocols' awareness of one Y document. It is given
 * to the editor, which writes where the reader's caret is into it and
 * draws the others' from it (`editor/carets.ts`).
 *
 * What the reader's editor says of its caret is told to the room: at most
 * once every `CARET_WAIT`, and again every fifteen seconds while it stays
 * (the library renews it; who holds a caret lets it go after thirty without
 * a word). A page that is hidden shows no caret: nobody is at it, and its
 * timers are too slow to keep one. Whose a caret is the room says, not the
 * reader (`contracts/websocket.md`, section 6). Out of the room the others'
 * carets are let go at once: the room tells them anew to who comes in.
 */
import {
  applyAwarenessUpdate,
  Awareness,
  encodeAwarenessUpdate,
  removeAwarenessStates,
} from 'y-protocols/awareness';
import type { Doc } from 'yjs';

/**
 * The least time between two tellings of the reader's caret, in
 * milliseconds: ten a second, beside the 33 frames of changes, where the
 * server allows 50 frames.
 */
export const CARET_WAIT = 100;

/** The page, where one is present (non-DOM runtimes and torn-down tests have none). */
const page = (): Document | undefined => (typeof document === 'undefined' ? undefined : document);

export class Carets {
  readonly awareness: Awareness;
  /** Sends the reader's state to the room; false when it cannot be told now. */
  readonly #tell: (update: Uint8Array) => boolean;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #toldAt = 0;
  /** The room was told a caret, and not yet that it is gone. */
  #shown = false;
  #gone = false;
  /** The document `#seen` was registered on, so `destroy` removes it from the same target. */
  #page: Document | undefined;

  constructor(doc: Doc, tell: (update: Uint8Array) => boolean) {
    this.awareness = new Awareness(doc);
    this.#tell = tell;
    this.awareness.on('update', this.#changed);
    this.#page = page();
    this.#page?.addEventListener('visibilitychange', this.#seen);
  }

  /** The page was hidden, or is shown again: the caret goes, or is told anew. */
  readonly #seen = (): void => {
    this.#soon();
  };

  /**
   * The reader's own state was set (by the editor, or renewed by the
   * library): told. Another's is gone: which telling of it was the last is
   * forgotten with it (the library keeps that for good; a room gives every
   * seat that comes in a number of its own).
   */
  readonly #changed = ({ removed }: { removed: number[] }, origin: unknown): void => {
    if (origin === 'local') this.#soon();
    for (const client of removed) {
      if (client !== this.awareness.clientID) this.awareness.meta.delete(client);
    }
  };

  /** Tells the room once the wait since the last telling is over; what was set meanwhile goes as one. */
  #soon(): void {
    if (this.#gone || this.#timer !== undefined) return;
    const wait = Math.max(0, this.#toldAt + CARET_WAIT - Date.now());
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      this.#say();
    }, wait);
  }

  #say(): void {
    const { awareness } = this;
    const cursor: unknown = awareness.getLocalState()?.cursor;
    const hidden = page()?.visibilityState === 'hidden';
    const shows = cursor !== null && cursor !== undefined && !hidden;
    // No caret, and the room knows of none: nothing to tell.
    if (!shows && !this.#shown) return;
    // A caret that is not shown is told as a state that is gone, whatever the state says.
    const states = shows ? awareness.getStates() : new Map<number, Record<string, unknown>>();
    const told = this.#tell(encodeAwarenessUpdate(awareness, [awareness.clientID], states));
    this.#toldAt = Date.now();
    this.#shown = told && shows;
  }

  /** The room can be told (again): where the reader's caret is is told anew. */
  again(): void {
    this.#shown = false;
    this.#soon();
  }

  /** The room said where carets are, or that they are gone. Bytes that are no such thing are dropped. */
  take(update: Uint8Array): void {
    try {
      applyAwarenessUpdate(this.awareness, update, this);
    } catch {
      // What of it was no state is dropped; the room tells one state at a time.
    }
  }

  /**
   * Out of the room: the others' carets are not known any more, nor which
   * telling of each was the last (the room, or another server, counts anew).
   */
  away(): void {
    const { awareness } = this;
    const others = [...awareness.meta.keys()].filter((client) => client !== awareness.clientID);
    removeAwarenessStates(awareness, others, this);
    for (const client of others) awareness.meta.delete(client);
    this.#shown = false;
  }

  destroy(): void {
    this.#gone = true;
    clearTimeout(this.#timer);
    this.#timer = undefined;
    this.awareness.off('update', this.#changed);
    this.#page?.removeEventListener('visibilitychange', this.#seen);
    this.#page = undefined;
    this.awareness.destroy();
  }
}
