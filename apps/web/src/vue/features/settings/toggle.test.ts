import assert from "node:assert/strict";
import test from "node:test";
import { toggleItem } from "./toggle.ts";

test("toggleItem adds once and removes by value", () => {
  assert.deepEqual(toggleItem(["a"], "b", true), ["a", "b"]);
  assert.deepEqual(toggleItem(["a", "b"], "b", true), ["a", "b"]);
  assert.deepEqual(toggleItem(["a", "b"], "b", false), ["a"]);
  assert.deepEqual(toggleItem(["a"], "b", false), ["a"]);
});
