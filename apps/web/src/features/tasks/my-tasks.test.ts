import assert from "node:assert/strict";
import test from "node:test";
import { groupTasksByProject, OPEN_ASSIGNED_QUERY } from "./my-tasks.ts";

test("my tasks keep first-seen project order and item order", () => {
  const items = [
    { id: "1", projectId: "b" },
    { id: "2", projectId: "a" },
    { id: "3", projectId: "b" },
  ];
  assert.deepEqual(groupTasksByProject(items), [
    ["b", [items[0], items[2]]],
    ["a", [items[1]]],
  ]);
});

test("the assigned query matches the source constant", () => {
  assert.deepEqual(JSON.parse(OPEN_ASSIGNED_QUERY), {
    filters: { assigneeId: "me", openOnly: true },
    sort: [{ field: "due", direction: "asc" }],
  });
});
