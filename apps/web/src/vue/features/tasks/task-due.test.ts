import assert from "node:assert/strict";
import test from "node:test";
import { taskDueLabel } from "./task-due";
test("task all-day dates keep their day in negative-offset zones; instants use saved timezone", () => {
  assert.equal(taskDueLabel("2026-10-01", null, "Pacific/Honolulu"), "10. 1.");
  assert.equal(taskDueLabel("2026-10-01", "2026-10-01T02:30:00Z", "Pacific/Honolulu"), "9. 30. 16:30");
  assert.equal(taskDueLabel(null, null, "Pacific/Honolulu"), "");
});
