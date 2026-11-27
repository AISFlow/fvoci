/**
 * Whether a WebSocket still carries (`contracts/websocket.md` 3): a page
 * cannot see the websocket's own Ping and Pong, so it sends a `PING` of
 * its own a while after it last heard the server, and takes the connection
 * for dead when nothing at all comes back in time. A connection that died
 * without a word (the network changed, the lid was closed) says nothing for
 * minutes otherwise, and the server's changes stop meanwhile.
 *
 * A page in the background has its timers slowed (Chrome lets them run
 * once a minute at most) or stopped, and a long task holds them back too. A
 * wait that ends later than it was set for says the page was busy or
 * asleep, not that the server is gone: the `PING` is sent again, and only a
 * second wait without an answer ends the connection.
 *
 * An answer comes only once a whole message has: on a slow link a large one
 * (a body's whole document) can take longer than the wait. So the wait of
 * the connections of one page doubles each time one was let go unanswered,
 * up to a minute (the server closes a connection it heard nothing from for
 * ninety seconds), and is the first again once one is answered.
 */

/** How long after the connection opened, or the last answer, the next `PING` goes, in milliseconds. */
export const PING_EVERY = 20_000;
/** How long a `PING` first waits for anything from the server, in milliseconds. */
export const PONG_WAIT = 15_000;
/**
 * The longest a `PING` waits, after connections were let go unanswered, in
 * milliseconds: with the twenty seconds before it, within the ninety the
 * server waits for anything from the page.
 */
export const LONGEST_PONG_WAIT = 60_000;
/** A wait that ends more than this late was a page held back, in milliseconds. */
const LATE = 1000;

export const PING = JSON.stringify({ type: 'ping' });
/** The server's answer, as it sends it. */
export const PONG = JSON.stringify({ type: 'pong' });

/** How long the `PING`s of one page's connections wait: longer after one was let go unanswered. */
export class Patience {
  #wait = PONG_WAIT;

  get wait(): number {
    return this.#wait;
  }

  /** A connection was let go unanswered. */
  ranOut(): void {
    this.#wait = Math.min(this.#wait * 2, LONGEST_PONG_WAIT);
  }

  /** A `PING` was answered. */
  answered(): void {
    this.#wait = PONG_WAIT;
  }
}

/** The `PING`s of one connection, from when it opened until it is let go. */
export class Heartbeat {
  readonly #ping: () => void;
  readonly #dead: () => void;
  readonly #patience: Patience;
  #timer: ReturnType<typeof setTimeout> | undefined;
  /** When the `PING` that waits for an answer went, and how long it waits; `undefined` when none waits. */
  #waiting: { sentAt: number; wait: number } | undefined;
  /** The `PING` that waits was sent again after a wait that ended late. */
  #again = false;
  #stopped = false;

  /**
   * `ping` sends a `PING`; `dead` is told once, when nothing answered in
   * time. `patience` is the page's, shared by its connections.
   */
  constructor(ping: () => void, dead: () => void, patience: Patience) {
    this.#ping = ping;
    this.#dead = dead;
    this.#patience = patience;
    this.#next();
  }

  /** Something came from the server: the connection carries. */
  heard(): void {
    if (this.#stopped || this.#waiting === undefined) return;
    this.#waiting = undefined;
    this.#again = false;
    this.#patience.answered();
    this.#next();
  }

  /** The connection is let go: nothing more is sent or told. */
  stop(): void {
    this.#stopped = true;
    clearTimeout(this.#timer);
    this.#timer = undefined;
  }

  #next(): void {
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => {
      this.#send();
    }, PING_EVERY);
  }

  #send(): void {
    const waiting = { sentAt: Date.now(), wait: this.#patience.wait };
    this.#waiting = waiting;
    this.#ping();
    this.#timer = setTimeout(() => {
      this.#waited();
    }, waiting.wait);
  }

  #waited(): void {
    const waiting = this.#waiting;
    if (this.#stopped || waiting === undefined) return;
    const late = Date.now() - waiting.sentAt - waiting.wait > LATE;
    if (late && !this.#again) {
      this.#again = true;
      this.#send();
      return;
    }
    this.stop();
    this.#patience.ranOut();
    this.#dead();
  }
}
