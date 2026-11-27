/** Presence on the page's existing socket; reconnects renew the latest visit. */
import {
  EMPTY_PRESENCE,
  type Presence,
  type PresenceSnapshot,
  type PresenceVisit,
  type PresentPerson,
} from '../state/presence.ts';

interface Watcher {
  containerId: string;
  hear: (snapshot: PresenceSnapshot) => void;
}

export class LivePresence implements Presence {
  readonly #send: (command: object) => void;
  readonly #wanted: () => void;
  readonly #watchers = new Set<Watcher>();
  #visit: PresenceVisit | null = null;
  #up = false;
  #heartbeat: ReturnType<typeof setInterval> | undefined;

  constructor(send: (command: object) => void, wanted: () => void) {
    this.#send = send;
    this.#wanted = wanted;
  }

  get wanted(): boolean {
    return this.#visit !== null || this.#watchers.size > 0;
  }

  setPresence(visit: PresenceVisit | null): void {
    const same = sameVisit(this.#visit, visit);
    this.#visit = visit;
    // Off the board, its roster is no longer told: what was heard is not kept.
    if (!visit) for (const watcher of this.#watchers) watcher.hear(EMPTY_PRESENCE);
    if (!same) {
      this.#renew();
      this.#keepHeartbeat();
    }
    this.#wanted();
  }

  watchPresence(containerId: string, hear: Watcher['hear']): () => void {
    const watcher = { containerId, hear };
    this.#watchers.add(watcher);
    this.#wanted();
    return () => {
      this.#watchers.delete(watcher);
      this.#wanted();
    };
  }

  up(): void {
    this.#up = true;
    if (this.#visit) this.#renew();
    this.#keepHeartbeat();
  }

  #keepHeartbeat(): void {
    clearInterval(this.#heartbeat);
    this.#heartbeat =
      this.#up && this.#visit
        ? setInterval(() => {
            this.#renew();
          }, 30_000)
        : undefined;
  }

  down(): void {
    this.#up = false;
    clearInterval(this.#heartbeat);
    this.#heartbeat = undefined;
    for (const watcher of this.#watchers) watcher.hear(EMPTY_PRESENCE);
  }

  #renew(): void {
    if (!this.#up) return;
    this.#send({ type: 'setPresence', containerId: '', ...this.#visit });
  }

  text(message: Record<string, unknown>): boolean {
    if (message.type !== 'presenceChanged') return false;
    const { containerId, people, anonymousCount, hiddenCount } = message;
    if (
      typeof containerId !== 'string' ||
      !Array.isArray(people) ||
      !people.every(isPerson) ||
      !isCount(anonymousCount) ||
      !isCount(hiddenCount)
    )
      return true;
    if (containerId !== this.#visit?.containerId) return true;
    for (const watcher of this.#watchers) {
      if (watcher.containerId === containerId)
        watcher.hear({ people, anonymousCount, hiddenCount });
    }
    return true;
  }
}

/** Whether two visits are the same place; the server is asked only for a new one. */
function sameVisit(a: PresenceVisit | null, b: PresenceVisit | null): boolean {
  if (!a || !b) return a === b;
  return (
    a.workspaceId === b.workspaceId &&
    a.containerId === b.containerId &&
    (a.cardId ?? '') === (b.cardId ?? '')
  );
}

function isCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}

function isPerson(value: unknown): value is PresentPerson {
  if (typeof value !== 'object' || value === null) return false;
  const person = value as Record<string, unknown>;
  return (
    typeof person.userId === 'string' &&
    Array.isArray(person.cardIds) &&
    person.cardIds.every((id: unknown) => typeof id === 'string')
  );
}
