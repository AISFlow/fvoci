import assert from "node:assert/strict";
import test, { mock } from "node:test";
import { installMockEventSource, MockEventSource } from "../../test/mock-event-source.ts";
import { resetSharedEventSourcePoolForTests } from "./shared-event-source.ts";
import {
  resetWorkspaceTaskStreamWatchersForTests,
  subscribeTaskStream,
  subscribeWorkspaceTaskStream,
  type TaskStreamHint,
  type WorkspaceTaskStreamHint,
} from "./task-stream.ts";

test.beforeEach(() => {
  installMockEventSource();
  resetSharedEventSourcePoolForTests();
  resetWorkspaceTaskStreamWatchersForTests();
  mock.method(Math, "random", () => 1);
  mock.timers.enable({ apis: ["setTimeout"] });
});

test.afterEach(() => {
  resetSharedEventSourcePoolForTests();
  resetWorkspaceTaskStreamWatchersForTests();
  mock.timers.reset();
  mock.restoreAll();
});

await test("a reopened task stream resyncs on its open and keeps delivering hints", () => {
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

await test("closing the subscription while refused stops the reopen", () => {
  const sub = subscribeTaskStream("ws", "project", { onResync: () => {}, onTask: () => {} });
  MockEventSource.latest().fail(MockEventSource.CLOSED);
  sub.close();
  mock.timers.tick(60_000);
  assert.equal(MockEventSource.instances.length, 1);
});

// SA-12: the browser dispatches both its native `open` (the 200 response) and
// the server's `event: open` item to the same "open" listeners.
await test("a connect resyncs once, on the server's open", () => {
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

await test("the task stream listens only for events the server sends", () => {
  const sub = subscribeTaskStream("ws", "project", { onResync: () => {}, onTask: () => {} });
  const source = MockEventSource.latest();
  // src/http/routes/streams.rs queue_item_to_event: `open` and `task` only;
  // `error` is the pool's own reopen listener.
  const types = [...source.listeners].filter(([, set]) => set.size > 0).map(([type]) => type);
  assert.deepEqual(types.sort(), ["error", "open", "task"]);
  sub.close();
});

await test("untrusted task hints require nonempty string identifiers and verbs", () => {
  const hints: TaskStreamHint[] = [];
  const sub = subscribeTaskStream("ws", "project", {
    onResync: () => {},
    onTask: (hint) => hints.push(hint),
  });
  const source = MockEventSource.latest();
  for (const body of [
    null,
    [],
    true,
    { taskId: 1, verb: "task.updated" },
    { taskId: "t1", verb: {} },
    { taskId: "", verb: "task.updated" },
    { taskId: "t1", verb: "" },
  ]) {
    source.message("task", JSON.stringify(body));
  }
  source.message("task", "{broken");
  assert.deepEqual(hints, []);
  // Unknown verbs are still hints: future server operations can request a GET.
  source.message("task", JSON.stringify({ taskId: "t1", verb: "task.future" }));
  source.message("task", JSON.stringify({ taskId: "t1", verb: "task.future" }));
  assert.deepEqual(hints, [
    { taskId: "t1", verb: "task.future" },
    { taskId: "t1", verb: "task.future" },
  ]);
  sub.close();
  source.message("task", JSON.stringify({ taskId: "t2", verb: "task.updated" }));
  assert.equal(hints.length, 2, "a closed scope cannot deliver a late hint");
});

await test("one workspace task stream: every connect resyncs the subscription's projects and hints carry their project", () => {
  const resyncs: (readonly string[])[] = [];
  const hints: WorkspaceTaskStreamHint[] = [];
  const sub = subscribeWorkspaceTaskStream("ws", ["a", "b", "a"], {
    onResync: (projects) => resyncs.push(projects),
    onTask: (hint) => hints.push(hint),
  });
  const first = MockEventSource.latest();
  assert.equal(first.url, "/api/v1/workspaces/ws/task-stream");
  assert.deepEqual(resyncs, [], "a connecting source resyncs on its open, not at subscribe");
  first.connect();
  assert.deepEqual(resyncs, [["a", "b"]]);

  for (const body of [
    { taskId: "t1", verb: "task.updated" },
    { taskId: "t1", verb: "task.updated", projectId: "" },
    { taskId: "t1", verb: "task.updated", projectId: 7 },
  ])
    first.message("task", JSON.stringify(body));
  first.message("task", JSON.stringify({ taskId: "t1", verb: "task.updated", projectId: "b" }));
  assert.deepEqual(hints, [{ taskId: "t1", verb: "task.updated", projectId: "b" }]);

  // Refused (429 at the stream cap): the reopened connection resyncs again.
  first.fail(MockEventSource.CLOSED);
  mock.timers.tick(1_000);
  const reopened = MockEventSource.latest();
  assert.notEqual(reopened, first);
  reopened.connect();
  assert.deepEqual(resyncs, [
    ["a", "b"],
    ["a", "b"],
  ]);
  sub.close();
  assert.equal(reopened.closed, true);
});

await test("projects of a tab share one workspace socket; only a newly watched project resyncs on an open connection", () => {
  const shell: (readonly string[])[] = [];
  const page: (readonly string[])[] = [];
  const stale: (readonly string[])[] = [];
  const shellSub = subscribeWorkspaceTaskStream("ws", ["a", "b"], {
    onResync: (projects) => shell.push(projects),
    onTask: () => {},
  });
  MockEventSource.latest().connect();
  // A page of a project the shell already watches: hints were never dropped.
  const pageSub = subscribeWorkspaceTaskStream("ws", ["a"], {
    onResync: (projects) => page.push(projects),
    onTask: () => {},
  });
  // A project the shell's list lacks: hints since the open went nowhere.
  const staleSub = subscribeWorkspaceTaskStream("ws", ["a", "c"], {
    onResync: (projects) => stale.push(projects),
    onTask: () => {},
  });
  assert.equal(MockEventSource.instances.length, 1, "one socket for every subscription");
  assert.deepEqual(page, []);
  assert.deepEqual(stale, [["c"]]);

  // c stops being watched, then is watched again on the same open socket.
  staleSub.close();
  const again: (readonly string[])[] = [];
  const againSub = subscribeWorkspaceTaskStream("ws", ["c"], {
    onResync: (projects) => again.push(projects),
    onTask: () => {},
  });
  assert.deepEqual(again, [["c"]]);
  assert.deepEqual(shell, [["a", "b"]]);

  for (const sub of [shellSub, pageSub, againSub]) sub.close();
  assert.equal(MockEventSource.latest().closed, true);
});
