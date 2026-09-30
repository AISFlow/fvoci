/** Browser `EventSource` stand-in for node unit tests of the shared stream pool. */
export class MockEventSource {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;
  static instances: MockEventSource[] = [];

  static latest(): MockEventSource {
    const source = MockEventSource.instances.at(-1);
    if (!source) throw new Error("no EventSource was opened");
    return source;
  }

  url: string;
  options?: EventSourceInit;
  closed = false;
  readyState = MockEventSource.CONNECTING;
  listeners = new Map<string, Set<EventListener>>();

  constructor(url: string, options?: EventSourceInit) {
    this.url = url;
    this.options = options;
    MockEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: EventListener) {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(listener);
  }

  removeEventListener(type: string, listener: EventListener) {
    this.listeners.get(type)?.delete(listener);
  }

  close() {
    this.closed = true;
    this.readyState = MockEventSource.CLOSED;
  }

  emit(event: Event) {
    for (const listener of [...(this.listeners.get(event.type) ?? [])]) {
      listener(event);
    }
  }

  /** The server answered 200 text/event-stream. */
  open() {
    this.readyState = MockEventSource.OPEN;
    this.emit(new Event("open"));
  }

  /**
   * A failed connection. CONNECTING: the browser retries by itself (network
   * error, or a 200 stream ended). CLOSED: any other response; no retry.
   */
  fail(readyState: number) {
    this.readyState = readyState;
    this.emit(new Event("error"));
  }
}

export function installMockEventSource(): void {
  MockEventSource.instances = [];
  // @ts-expect-error test shim
  globalThis.EventSource = MockEventSource;
}
