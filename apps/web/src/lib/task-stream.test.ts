import assert from "node:assert/strict";
import test, { mock } from "node:test";
import { installMockEventSource, MockEventSource } from "../../test/mock-event-source.ts";
import { resetSharedEventSourcePoolForTests } from "./shared-event-source.ts";
import { subscribeTaskStream, type TaskStreamHint } from "./task-stream.ts";

test.beforeEach(() => {
  installMockEventSource();
  resetSharedEventSourcePoolForTests();
  mock.method(Math, "random", () => 1);
  mock.timers.enable({ apis: ["setTimeout"] });
});

test.afterEach(() => {
  resetSharedEventSourcePoolForTests();
  mock.timers.reset();
  mock.restoreAll();
});

test("a reopened task stream resyncs on its open and keeps delivering hints", () => {
  let resyncs = 0;
  const hints: TaskStreamHint[] = [];
  const sub = subscribeTaskStream("ws", "project", {
    onResync: () => {
      resyncs += 1;
    },
    onTask: (hint) => hints.push(hint),
  });
  const first = MockEventSource.latest();
  assert.equal(first.url, "/api/v1/workspaces/ws/projects/project/stream");
  first.connect();
  assert.equal(resyncs, 1);

  // e.g. 429 at the stream cap, or a proxy's 502 while the server restarts.
  first.fail(MockEventSource.CLOSED);
  mock.timers.tick(1_000);
  const reopened = MockEventSource.latest();
  assert.notEqual(reopened, first);
  reopened.connect();
  assert.equal(resyncs, 2, "hints missed while refused are recovered by a resync");
  reopened.message("task", JSON.stringify({ verb: "task.updated", taskId: "t1" }));
  assert.deepEqual(hints, [{ verb: "task.updated", taskId: "t1" }]);

  sub.close();
  assert.equal(reopened.closed, true);
});

test("closing the subscription while refused stops the reopen", () => {
  const sub = subscribeTaskStream("ws", "project", { onResync: () => {}, onTask: () => {} });
  MockEventSource.latest().fail(MockEventSource.CLOSED);
  sub.close();
  mock.timers.tick(60_000);
  assert.equal(MockEventSource.instances.length, 1);
});

// SA-12: the browser dispatches both its native `open` (the 200 response) and
// the server's `event: open` item to the same "open" listeners.
test("a connect resyncs once, on the server's open", () => {
  let resyncs = 0;
  const sub = subscribeTaskStream("ws", "project", {
    onResync: () => {
      resyncs += 1;
    },
    onTask: () => {},
  });
  const source = MockEventSource.latest();
  source.open();
  assert.equal(resyncs, 0, "the 200 response alone is not the server's open");
  source.message("open", "{}");
  assert.equal(resyncs, 1);

  // The browser's own retry after a 200 stream ended is a new connect.
  source.fail(MockEventSource.CONNECTING);
  source.connect();
  assert.equal(resyncs, 2);
  sub.close();
});

test("the task stream listens only for events the server sends", () => {
  const sub = subscribeTaskStream("ws", "project", { onResync: () => {}, onTask: () => {} });
  const source = MockEventSource.latest();
  // src/http/routes/streams.rs queue_item_to_event: `open` and `task` only;
  // `error` is the pool's own reopen listener.
  const types = [...source.listeners].filter(([, set]) => set.size > 0).map(([type]) => type);
  assert.deepEqual(types.sort(), ["error", "open", "task"]);
  sub.close();
});
