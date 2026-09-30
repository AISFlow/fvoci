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
  first.open();
  assert.equal(resyncs, 1);

  // e.g. 429 at the stream cap, or a proxy's 502 while the server restarts.
  first.fail(MockEventSource.CLOSED);
  mock.timers.tick(1_000);
  const reopened = MockEventSource.latest();
  assert.notEqual(reopened, first);
  reopened.open();
  assert.equal(resyncs, 2, "hints missed while refused are recovered by a resync");
  reopened.emit(
    new MessageEvent("task", { data: JSON.stringify({ verb: "task.updated", taskId: "t1" }) }),
  );
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
