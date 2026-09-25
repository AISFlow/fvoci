import assert from "node:assert/strict";
import test from "node:test";
import {
  asCollectionValue,
  draftFromValue,
  fieldTakesOptions,
  formatCollectionValue,
  isoToZonedLocal,
  monthGrid,
  monthWindow,
  optionPatch,
  parseOptionLines,
  shiftMonth,
  suggestedFieldKey,
  valueFromDraft,
  zonedLocalToIso,
} from "./collection-values.ts";

test("option lines trim, drop blanks and duplicates", () => {
  assert.deepEqual(parseOptionLines(" 높음\n\n낮음\n높음 \n"), ["높음", "낮음"]);
  assert.equal(fieldTakesOptions("select"), true);
  assert.equal(fieldTakesOptions("text"), false);
});

test("optionPatch keeps every existing id and drops blank new rows", () => {
  assert.deepEqual(
    optionPatch([
      { id: "a", label: " A ", deleted: false },
      { id: "b", label: "B", deleted: true },
      { label: "  ", deleted: false },
      { label: "C", deleted: false },
      { label: "D", deleted: true },
    ]),
    [
      { id: "a", label: "A", deleted: false },
      { id: "b", label: "B", deleted: true },
      { label: "C" },
    ],
  );
});

test("suggestedFieldKey slugs ASCII names and leaves Korean names to the server", () => {
  assert.equal(suggestedFieldKey("Due Soon!"), "due_soon");
  assert.equal(suggestedFieldKey("2nd phase"), "nd_phase");
  assert.equal(suggestedFieldKey("우선순위"), "");
});

test("time zone round-trip for datetime values", () => {
  assert.equal(isoToZonedLocal("2026-03-01T00:30:00Z", "Asia/Seoul"), "2026-03-01T09:30");
  assert.equal(zonedLocalToIso("2026-03-01T09:30", "Asia/Seoul"), "2026-03-01T00:30:00Z");
  assert.equal(zonedLocalToIso("2026-07-01T12:00", "America/New_York"), "2026-07-01T16:00:00Z");
  // Spring-forward gap does not exist on the wall clock.
  assert.equal(zonedLocalToIso("2026-03-08T02:30", "America/New_York"), null);
  assert.equal(zonedLocalToIso("bad", "UTC"), null);
});

test("draft conversion per field type", () => {
  assert.deepEqual(valueFromDraft("number", "3.5", "UTC"), { number: 3.5 });
  assert.equal(valueFromDraft("number", "abc", "UTC"), "invalid");
  assert.equal(valueFromDraft("text", "   ", "UTC"), null);
  assert.deepEqual(valueFromDraft("paragraph", " a ", "UTC"), { text: " a " });
  assert.deepEqual(valueFromDraft("date", "2026-09-26", "UTC"), { date: "2026-09-26" });
  assert.deepEqual(valueFromDraft("datetime", "2026-09-26T10:00", "UTC"), {
    datetime: "2026-09-26T10:00:00Z",
  });
  assert.equal(draftFromValue({ datetime: "2026-09-26T01:00:00Z" }, "Asia/Seoul"), "2026-09-26T10:00");
  assert.equal(draftFromValue({ number: 2 }, "UTC"), "2");
  assert.equal(draftFromValue(null, "UTC"), "");
});

test("asCollectionValue narrows payloads and formatCollectionValue names options", () => {
  assert.deepEqual(asCollectionValue({ options: ["o1", 3] }), { options: ["o1"] });
  assert.equal(asCollectionValue({ unknown: 1 }), null);
  assert.equal(asCollectionValue(null), null);
  const labels = { yes: "참", no: "거짓" };
  assert.equal(
    formatCollectionValue({ options: ["o1", "o2"] }, [{ id: "o1", label: "높음" }], [], "UTC", labels),
    "높음, o2",
  );
  assert.equal(formatCollectionValue({ checkbox: true }, [], [], "UTC", labels), "참");
  assert.equal(
    formatCollectionValue({ users: ["u1"] }, [], [{ userId: "u1", name: "김관리자" }], "UTC", labels),
    "김관리자",
  );
});

test("month window and grid", () => {
  assert.deepEqual(monthWindow("2026-12"), { from: "2026-12-01", to: "2027-01-01" });
  assert.equal(shiftMonth("2026-01", -1), "2025-12");
  assert.equal(shiftMonth("2026-11", 2), "2027-01");
  // September 2026 starts on a Tuesday.
  const mondayFirst = monthGrid("2026-09", 1);
  assert.equal(mondayFirst[0]![0]!.date, "2026-08-31");
  assert.equal(mondayFirst[0]![1]!.date, "2026-09-01");
  assert.equal(mondayFirst[0]![0]!.inMonth, false);
  assert.equal(mondayFirst.every((week) => week.length === 7), true);
  const sundayFirst = monthGrid("2026-09", 0);
  assert.equal(sundayFirst[0]![0]!.date, "2026-08-30");
  const last = sundayFirst[sundayFirst.length - 1]!;
  assert.equal(last.some((cell) => cell.date === "2026-09-30"), true);
});

test("customEqualsValue types the filter value per field type", async () => {
  const { customEqualsValue } = await import("./collection-values.ts");
  assert.equal(customEqualsValue("number", "4", "UTC"), 4);
  assert.equal(customEqualsValue("number", "x", "UTC"), null);
  assert.equal(customEqualsValue("checkbox", "false", "UTC"), false);
  assert.equal(customEqualsValue("select", "opt-id", "UTC"), "opt-id");
  assert.equal(customEqualsValue("date", "2026-9-1", "UTC"), null);
  assert.equal(customEqualsValue("datetime", "2026-09-01T09:00", "Asia/Seoul"), "2026-09-01T00:00:00Z");
  assert.equal(customEqualsValue("text", "  ", "UTC"), null);
});
