import assert from "node:assert/strict";
import test from "node:test";
import {
  clearsHierarchyParent,
  eligibleParentCandidates,
  isEstimateValid,
  isIsoDate,
  patchDateBody,
  patchEstimateBody,
  patchTitleBody,
  patchTypeBody,
  violatesTaskHierarchy,
} from "./task-edit-payload.ts";

await test("isIsoDate accepts only strict YYYY-MM-DD", () => {
  assert.equal(isIsoDate("2026-01-31"), true);
  assert.equal(isIsoDate("9999-12-31"), true);
  for (const bad of ["2026-1-5", "0000-01-01", "2026-02-30", "20260131", ""]) {
    assert.equal(isIsoDate(bad), false, bad);
  }
});

await test("isEstimateValid matches server estimate rules", () => {
  assert.equal(isEstimateValid("2.5"), true);
  assert.equal(isEstimateValid("12"), true);
  assert.equal(isEstimateValid(""), false);
  assert.equal(isEstimateValid("1.2.3"), false);
  assert.equal(isEstimateValid("1234567890123"), false);
});

await test("patchTitleBody trims and rejects empty titles", () => {
  assert.deepEqual(patchTitleBody("  hello  "), { ok: true, body: { title: "hello" } });
  assert.equal(patchTitleBody("   ").ok, false);
});

await test("patchDateBody sends expectedDates for optimistic locking", () => {
  const task = {
    startDate: "2026-01-01",
    dueDate: "2026-01-15",
    dueAt: null,
  };
  const parsed = patchDateBody(task, "dueDate", "2026-02-01");
  assert.equal(parsed.ok, true);
  {
    assert.deepEqual(parsed.body, {
      dueDate: "2026-02-01",
      dueAt: null,
      expectedDates: {
        startDate: "2026-01-01",
        dueDate: "2026-01-15",
        dueAt: null,
      },
    });
  }
  assert.equal(patchDateBody(task, "dueDate", "2026-2-1").ok, false);
});

await test("patchEstimateBody accepts blank to clear and rejects malformed values", () => {
  assert.deepEqual(patchEstimateBody(""), { ok: true, body: { estimate: null } });
  assert.equal(patchEstimateBody("not-a-number").ok, false);
});

await test("patchTypeBody sends type and parent together like source HierarchyForm", () => {
  assert.deepEqual(patchTypeBody("task", null), {
    ok: true,
    body: { type: "task", parentId: null },
  });
  assert.deepEqual(patchTypeBody("subtask", "parent-1"), {
    ok: true,
    body: { type: "subtask", parentId: "parent-1" },
  });
  assert.deepEqual(patchTypeBody("task", "parent-1"), {
    ok: true,
    body: { type: "task", parentId: "parent-1" },
  });
  assert.deepEqual(patchTypeBody("epic", "parent-1"), {
    ok: true,
    body: { type: "epic", parentId: null },
  });
  assert.deepEqual(patchTypeBody("subtask", null), { ok: false, issue: "parent" });
});

await test("clearsHierarchyParent matches source type-boundary rules", () => {
  assert.equal(clearsHierarchyParent("task", "subtask"), true);
  assert.equal(clearsHierarchyParent("subtask", "task"), true);
  assert.equal(clearsHierarchyParent("subtask", "bug"), true);
  assert.equal(clearsHierarchyParent("task", "bug"), false);
  assert.equal(clearsHierarchyParent("task", "epic"), true);
  assert.equal(clearsHierarchyParent("epic", "task"), false);
});

await test("hierarchy helpers filter eligible parents", () => {
  assert.equal(violatesTaskHierarchy("subtask", "epic"), true);
  assert.equal(violatesTaskHierarchy("task", "epic"), false);
  const candidates = eligibleParentCandidates({ id: "self", type: "task" }, [
    { id: "self", type: "task", number: 1, title: "Self" },
    { id: "epic", type: "epic", number: 2, title: "Epic" },
    { id: "bug", type: "bug", number: 3, title: "Bug" },
  ]);
  assert.deepEqual(
    candidates.map((item) => item.id),
    ["epic"],
  );
  const subtaskParents = eligibleParentCandidates({ id: "self", type: "subtask" }, [
    { id: "self", type: "task", number: 1, title: "Self" },
    { id: "epic", type: "epic", number: 2, title: "Epic" },
    { id: "bug", type: "bug", number: 3, title: "Bug" },
  ]);
  assert.deepEqual(
    subtaskParents.map((item) => item.id),
    ["bug"],
  );
});
