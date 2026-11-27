/** Visits shared by instances of one world and, like the document rooms, browser tabs. */
import {
  EMPTY_PRESENCE,
  type Presence,
  type PresenceSnapshot,
  type PresenceVisit,
} from '../presence.ts';
import { reads } from './container-access.ts';
import type { World } from './world.ts';

interface Visit {
  id: string;
  userId: string;
  place: PresenceVisit;
  seen: number;
}
interface Watcher {
  containerId: string;
  hear: (snapshot: PresenceSnapshot) => void;
  before: string;
}
const worlds = new WeakMap<World, Set<MemoryPresence>>();

export class MemoryPresence implements Presence {
  readonly #world: World;
  readonly #userId: string;
  readonly #id = crypto.randomUUID();
  readonly #watchers = new Set<Watcher>();
  readonly #remote = new Map<string, Visit>();
  #visit: PresenceVisit | null = null;
  #channel: BroadcastChannel | undefined;
  #tick: ReturnType<typeof setInterval> | undefined;
  #renewed = 0;

  constructor(world: World, userId: string) {
    this.#world = world;
    this.#userId = userId;
  }

  setPresence(visit: PresenceVisit | null): void {
    this.#visit = visit && this.#valid(visit, this.#userId) ? { ...visit } : null;
    this.#start();
    this.#publish();
    this.#stopIfUnused();
  }

  watchPresence(containerId: string, hear: Watcher['hear']): () => void {
    const watcher = { containerId, hear, before: '' };
    this.#watchers.add(watcher);
    this.#start();
    return () => {
      this.#watchers.delete(watcher);
      this.#stopIfUnused();
    };
  }

  #valid(place: PresenceVisit, userId: string): boolean {
    const board = this.#world.containers.get(place.containerId);
    if (board?.workspaceId !== place.workspaceId) return false;
    if (!reads(this.#world, userId, board.id)) return false;
    if (!place.cardId) return true;
    const card = this.#world.blocks.get(place.cardId);
    // A wiki's page is visited as a card is.
    return card?.containerId === board.id && (card.type === 'card' || card.type === 'page');
  }

  #start(): void {
    if (this.#tick !== undefined || (!this.#visit && this.#watchers.size === 0)) return;
    let local = worlds.get(this.#world);
    if (!local) {
      local = new Set();
      worlds.set(this.#world, local);
    }
    local.add(this);
    if (this.#world.shareRooms && typeof BroadcastChannel !== 'undefined') {
      this.#channel = new BroadcastChannel(`${this.#world.shareRooms}:presence`);
      this.#channel.onmessage = (event: MessageEvent<unknown>) => {
        this.#receive(event.data);
      };
      this.#channel.postMessage({ hello: true });
    }
    this.#tick = setInterval(() => {
      this.#flush();
    }, 250);
  }

  #publish(): void {
    this.#renewed = Date.now();
    this.#channel?.postMessage({
      id: this.#id,
      userId: this.#userId,
      place: this.#visit,
    });
  }

  #receive(data: unknown): void {
    if (typeof data !== 'object' || data === null) return;
    const message = data as Record<string, unknown>;
    if (message.hello === true) {
      this.#publish();
      return;
    }
    if (typeof message.id !== 'string' || typeof message.userId !== 'string') return;
    if (message.place === null) {
      this.#remote.delete(message.id);
      return;
    }
    if (typeof message.place !== 'object') return;
    const place = message.place as PresenceVisit;
    if (
      typeof place.containerId !== 'string' ||
      typeof place.workspaceId !== 'string' ||
      (place.cardId !== undefined && typeof place.cardId !== 'string')
    )
      return;
    this.#remote.set(message.id, {
      id: message.id,
      userId: message.userId,
      place,
      seen: Date.now(),
    });
  }

  #flush(): void {
    if (this.#visit && !this.#valid(this.#visit, this.#userId)) {
      this.#visit = null;
      this.#publish();
    }
    if (Date.now() - this.#renewed >= 30_000) this.#publish();
    for (const [id, visit] of this.#remote) {
      if (Date.now() - visit.seen >= 90_000) this.#remote.delete(id);
    }
    this.#stopIfUnused();
    for (const watcher of this.#watchers) {
      const snapshot = this.#snapshot(watcher.containerId);
      const next = JSON.stringify(snapshot);
      if (next !== watcher.before) {
        watcher.before = next;
        watcher.hear(snapshot);
      }
    }
  }

  #snapshot(containerId: string): PresenceSnapshot {
    if (!this.#visit || this.#visit.containerId !== containerId) return EMPTY_PRESENCE;
    const visits = new Map(this.#remote);
    for (const peer of worlds.get(this.#world) ?? []) {
      if (peer.#visit)
        visits.set(peer.#id, {
          id: peer.#id,
          userId: peer.#userId,
          place: peer.#visit,
          seen: Date.now(),
        });
    }
    const people = new Map<string, Set<string>>();
    for (const visit of visits.values()) {
      if (visit.place.containerId !== containerId || !this.#valid(visit.place, visit.userId))
        continue;
      let cards = people.get(visit.userId);
      if (!cards) {
        cards = new Set();
        people.set(visit.userId, cards);
      }
      if (visit.place.cardId) cards.add(visit.place.cardId);
    }
    return {
      people: [...people]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([userId, cards]) => ({ userId, cardIds: [...cards].sort() })),
      anonymousCount: 0,
      hiddenCount: 0,
    };
  }

  #stopIfUnused(): void {
    if (this.#visit || this.#watchers.size > 0) return;
    clearInterval(this.#tick);
    this.#tick = undefined;
    worlds.get(this.#world)?.delete(this);
    this.#channel?.close();
    this.#channel = undefined;
    this.#remote.clear();
  }
}
