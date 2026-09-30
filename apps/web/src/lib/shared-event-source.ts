/**
 * One EventSource per URL per tab, shared by reference-counted leases.
 *
 * The browser retries by itself after a network error or when a 200 stream
 * ends (`error` with readyState CONNECTING). Any other response (429 at the
 * server's stream cap, a proxy's 502 while the server restarts, 401/404) fails
 * the connection for good (readyState CLOSED). The pool then opens a new
 * EventSource after a jittered exponential backoff and moves every listener to
 * it, so subscribers see the new source's `open` and can resync. The reopen
 * stops when the last lease closes.
 *
 * Transport errors invoke `onError` only; callers must not treat them as
 * access revocation.
 */
export type SharedEventSourceHandlers = {
  onOpen?: () => void;
  onError?: (event: Event) => void;
};

export type SharedEventSource = {
  addEventListener(type: string, listener: EventListener): void;
  removeEventListener(type: string, listener: EventListener): void;
  /** The pooled source's state: CLOSED from a refused connection until its reopen. */
  readonly readyState: number;
  close(): void;
};

/** Reopen n waits a random delay in [ceiling / 2, ceiling], ceiling = min(base * 2^n, max). */
export const REOPEN_BACKOFF = { baseMs: 1_000, maxMs: 30_000 } as const;

type PoolEntry = {
  source: EventSource;
  refs: number;
  /** Every listener of every lease, re-added to each reopened source. */
  listeners: Map<string, Set<EventListener>>;
  attempt: number;
  timer: ReturnType<typeof setTimeout> | undefined;
};

const poolByUrl = new Map<string, PoolEntry>();

function reopenDelayMs(attempt: number): number {
  const ceiling = Math.min(REOPEN_BACKOFF.maxMs, REOPEN_BACKOFF.baseMs * 2 ** attempt);
  return ceiling / 2 + Math.random() * (ceiling / 2);
}

function newSource(url: string): EventSource {
  return new EventSource(url, { withCredentials: true });
}

/** Wires the entry's current source: the reopen first, then every lease listener. */
function watch(url: string, entry: PoolEntry): void {
  const source = entry.source;
  source.addEventListener("open", () => {
    if (entry.source === source) entry.attempt = 0;
  });
  source.addEventListener("error", () => {
    if (entry.source !== source || source.readyState !== EventSource.CLOSED) return;
    if (entry.timer !== undefined) return;
    const delay = reopenDelayMs(entry.attempt);
    entry.attempt += 1;
    entry.timer = setTimeout(() => {
      entry.timer = undefined;
      if (poolByUrl.get(url) !== entry) return;
      entry.source = newSource(url);
      watch(url, entry);
    }, delay);
  });
  for (const [type, listeners] of entry.listeners) {
    for (const listener of listeners) source.addEventListener(type, listener);
  }
}

function addListener(entry: PoolEntry, type: string, listener: EventListener): void {
  let listeners = entry.listeners.get(type);
  if (!listeners) {
    listeners = new Set();
    entry.listeners.set(type, listeners);
  }
  listeners.add(listener);
  entry.source.addEventListener(type, listener);
}

function removeListener(entry: PoolEntry, type: string, listener: EventListener): void {
  entry.listeners.get(type)?.delete(listener);
  entry.source.removeEventListener(type, listener);
}

function dispose(entry: PoolEntry): void {
  clearTimeout(entry.timer);
  entry.timer = undefined;
  entry.source.close();
}

export function openSharedEventSource(
  url: string,
  handlers: SharedEventSourceHandlers,
): SharedEventSource {
  let entry = poolByUrl.get(url);
  if (!entry) {
    entry = { source: newSource(url), refs: 0, listeners: new Map(), attempt: 0, timer: undefined };
    poolByUrl.set(url, entry);
    watch(url, entry);
  }
  const capturedEntry = entry;
  capturedEntry.refs += 1;

  const { onOpen, onError } = handlers;
  if (onOpen) {
    addListener(capturedEntry, "open", onOpen);
  }
  if (onError) {
    addListener(capturedEntry, "error", onError);
  }

  let closed = false;

  return {
    addEventListener: (type, listener) => {
      addListener(capturedEntry, type, listener);
    },
    removeEventListener: (type, listener) => {
      removeListener(capturedEntry, type, listener);
    },
    get readyState() {
      return capturedEntry.source.readyState;
    },
    close: () => {
      if (closed) {
        return;
      }
      closed = true;
      if (poolByUrl.get(url) !== capturedEntry) {
        return;
      }
      if (onOpen) {
        removeListener(capturedEntry, "open", onOpen);
      }
      if (onError) {
        removeListener(capturedEntry, "error", onError);
      }
      capturedEntry.refs -= 1;
      if (capturedEntry.refs <= 0) {
        dispose(capturedEntry);
        poolByUrl.delete(url);
      }
    },
  };
}

/** @internal test-only */
export function sharedEventSourceRefCount(url: string): number {
  return poolByUrl.get(url)?.refs ?? 0;
}

/** @internal test-only */
export function resetSharedEventSourcePoolForTests(): void {
  for (const entry of poolByUrl.values()) {
    dispose(entry);
  }
  poolByUrl.clear();
}
