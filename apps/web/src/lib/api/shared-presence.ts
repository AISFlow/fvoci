/** A public-link visit announces only its token and never subscribes to names or changes. */
import type { Presence, PresenceVisit } from '../state/presence.ts';
import { Heartbeat, Patience, PING } from './live-heartbeat.ts';
import { LivePresence } from './live-presence.ts';

export class SharedPresence implements Presence {
  readonly #url: string;
  readonly #containerId: string;
  readonly #live: LivePresence;
  #socket: WebSocket | undefined;
  #heartbeat: Heartbeat | undefined;
  readonly #patience = new Patience();
  #retry: ReturnType<typeof setTimeout> | undefined;
  #wait = 500;

  constructor(basePath: string, containerId: string, readToken: string) {
    const url = new URL(`${basePath}/ws`, location.href);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    this.#url = url.href;
    this.#containerId = containerId;
    this.#live = new LivePresence(
      (command) => {
        this.#socket?.send(JSON.stringify({ ...command, workspaceId: '', readToken }));
      },
      () => {
        if (!this.#live.wanted) this.#stop();
        else if (!this.#socket && this.#retry === undefined) this.#open();
      },
    );
  }

  setPresence(visit: PresenceVisit | null): void {
    this.#live.setPresence(visit?.containerId === this.#containerId ? visit : null);
  }

  watchPresence: Presence['watchPresence'] = () => () => undefined;

  #open(): void {
    if (!this.#live.wanted || this.#socket) return;
    const socket = new WebSocket(this.#url);
    this.#socket = socket;
    socket.addEventListener('open', () => {
      if (this.#socket !== socket) return;
      // Not answered in time: gone, though its close may not be heard for long.
      this.#heartbeat = new Heartbeat(
        () => {
          socket.send(PING);
        },
        () => {
          this.#closed(socket);
          socket.close();
        },
        this.#patience,
      );
      this.#live.up();
    });
    socket.addEventListener('message', () => {
      if (this.#socket !== socket) return;
      // It carried: the waits before opening again start from the first.
      this.#wait = 500;
      this.#heartbeat?.heard();
    });
    socket.addEventListener('close', () => {
      this.#closed(socket);
    });
  }

  /** The connection `socket` is gone, if it was the one: it is opened again after a wait. */
  #closed(socket: WebSocket): void {
    if (this.#socket !== socket) return;
    this.#socket = undefined;
    this.#heartbeat?.stop();
    this.#heartbeat = undefined;
    this.#live.down();
    if (!this.#live.wanted) return;
    this.#retry = setTimeout(() => {
      this.#retry = undefined;
      this.#open();
    }, this.#wait);
    this.#wait = Math.min(this.#wait * 2, 30_000);
  }

  #stop(): void {
    clearTimeout(this.#retry);
    this.#retry = undefined;
    this.#wait = 500;
    this.#heartbeat?.stop();
    this.#heartbeat = undefined;
    const socket = this.#socket;
    this.#socket = undefined;
    this.#live.down();
    socket?.close();
  }
}
