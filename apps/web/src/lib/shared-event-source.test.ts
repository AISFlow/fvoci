import assert from "node:assert/strict";
import test, { mock } from "node:test";
import { installMockEventSource, MockEventSource } from "../../test/mock-event-source.ts";
import {
  openSharedEventSource,
  resetSharedEventSourcePoolForTests,
  sharedEventSourceRefCount,
} from "./shared-event-source.ts";

const latest = () => MockEventSource.latest();

test.beforeEach(() => {
  installMockEventSource();
  resetSharedEventSourcePoolForTests();
  // The longest delay of each backoff step, so ticks are exact.
  mock.method(Math, "random", () => 1);
  mock.timers.enable({ apis: ["setTimeout"] });
});

test.afterEach(() => {
  resetSharedEventSourcePoolForTests();
  mock.timers.reset();
  mock.restoreAll();
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

test("a refused connection is reopened after a jittered backoff", () => {
  const url = "/api/v1/stream";
  openSharedEventSource(url, {});
  latest().fail(MockEventSource.CLOSED);
  mock.timers.tick(999);
  assert.equal(MockEventSource.instances.length, 1, "not before the first delay");
  mock.timers.tick(1);
  assert.equal(MockEventSource.instances.length, 2);
  assert.equal(latest().url, url);
  assert.deepEqual(latest().options, { withCredentials: true });
  assert.equal(sharedEventSourceRefCount(url), 1);
});

test("the first reopen waits at least half the base delay", () => {
  mock.method(Math, "random", () => 0);
  openSharedEventSource("/api/v1/stream", {});
  latest().fail(MockEventSource.CLOSED);
  mock.timers.tick(499);
  assert.equal(MockEventSource.instances.length, 1);
  mock.timers.tick(1);
  assert.equal(MockEventSource.instances.length, 2);
});

test("listeners move to the reopened source", () => {
  const url = "/api/v1/stream";
  const opened: string[] = [];
  const hints: string[] = [];
  const lease = openSharedEventSource(url, { onOpen: () => opened.push("handler") });
  lease.addEventListener("open", () => opened.push("lease"));
  lease.addEventListener("task", (event) => hints.push((event as MessageEvent<string>).data));
  latest().fail(MockEventSource.CLOSED);
  mock.timers.tick(1_000);

  const reopened = latest();
  assert.equal(MockEventSource.instances.length, 2);
  reopened.open();
  reopened.emit(new MessageEvent("task", { data: "hint" }));
  assert.deepEqual(opened, ["handler", "lease"]);
  assert.deepEqual(hints, ["hint"]);

  const removed = () => hints.push("removed");
  lease.addEventListener("task", removed);
  lease.removeEventListener("task", removed);
  reopened.emit(new MessageEvent("task", { data: "again" }));
  assert.deepEqual(hints, ["hint", "again"]);
});

test("the backoff doubles to a 30 s cap and resets once a source opens", () => {
  openSharedEventSource("/api/v1/stream", {});
  const delays = [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000];
  for (const [index, delay] of delays.entries()) {
    latest().fail(MockEventSource.CLOSED);
    mock.timers.tick(delay - 1);
    assert.equal(MockEventSource.instances.length, index + 1, `attempt ${index} waits ${delay} ms`);
    mock.timers.tick(1);
    assert.equal(MockEventSource.instances.length, index + 2);
  }
  latest().open();
  latest().fail(MockEventSource.CLOSED);
  mock.timers.tick(1_000);
  assert.equal(MockEventSource.instances.length, delays.length + 2, "open resets the backoff");
});

test("closing the last lease during the backoff cancels the reopen", () => {
  const url = "/api/v1/stream";
  const lease = openSharedEventSource(url, {});
  latest().fail(MockEventSource.CLOSED);
  lease.close();
  mock.timers.tick(60_000);
  assert.equal(MockEventSource.instances.length, 1);
  assert.equal(sharedEventSourceRefCount(url), 0);
});

test("a lease that stays open keeps the reopen for the others", () => {
  const url = "/api/v1/stream";
  const first = openSharedEventSource(url, {});
  const hints: string[] = [];
  const second = openSharedEventSource(url, {});
  second.addEventListener("task", (event) => hints.push((event as MessageEvent<string>).data));
  latest().fail(MockEventSource.CLOSED);
  first.close();
  mock.timers.tick(1_000);
  assert.equal(MockEventSource.instances.length, 2, "one reopen for the shared source");
  latest().emit(new MessageEvent("task", { data: "hint" }));
  assert.deepEqual(hints, ["hint"]);
  second.close();
  assert.equal(latest().closed, true);
});

test("an error while the browser retries by itself does not reopen", () => {
  openSharedEventSource("/api/v1/stream", {});
  latest().fail(MockEventSource.CONNECTING);
  mock.timers.tick(60_000);
  assert.equal(MockEventSource.instances.length, 1);
});

test("the lease reports the pooled source's state", () => {
  const lease = openSharedEventSource("/api/v1/stream", {});
  assert.equal(lease.readyState, MockEventSource.CONNECTING);
  latest().open();
  assert.equal(lease.readyState, MockEventSource.OPEN);
  latest().fail(MockEventSource.CLOSED);
  assert.equal(lease.readyState, MockEventSource.CLOSED);
  mock.timers.tick(1_000);
  assert.equal(lease.readyState, MockEventSource.CONNECTING);
});

test("resetting the pool cancels a pending reopen", () => {
  openSharedEventSource("/api/v1/stream", {});
  latest().fail(MockEventSource.CLOSED);
  resetSharedEventSourcePoolForTests();
  mock.timers.tick(60_000);
  assert.equal(MockEventSource.instances.length, 1);
});
