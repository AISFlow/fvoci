import { expect, test } from "bun:test";
import { explicitMinutes, minuteEstimate, planDocumentRef } from "./task-plan-input";

test("explicit minutes preserve zero, unknown unit and exact DTO range", () => {
  expect(explicitMinutes(" ")).toEqual({ valid: true, minutes: null });
  expect(explicitMinutes("0")).toEqual({ valid: true, minutes: 0 });
  expect(explicitMinutes("2147483647")).toEqual({ valid: true, minutes: 2147483647 });
  for (const value of ["-1", "1.5", "1e3", "2147483648", "Infinity"]) {
    expect(explicitMinutes(value)).toEqual({ valid: false });
  }
  expect(minuteEstimate("17.25", null)).toBeNull();
  expect(minuteEstimate("40", null)).toBeNull();
  expect(minuteEstimate("0", "minutes")).toBe(0);
  expect(minuteEstimate("40.0", "minutes")).toBe(40);
});

test("material and notes links stay in the selected ordinary workspace", () => {
  const origin = "http://127.0.0.1:8000";
  expect(planDocumentRef("WIKI-12", "acme", origin)).toEqual({
    displayId: "WIKI-12",
    anchor: null,
  });
  expect(planDocumentRef("/w/acme/READ-3#material", "acme", origin)).toEqual({
    displayId: "READ-3",
    anchor: "material",
  });
  for (const value of [
    "/w/other/WIKI-12",
    "https://example.com/w/acme/WIKI-12",
    "/w/acme/WIKI-12?x=1",
    "/w/acme/WIKI-01",
    "javascript:alert(1)",
    "/w/acme/%ZZ",
  ]) {
    expect(planDocumentRef(value, "acme", origin)).toBeUndefined();
  }
});
