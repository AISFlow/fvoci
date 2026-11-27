/**
 * The server's changes as they happen (`contracts/websocket.md`): one
 * WebSocket per page, subscribed to the workspace the reader is in
 * (`enter`: the one the address names), its messages turned into `Change`
 * values and handed to listeners of that container or the reader's own state.
 *
 * Reconnection uses exponential backoff (`WAITS`, with jitter on `1013`),
 * resets after `STEADY`, and broadcasts `resync` so missed updates are re-read.
 * Binary Yjs room frames (`live-rooms.ts`) and presence (`live-presence.ts`)
 * share the same connection.
 */
import { isBackendError, type Change, type ChangeListener } from '../state/backend.ts';

import { Heartbeat, Patience, PING, PONG } from './live-heartbeat.ts';
import { dispatchLiveMessage, type LiveMessageSink, type LiveReads } from './live-messages.ts';
import { LivePresence } from './live-presence.ts';
import { LiveRooms } from './live-rooms.ts';

export type { LiveReads } from './live-messages.ts';

interface Listening {
  /** `undefined` for the reader's own things. */
  containerId: string | undefined;
  listener: ChangeListener;
}

/** The waits before opening again, in milliseconds, by how many tries failed; the last repeats. */
const WAITS = [500, 1000, 2000, 5000, 10_000, 30_000];
/**
 * The close the server sends when it could not read what a connection may be told
 * (`contracts/websocket.md` 3): every page it closed so comes back, so each waits
 * a random part of its wait more, that they do not come back all at once.
 */
const TRY_AGAIN = 1013;
/** How long a connection holds before the waits start again from the first. */
const STEADY = 10_000;
/** How long member and category messages are gathered before the sidebar is read again. */
const GATHER = 200;

export class LiveChanges {
  readonly #url: string;
  readonly #reads: LiveReads;
  readonly #listening = new Set<Listening>();
  #socket: WebSocket | undefined;
  /** Whether `#socket` still carries, once it opened. */
  #heartbeat: Heartbeat | undefined;
  /** How long the connections' `ping`s wait. */
  readonly #patience = new Patience();
  /** Whether `#socket` has opened and subscribed. */
  #subscribed = false;
  /** The workspace entered: the one subscribed to. */
  #workspaceId: string | undefined;
  /** Tries in a row that did not open: picks the wait. */
  #failures = 0;
  /** A connection was open before: the next one that opens is a reconnection. */
  #wasOpen = false;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #steady: ReturnType<typeof setTimeout> | undefined;
  #gathering: ReturnType<typeof setTimeout> | undefined;
  /** The latest reading of each board's members: an older answer is not told. */
  readonly #readings = new Map<string, number>();
  #readCount = 0;
  readonly #sink: LiveMessageSink;
  /** The rooms of the bodies open on this connection. */
  readonly rooms: LiveRooms;
  readonly presence: LivePresence;

