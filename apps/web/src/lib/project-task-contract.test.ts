import assert from "node:assert/strict";
import test from "node:test";
import { projectCreatePayload } from "../features/projects/create-payload.ts";
import { TASK_TITLE_MAX, taskCreatePayload } from "../features/tasks/create-payload.ts";
import type { components, paths } from "../generated/api.ts";
import { pickLookupTask, type LookupItem } from "../features/tasks/lookup.ts";
import {
  appendTaskListPage,
  mergeTaskListPages,
  statusCountFor,
  taskListHasMore,
} from "../features/tasks/task-list-page.ts";

test("generated OpenAPI includes lookup and required list nextCursor/statusCounts", () => {
  const lookupPath: keyof paths = "/api/v1/workspaces/{workspace_id}/lookup/{display_id}";
  assert.equal(lookupPath, "/api/v1/workspaces/{workspace_id}/lookup/{display_id}");
  const page: components["schemas"]["TaskListResponse"] = {
    items: [],
    nextCursor: null,
    statusCounts: [{ statusId: "s-backlog", count: 1 }],
  };
  assert.equal(page.nextCursor, null);
  assert.equal(page.statusCounts[0]?.count, 1);
});

test("project create payload matches source NFKC, reserved, KEY-n, and blank-to-null", () => {
  const fullwidth = projectCreatePayload({
    key: "ｌａｂ",
    name: "  Lab  ",
    visibility: "workspace",
    description: "  ",
    icon: "",
  });
  assert.deepEqual(fullwidth, {
    ok: true,
    body: {
      key: "LAB",
      name: "Lab",
      visibility: "workspace",
      description: null,
      icon: null,
    },
  });
  assert.equal("leadUserId" in (fullwidth.ok ? fullwidth.body : {}), false);

  assert.deepEqual(
    projectCreatePayload({ key: "WIKI", name: "Wiki", visibility: "private" }),
    { ok: false, issue: { field: "key", code: "reserved" } },
  );
  assert.deepEqual(
    projectCreatePayload({ key: "OPS-5", name: "Ops", visibility: "workspace" }),
    { ok: false, issue: { field: "key", code: "pattern" } },
  );
  assert.equal(
    projectCreatePayload({
      key: "LAB",
      name: "Lab",
      visibility: "workspace",
      description: "x".repeat(2001),
    }).ok,
    false,
  );
});

test("task create payload trims title, defaults type, and never sends parentId", () => {
  const created = taskCreatePayload({ title: "  첫 일  ", type: "task" });
  assert.deepEqual(created, { ok: true, body: { title: "첫 일", type: "task" } });
  assert.equal(created.ok && "parentId" in created.body, false);
  assert.equal(taskCreatePayload({ title: "   ", type: "bug" }).ok, false);
  assert.equal(taskCreatePayload({ title: "x".repeat(TASK_TITLE_MAX + 1), type: "task" }).ok, false);
  assert.deepEqual(taskCreatePayload({ title: "하위", type: "subtask" }), {
    ok: false,
    issue: "parent",
  });
  assert.equal(taskCreatePayload({ title: "일", type: "milestone" }).ok, false);
});

const doc: LookupItem = {
  kind: "document",
  id: "doc-1",
  displayId: "LAB-2",
  title: "문서",
  projectId: "p1",
};
const task: LookupItem = {
  kind: "task",
  id: "task-1",
  displayId: "LAB-2",
  title: "첫 일",
  projectId: "p1",
};

test("lookup selection uses kind===task; empty or document-only is a miss", () => {
  assert.equal(pickLookupTask([], "LAB-2"), null);
  assert.equal(pickLookupTask([doc], "LAB-2"), null);
  assert.deepEqual(pickLookupTask([doc, task], "lab-2"), task);
  assert.equal(pickLookupTask([task], "HID-2"), null);
  assert.equal(pickLookupTask([{ ...task, kind: "document" }], "LAB-2"), null);
});

test("appendTaskListPage concatenates items, keeps first statusCounts, and uses page cursor", () => {
  const first = {
    items: [{ id: "t1" }],
    nextCursor: "c1",
    statusCounts: [{ statusId: "s-backlog", count: 65 }],
  };
  const second = {
    items: [{ id: "t2" }],
    nextCursor: null,
    statusCounts: [{ statusId: "s-backlog", count: 2 }],
    truncated: true,
  };
  assert.deepEqual(appendTaskListPage(undefined, first), first);
  const merged = appendTaskListPage(first, second);
  assert.deepEqual(merged.items, [{ id: "t1" }, { id: "t2" }]);
  assert.equal(merged.nextCursor, null);
  assert.deepEqual(merged.statusCounts, [{ statusId: "s-backlog", count: 65 }]);
  assert.equal(merged.truncated, true);
  assert.equal(taskListHasMore(first), true);
  assert.equal(taskListHasMore(merged), false);
  assert.equal(statusCountFor(first.statusCounts, "s-backlog"), 65);
  assert.equal(statusCountFor(first.statusCounts, "missing"), undefined);
  const fromPages = mergeTaskListPages([first, second]);
  assert.deepEqual(fromPages, merged);
});
