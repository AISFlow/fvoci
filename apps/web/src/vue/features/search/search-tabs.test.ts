import assert from "node:assert/strict";
import test from "node:test";
import { parseSearchTab, SEARCH_TABS } from "./search-tabs.ts";

await test("parseSearchTab keeps known tabs and falls back to all", () => {
  for (const tab of SEARCH_TABS) {
    assert.equal(parseSearchTab(tab), tab);
  }
  assert.equal(parseSearchTab(null), "all");
  assert.equal(parseSearchTab(undefined), "all");
  assert.equal(parseSearchTab(""), "all");
  assert.equal(parseSearchTab("All"), "all");
  assert.equal(parseSearchTab("documents"), "all");
  assert.equal(parseSearchTab(["task"]), "all");
});
