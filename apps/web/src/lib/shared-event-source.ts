/**
 * Shared EventSource with credentials and explicit reconnect (source shared-event-source.ts).
 * Transport errors invoke `onError` only; callers must not treat them as access revocation.
 */
export type SharedEventSourceHandlers = {
  onOpen?: () => void;
  onError?: (event: Event) => void;
};

export function openSharedEventSource(
  url: string,
  handlers: SharedEventSourceHandlers,
): EventSource {
  const source = new EventSource(url, { withCredentials: true });
  if (handlers.onOpen) {
    source.addEventListener("open", handlers.onOpen);
  }
  if (handlers.onError) {
    source.addEventListener("error", handlers.onError);
  }
  return source;
}
