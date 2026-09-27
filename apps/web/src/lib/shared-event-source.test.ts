import assert from "node:assert/strict";
import test from "node:test";
import {
  openSharedEventSource,
  resetSharedEventSourcePoolForTests,
  sharedEventSourceRefCount,
} from "./shared-event-source.ts";

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  closed = false;
  listeners = new Map<string, Set<EventListener>>();

  constructor(url: string, _options?: EventSourceInit) {
    this.url = url;
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
  }
}

test.beforeEach(() => {
  MockEventSource.instances = [];
  resetSharedEventSourcePoolForTests();
  // @ts-expect-error test shim
  globalThis.EventSource = MockEventSource;
});

test("close is idempotent per lease", () => {
  const url = "/api/v1/stream";
  const lease = openSharedEventSource(url, {});
  assert.equal(sharedEventSourceRefCount(url), 1);
  lease.close();
  assert.equal(sharedEventSourceRefCount(url), 0);
  lease.close();
  assert.equal(sharedEventSourceRefCount(url), 0);
  assert.equal(MockEventSource.instances.length, 1);
  assert.equal(MockEventSource.instances[0]?.closed, true);
});

test("stale close does not affect a new pool entry", () => {
  const url = "/api/v1/stream";
  const first = openSharedEventSource(url, {});
  first.close();
  assert.equal(sharedEventSourceRefCount(url), 0);

  const second = openSharedEventSource(url, {});
  assert.equal(sharedEventSourceRefCount(url), 1);
  first.close();
  assert.equal(sharedEventSourceRefCount(url), 1);
  assert.equal(MockEventSource.instances[1]?.closed, false);

  second.close();
  assert.equal(sharedEventSourceRefCount(url), 0);
  assert.equal(MockEventSource.instances[1]?.closed, true);
});
