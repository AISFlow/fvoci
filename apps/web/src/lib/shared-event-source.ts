/**
 * Shared EventSource with credentials and explicit reconnect (source shared-event-source.ts).
 * Transport errors invoke `onError` only; callers must not treat them as access revocation.
 */
export type SharedEventSourceHandlers = {
  onOpen?: () => void;
  onError?: (event: Event) => void;
};

export type SharedEventSource = {
  addEventListener(type: string, listener: EventListener): void;
  removeEventListener(type: string, listener: EventListener): void;
  close(): void;
};

type PoolEntry = {
  source: EventSource;
  refs: number;
};

const poolByUrl = new Map<string, PoolEntry>();

export function openSharedEventSource(
  url: string,
  handlers: SharedEventSourceHandlers,
): SharedEventSource {
  let entry = poolByUrl.get(url);
  if (!entry) {
    const source = new EventSource(url, { withCredentials: true });
    entry = { source, refs: 0 };
    poolByUrl.set(url, entry);
  }
  const capturedEntry = entry;
  capturedEntry.refs += 1;

  const { onOpen, onError } = handlers;
  if (onOpen) {
    capturedEntry.source.addEventListener("open", onOpen);
  }
  if (onError) {
    capturedEntry.source.addEventListener("error", onError);
  }

  const source = capturedEntry.source;
  let closed = false;

  return {
    addEventListener: (type, listener) => {
      source.addEventListener(type, listener);
    },
    removeEventListener: (type, listener) => {
      source.removeEventListener(type, listener);
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
        capturedEntry.source.removeEventListener("open", onOpen);
      }
      if (onError) {
        capturedEntry.source.removeEventListener("error", onError);
      }
      capturedEntry.refs -= 1;
      if (capturedEntry.refs <= 0) {
        capturedEntry.source.close();
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
    entry.source.close();
  }
  poolByUrl.clear();
}
