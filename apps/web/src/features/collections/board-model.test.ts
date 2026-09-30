import assert from "node:assert/strict";
import test from "node:test";
import {
  BOARD_PAGE_LIMIT,
  boardColumnBody,
  columnRows,
  moveChoices,
  moveRequest,
  type BoardRow,
} from "./board-model.ts";

const config = { query: { filters: {}, sort: [] }, groupBy: "field-1", dateBy: null };

function row(id: string, group: string | null, patch: Partial<BoardRow> = {}): BoardRow {
  return { id, group, canEdit: true, taskId: `task-${id}`, statusId: "s-1", values: {}, ...patch };
}

test("column body always names its group, including the unassigned column", () => {
  assert.deepEqual(boardColumnBody(config, null), { config, group: null, limit: BOARD_PAGE_LIMIT });
  assert.deepEqual(boardColumnBody(config, "opt-a", "c1"), {
    config,
    group: "opt-a",
    limit: BOARD_PAGE_LIMIT,
    cursor: "c1",
  });
  assert.ok(Object.hasOwn(boardColumnBody(config, null), "group"));
});

test("column rows keep page order, drop duplicates and rows of another group", () => {
  const pages = [
    { items: [row("1", "a"), row("2", "a")] },
    // Page 2 fetched after card 2 was re-sorted and card 3 moved to group b.
    { items: [row("2", "a"), row("3", "b"), row("4", "a")] },
  ];
  assert.deepEqual(
    columnRows(pages, "a").map((item) => item.id),
    ["1", "2", "4"],
  );
  assert.deepEqual(
    columnRows(pages, "b").map((item) => item.id),
    ["3"],
  );
  assert.deepEqual(
    columnRows([{ items: [row("5", null)] }], null).map((item) => item.id),
    ["5"],
  );
});

test("more than one page per column stays complete and unique", () => {
  const all = Array.from({ length: 120 }, (_, index) => row(String(index), "a"));
  const pages = [
    { items: all.slice(0, BOARD_PAGE_LIMIT) },
    { items: all.slice(BOARD_PAGE_LIMIT, BOARD_PAGE_LIMIT * 2) },
    { items: all.slice(BOARD_PAGE_LIMIT * 2) },
  ];
  const rows = columnRows(pages, "a");
  assert.equal(rows.length, 120);
  assert.equal(new Set(rows.map((item) => item.id)).size, 120);
});

test("status move sends the CAS status and refuses unassigned or statusless rows", () => {
  assert.deepEqual(moveRequest("status", row("1", "s-1"), { id: "s-2", deleted: false }), {
    kind: "status",
    taskId: "task-1",
    statusId: "s-2",
    expectedStatusId: "s-1",
  });
  assert.equal(moveRequest("status", row("1", "s-1"), { id: null, deleted: false }), null);
  assert.equal(
    moveRequest("status", row("1", null, { statusId: null }), { id: "s-2", deleted: false }),
    null,
  );
  assert.equal(
    moveRequest("status", row("1", "s-1", { taskId: null }), { id: "s-2", deleted: false }),
    null,
  );
});

test("select field move sets one option or clears to unassigned", () => {
  assert.deepEqual(moveRequest("field-1", row("1", null), { id: "opt-a", deleted: false }), {
    kind: "field",
    fieldId: "field-1",
    value: { options: ["opt-a"] },
  });
  assert.deepEqual(moveRequest("field-1", row("1", "opt-a"), { id: null, deleted: false }), {
    kind: "field",
    fieldId: "field-1",
    value: null,
  });
});

test("no move for read-only rows, archived options, the same group or no grouping", () => {
  assert.equal(
    moveRequest("field-1", row("1", null, { canEdit: false }), { id: "opt-a", deleted: false }),
    null,
  );
  assert.equal(moveRequest("field-1", row("1", null), { id: "opt-old", deleted: true }), null);
  assert.equal(moveRequest("field-1", row("1", "opt-a"), { id: "opt-a", deleted: false }), null);
  assert.equal(moveRequest(null, row("1", "opt-a"), { id: "opt-b", deleted: false }), null);
});

test("keyboard choices mirror the board groups; archived ones stay visible but disabled", () => {
  const groups = [
    { id: "opt-a", name: "A", count: 1, deleted: false },
    { id: "opt-old", name: "Old", count: 1, deleted: true },
    { id: null, name: "", count: 0, deleted: false },
  ];
  assert.deepEqual(moveChoices("field-1", groups), [
    { id: "opt-a", name: "A", disabled: false },
    { id: "opt-old", name: "Old", disabled: true },
    { id: null, name: "", disabled: false },
  ]);
  assert.deepEqual(moveChoices("status", [{ id: null, name: "", count: 0, deleted: false }]), []);
});