  /** `url` is the address of `/ws`, with `ws:` or `wss:`. */
  constructor(url: string, reads: LiveReads) {
    this.#url = url;
    this.#reads = reads;
    this.#sink = {
      workspaceId: () => this.#workspaceId,
      reads,
      resubscribeWorkspace: (workspaceId) => {
        this.#send(JSON.stringify({ type: 'subscribeWorkspace', workspaceId }));
      },
      tellAll: (change) => {
        this.#tellAll(change);
      },
      tellSelf: (change) => {
        this.#tellSelf(change);
      },
      tellContainer: (containerId, change, self) => {
        this.#tellContainer(containerId, change, self);
      },
      sidebarSoon: () => {
        this.#sidebarSoon();
      },
      readMembers: (containerId) => this.#readMembers(containerId),
    };
    this.rooms = new LiveRooms(
      {
        text: (command) => this.#send(JSON.stringify(command)),
        bytes: (frame) => this.#send(frame),
      },
      () => {
        if (!this.#wanted()) this.#stop();
        else if (!this.#socket && this.#timer === undefined) this.#open();
      },
    );
    this.presence = new LivePresence(
      (command) => {
        this.#send(JSON.stringify(command));
      },
      () => {
        if (!this.#wanted()) this.#stop();
        else if (!this.#socket && this.#timer === undefined) this.#open();
      },
    );
    globalThis.addEventListener('online', () => {
      if (this.#wanted() && !this.#socket) this.#openNow();
    });
    globalThis.addEventListener('offline', () => {
      const socket = this.#socket;
      if (socket) this.#letGo(socket);
    });
  }

  /**
   * Opens the connection again at once, as somebody new: after signing in,
   * the cookie is another session's (an ended session's connection is
   * anonymous and hears nothing). The listeners then read again.
   */
  restart(): void {
    this.#failures = 0;
    clearTimeout(this.#steady);
    this.#stopHeartbeat();
    const socket = this.#socket;
    this.#socket = undefined;
    this.#subscribed = false;
    socket?.close();
    this.rooms.down();
    this.presence.down();
    if (!this.#wanted()) return;
    this.#wasOpen = true;
    this.#openNow();
  }

  #openNow(): void {
    clearTimeout(this.#timer);
    this.#timer = undefined;
    this.#open();
  }

  /**
   * Follows `workspaceId` from now on. An open connection leaves the one it
   * followed and subscribes to this one; what was heard of the old one is
   * the business of whoever listened to it (the workspace reads anew).
   */
  enter(workspaceId: string): void {
    const before = this.#workspaceId;
    if (before === workspaceId) {
      this.#send(JSON.stringify({ type: 'subscribeWorkspace', workspaceId }));
      this.#tellAll({ kind: 'resync' });
      return;
    }
    this.#workspaceId = workspaceId;
    const socket = this.#socket;
    if (socket && this.#subscribed) {
      if (before !== undefined) {
        socket.send(JSON.stringify({ type: 'unsubscribeWorkspace', workspaceId: before }));
      }
      socket.send(JSON.stringify({ type: 'subscribeWorkspace', workspaceId }));
    } else if (!socket && this.#wanted() && this.#timer === undefined) {
      this.#open();
    }
  }

  watch(containerId: string | undefined, listener: ChangeListener): () => void {
    const entry: Listening = { containerId, listener };
    this.#listening.add(entry);
    if (!this.#socket && this.#timer === undefined) this.#open();
    return () => {
      this.#listening.delete(entry);
      if (!this.#wanted()) this.#stop();
    };
  }

  /** Somebody listens, or a room is open: the connection is kept. */
  #wanted(): boolean {
    return this.#listening.size > 0 || this.rooms.size > 0 || this.presence.wanted;
  }

  /** Sends on the connection if it is open and subscribed; false when it is not. */
  #send(data: string | Uint8Array<ArrayBuffer>): boolean {
    const socket = this.#socket;
    if (!socket || !this.#subscribed) return false;
    socket.send(data);
    return true;
  }

  #stop(): void {
    clearTimeout(this.#timer);
    clearTimeout(this.#steady);
    clearTimeout(this.#gathering);
    this.#timer = undefined;
    this.#gathering = undefined;
    this.#stopHeartbeat();
    const socket = this.#socket;
    this.#socket = undefined;
    this.#subscribed = false;
    this.#wasOpen = false;
    this.#failures = 0;
    socket?.close();
    this.rooms.down();
    this.presence.down();
  }

  #open(): void {
    if (this.#workspaceId === undefined) return;
    if (!this.#wanted() || this.#socket) return;
    const socket = new WebSocket(this.#url);
    socket.binaryType = 'arraybuffer';
    this.#socket = socket;
    this.#subscribed = false;
    socket.addEventListener('open', () => {
      if (this.#socket !== socket) return;
      socket.send(JSON.stringify({ type: 'subscribeWorkspace', workspaceId: this.#workspaceId }));
      this.#subscribed = true;
      this.#heartbeat = new Heartbeat(
        () => {
          socket.send(PING);
        },
        () => {
          this.#letGo(socket);
        },
        this.#patience,
      );
      this.#steady = setTimeout(() => {
        this.#failures = 0;
      }, STEADY);
      if (this.#wasOpen) this.#tellAll({ kind: 'resync' });
      this.#wasOpen = true;
      this.rooms.up();
      this.presence.up();
    });
    socket.addEventListener('message', (event: MessageEvent<unknown>) => {
      if (this.#socket !== socket) return;
      this.#heartbeat?.heard();
      if (event.data instanceof ArrayBuffer) this.rooms.bytes(new Uint8Array(event.data));
      else if (typeof event.data === 'string' && event.data !== PONG)
        void this.#receive(event.data);
    });
    socket.addEventListener('close', (event: CloseEvent) => {
      this.#dropped(socket, event.code === TRY_AGAIN);
    });
  }

  #dropped(socket: WebSocket, spread = false): void {
    if (this.#socket !== socket) return;
    clearTimeout(this.#steady);
    this.#stopHeartbeat();
    this.#socket = undefined;
    this.#subscribed = false;
    this.#failures += 1;
    this.rooms.down();
    this.presence.down();
    this.#again(spread);
  }

  #letGo(socket: WebSocket): void {
    this.#dropped(socket);
    socket.close();
  }

  #stopHeartbeat(): void {
    this.#heartbeat?.stop();
    this.#heartbeat = undefined;
  }

  #again(spread = false): void {
    if (!this.#wanted() || this.#timer !== undefined) return;
    const base = WAITS[Math.min(this.#failures - 1, WAITS.length - 1)] ?? 30_000;
    const wait = spread ? base + Math.floor(Math.random() * base) : base;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      this.#open();
    }, wait);
  }

  #tellAll(change: Change): void {
    for (const { listener } of [...this.#listening]) listener(change);
  }

  #tellContainer(containerId: string, change: Change, self: boolean): void {
    for (const { containerId: watched, listener } of [...this.#listening]) {
      if (watched === containerId || (self && watched === undefined)) listener(change);
    }
  }

  #tellSelf(change: Change): void {
    for (const { containerId, listener } of [...this.#listening]) {
      if (containerId === undefined) listener(change);
    }
  }

  async #receive(text: string): Promise<void> {
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      return;
    }
    if (typeof parsed !== 'object' || parsed === null) return;
    const record = parsed as Record<string, unknown>;
    if (this.rooms.text(record)) return;
    if (this.presence.text(record)) return;
    await dispatchLiveMessage(record, this.#sink);
  }

  async #readMembers(containerId: string): Promise<void> {
    if (![...this.#listening].some((entry) => entry.containerId === containerId)) return;
    this.#readCount += 1;
    const reading = this.#readCount;
    this.#readings.set(containerId, reading);
    try {
      const members = await this.#reads.members(containerId);
      if (this.#readings.get(containerId) !== reading) return;
      this.#tellContainer(containerId, { kind: 'members', containerId, members }, false);
    } catch (error) {
      if (this.#readings.get(containerId) !== reading) return;
      if (isBackendError(error) && (error.status === 403 || error.status === 404)) {
        this.#tellContainer(containerId, { kind: 'containerDeleted', containerId }, false);
      }
    }
  }

  #sidebarSoon(): void {
    if (this.#gathering !== undefined) return;
    this.#gathering = setTimeout(() => {
      this.#gathering = undefined;
      this.#tellSelf({ kind: 'resync' });
    }, GATHER);
  }
}
