import assert from "node:assert/strict";
import test, { mock } from "node:test";
import { installMockEventSource, MockEventSource } from "../../test/mock-event-source.ts";
import { installNodeRelativeRequestShim } from "../../test/node-api-fetch.ts";
import {
  resetSharedEventSourcePoolForTests,
  sharedEventSourceRefCount,
} from "./shared-event-source.ts";

const WS = "11111111-1111-7111-8111-111111111111";
const STREAM = `/api/v1/workspaces/${WS}/access-stream`;

// The API client takes `Request` when its module loads, so shim it first.
const restoreNodeRequest = installNodeRelativeRequestShim();
const { watchWorkspaceAccess } = await import("./workspace-access-stream.ts");
const originalFetch = globalThis.fetch;
let probes: string[] = [];

/** The workspace endpoint the watcher probes answers `status`. */
function serveWorkspace(status: number) {
  globalThis.fetch = async (input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(input);
    probes.push(`${request.method} ${new URL(request.url).pathname}`);
    const body =
      status === 200
        ? { id: WS, slug: "acme", name: "Acme" }
        : { type: "about:blank", title: "x", status, code: status === 401 ? "authentication_required" : "not_found" };
    return new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    });
  };
}

/** Let the probe's fetch and its continuation run. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

test.beforeEach(() => {
  installMockEventSource();
  resetSharedEventSourcePoolForTests();
  probes = [];
  mock.method(Math, "random", () => 1);
  mock.timers.enable({ apis: ["setTimeout"] });
});

test.afterEach(() => {
  resetSharedEventSourcePoolForTests();
  mock.timers.reset();
  mock.restoreAll();
  globalThis.fetch = originalFetch;
});

test.after(() => {
  restoreNodeRequest();
});

test("an ended stream asks for a reconcile and leaves the reconnect to the browser", async () => {
  serveWorkspace(200);
  let changes = 0;
  watchWorkspaceAccess(WS, { onAccessChange: () => (changes += 1) });
  const source = MockEventSource.latest();
  assert.equal(source.url, STREAM);
  source.open();
  // The server ended the 200 stream (membership, role or session changed).
  source.fail(MockEventSource.CONNECTING);
  await settle();
  mock.timers.tick(60_000);
  assert.equal(changes, 1);
  assert.deepEqual(probes, []);
  assert.equal(MockEventSource.instances.length, 1);
});

for (const status of [401, 404]) {
  test(`a refused stream whose access is gone (${status}) stops and reconciles once`, async () => {
    serveWorkspace(status);
    let changes = 0;
    const sub = watchWorkspaceAccess(WS, { onAccessChange: () => (changes += 1) });
    const source = MockEventSource.latest();
    source.fail(MockEventSource.CLOSED);
    await settle();
    assert.deepEqual(probes, [`GET /api/v1/workspaces/${WS}`]);
    assert.equal(changes, 1);
    assert.equal(sharedEventSourceRefCount(STREAM), 0, "the watcher released its stream");
    assert.equal(source.closed, true);
    mock.timers.tick(60_000);
    assert.equal(MockEventSource.instances.length, 1, "no reopen after the access is gone");
    sub.close();
    assert.equal(changes, 1);
  });
}

test("a refused stream of a current member reopens without refetching the list", async () => {
  serveWorkspace(200);
  let changes = 0;
  const sub = watchWorkspaceAccess(WS, { onAccessChange: () => (changes += 1) });
  // e.g. 429 at the stream cap, or a proxy's 502 while the server restarts.
  MockEventSource.latest().fail(MockEventSource.CLOSED);
  await settle();
  assert.deepEqual(probes, [`GET /api/v1/workspaces/${WS}`]);
  assert.equal(changes, 0);
  mock.timers.tick(1_000);
  assert.equal(MockEventSource.instances.length, 2, "reopened after the backoff");
  assert.equal(MockEventSource.latest().url, STREAM);
  sub.close();
  assert.equal(MockEventSource.latest().closed, true);
});

test("a probe that fails on the network keeps reopening", async () => {
  globalThis.fetch = async () => {
    throw new TypeError("network down");
  };
  let changes = 0;
  watchWorkspaceAccess(WS, { onAccessChange: () => (changes += 1) });
  MockEventSource.latest().fail(MockEventSource.CLOSED);
  await settle();
  assert.equal(changes, 0);
  mock.timers.tick(1_000);
  assert.equal(MockEventSource.instances.length, 2);
});

test("closing the watcher before the probe answers skips the reconcile", async () => {
  serveWorkspace(404);
  let changes = 0;
  const sub = watchWorkspaceAccess(WS, { onAccessChange: () => (changes += 1) });
  MockEventSource.latest().fail(MockEventSource.CLOSED);
  sub.close();
  await settle();
  assert.equal(changes, 0);
});
