import assert from "node:assert/strict";
import { test } from "node:test";
import { QueryClient, QueryObserver } from "@tanstack/query-core";
import type { TaskDetail, TaskMeta } from "./queries";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import { mergeTaskMeta, settleTaskPatch } from "./task-patch-cache.ts";

const WS = "ws-1";
const PROJECT = "project-1";
const TASK = "task-1";
const PARENT = "parent-1";

function detail(overrides: Partial<TaskDetail> = {}): TaskDetail {
  return {
    archivedAt: null,
    createdAt: "2026-01-01T00:00:00Z",
    createdBy: "user-1",
    dueAt: null,
    dueDate: "2026-01-31",
    estimate: null,
    id: TASK,
    milestoneId: null,
    number: 3,
    parentId: null,
    priority: "high",
    projectId: PROJECT,
    recurrence: null,
    schemaVersion: 1,
    sortKey: "a0",
    startDate: null,
    statusId: "status-1",
    title: "반복 일감",
    type: "task",
    updatedAt: "2026-01-01T00:00:00Z",
    version: 1,
    workspaceId: WS,
    assigneeIds: ["user-1"],
    canEdit: true,
    childProgress: null,
    children: [],
    contentJson: null,
    dependencies: [],
    labelIds: ["label-1"],
    parent: null,
    ...overrides,
  };
}

function metaOf(task: TaskDetail): TaskMeta {
  const {
    assigneeIds: _a,
    canEdit: _c,
    childProgress: _p,
    children: _ch,
    contentJson: _j,
    dependencies: _d,
    labelIds: _l,
    parent: _pa,
    ...meta
  } = task;
  return meta;
}

type Deferred = { resolve: (value: TaskDetail) => void; promise: Promise<TaskDetail> };

/**
 * A mounted `["task", ws, id]` query whose GETs the test answers by hand, so the
 * order of responses is fixed instead of left to network timing.
 */
function mountTaskQuery(initial: TaskDetail) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const gets: Deferred[] = [];
  client.setQueryData(["task", WS, TASK], initial);
  const observer = new QueryObserver(client, {
    queryKey: ["task", WS, TASK],
    queryFn: () => {
      let resolve!: (value: TaskDetail) => void;
      const promise = new Promise<TaskDetail>((r) => {
        resolve = r;
      });
      gets.push({ resolve, promise });
      return promise;
    },
    staleTime: Infinity,
  });
  const unsubscribe = observer.subscribe(() => {});
  const cached = () => client.getQueryData<TaskDetail>(["task", WS, TASK]);
  return { client, gets, cached, unsubscribe };
}

const flush = () => new Promise((resolve) => setImmediate(resolve));

test("settled hierarchy PATCH is in the task cache even when stream invalidations overlap its refetch", async () => {
  const before = detail();
  const saved = detail({ type: "subtask", parentId: PARENT, updatedAt: "2026-01-01T00:00:05Z" });
  const { client, gets, cached, unsubscribe } = mountTaskQuery(before);

  let settled = false;
  const settle = settleTaskPatch(client, WS, PROJECT, metaOf(saved)).then(() => {
    settled = true;
  });
  await flush();
  // Two `task` stream hints (this write and an earlier one) arrive while the
  // PATCH-success refetch is still in flight; each restarts the GET.
  void invalidateTaskCaches(client, WS, PROJECT, TASK);
  await flush();
  void invalidateTaskCaches(client, WS, PROJECT, TASK);
  await flush();

  // Only the newest GET may land; it has not answered yet.
  assert.equal(settled, true, "PATCH success settles once the cancelled refetches settle");
  assert.equal(cached()?.type, "subtask");
  assert.equal(cached()?.parentId, PARENT);

  gets.at(-1)?.resolve(saved);
  await settle;
  await flush();
  assert.equal(cached()?.type, "subtask");
  assert.equal(cached()?.parentId, PARENT);
  unsubscribe();
  client.clear();
});

test("a GET that started before the PATCH committed cannot overwrite the PATCH result", async () => {
  const before = detail();
  const saved = detail({ type: "subtask", parentId: PARENT, updatedAt: "2026-01-01T00:00:05Z" });
  const { client, gets, cached, unsubscribe } = mountTaskQuery(before);

  // Earlier write's stream hint: a GET that reads the pre-hierarchy row.
  void invalidateTaskCaches(client, WS, PROJECT, TASK);
  await flush();
  const staleGet = gets.at(-1);
  assert.ok(staleGet);

  const settle = settleTaskPatch(client, WS, PROJECT, metaOf(saved));
  await flush();
  staleGet.resolve(before);
  await flush();
  assert.equal(cached()?.type, "subtask");
  assert.equal(cached()?.parentId, PARENT);

  gets.at(-1)?.resolve(saved);
  await settle;
  assert.equal(cached()?.type, "subtask");
  assert.equal(cached()?.parentId, PARENT);
  unsubscribe();
  client.clear();
});

test("mergeTaskMeta keeps detail-only fields and drops a parent preview that no longer matches", () => {
  const parent = { id: PARENT, number: 2, title: "부모 일", type: "task" };
  const cachedWithParent = detail({ type: "subtask", parentId: PARENT, parent });

  const cleared = mergeTaskMeta(cachedWithParent, metaOf(detail({ type: "task", parentId: null })));
  assert.equal(cleared?.type, "task");
  assert.equal(cleared?.parentId, null);
  assert.equal(cleared?.parent, null);
  assert.deepEqual(cleared?.assigneeIds, ["user-1"]);
  assert.deepEqual(cleared?.labelIds, ["label-1"]);
  assert.equal(cleared?.canEdit, true);

  const kept = mergeTaskMeta(
    cachedWithParent,
    metaOf(detail({ type: "subtask", parentId: PARENT, title: "새 제목" })),
  );
  assert.equal(kept?.parent, parent);
  assert.equal(kept?.title, "새 제목");

  const moved = mergeTaskMeta(
    cachedWithParent,
    metaOf(detail({ type: "subtask", parentId: "other" })),
  );
  assert.equal(moved?.parent, null);

  assert.equal(mergeTaskMeta(undefined, metaOf(detail())), undefined);
  const other = detail({ id: "task-2" });
  assert.equal(mergeTaskMeta(other, metaOf(detail({ title: "x" }))), other);
});
