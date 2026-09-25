import assert from "node:assert/strict";
import test from "node:test";
import { parentListViewQuery, parentSearchMode, parentTypeFilter } from "./task-parent-query.ts";

test("parentSearchMode uses lookup for this project's display id", () => {
  assert.deepEqual(parentSearchMode("edt-2", "EDT"), {
    kind: "display-id",
    displayId: "EDT-2",
  });
  assert.deepEqual(parentSearchMode("OTHER-9", "EDT"), { kind: "empty" });
  assert.deepEqual(parentSearchMode("부모 일", "EDT"), { kind: "list", title: "부모 일" });
  assert.deepEqual(parentSearchMode("  ", "EDT"), { kind: "list", title: undefined });
});

test("parent list query encodes title and epic type without a client-side scan", () => {
  assert.equal(parentTypeFilter("task"), "epic");
  assert.equal(parentTypeFilter("bug"), "epic");
  assert.equal(parentTypeFilter("subtask"), undefined);
  assert.equal(parentTypeFilter("epic"), undefined);
  assert.equal(parentListViewQuery("task", "부모"), '{"filters":{"type":"epic","title":"부모"}}');
  assert.equal(parentListViewQuery("subtask", "부모"), '{"filters":{"title":"부모"}}');
  assert.equal(parentListViewQuery("task"), '{"filters":{"type":"epic"}}');
});
