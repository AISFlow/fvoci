import assert from "node:assert/strict";
import test from "node:test";
import { parseRhwpMutation, rhwpMutationChanged } from "./rhwp-mutation.ts";

test("replaceAll counts and replaceOne positions parse; only a positive count or a position is a change", () => {
  assert.deepEqual(parseRhwpMutation('{"ok":true,"count":2}'), { ok: true, count: 2 });
  assert.equal(rhwpMutationChanged(parseRhwpMutation('{"ok":true,"count":2}')), true);
  const zero = parseRhwpMutation('{"ok":true,"count":0}');
  assert.deepEqual(zero, { ok: true, count: 0 });
  assert.equal(rhwpMutationChanged(zero), false);
  const one = parseRhwpMutation('{"ok":true,"sec":0,"para":0,"charOffset":0,"newLength":1}');
  assert.deepEqual(one, { ok: true });
  assert.equal(rhwpMutationChanged(one), true);
});

test("refusals and malformed answers are failures, never changes", () => {
  const cases: [string, string][] = [
    ['{"ok":false}', "not_ok"],
    ['{"count":3}', "not_ok"],
    ['{"ok":"true","count":1}', "not_ok"],
    ['{"ok":true,"count":"x"}', "invalid_count"],
    ['{"ok":true,"count":-1}', "invalid_count"],
    ['{"ok":true,"count":1.5}', "invalid_count"],
    ["[]", "invalid_shape"],
    ["null", "invalid_shape"],
    ["2", "invalid_shape"],
    ["not-json", "invalid_json"],
    ["", "invalid_json"],
  ];
  for (const [json, reason] of cases) {
    const result = parseRhwpMutation(json);
    assert.deepEqual(result, { ok: false, reason }, json);
    assert.equal(rhwpMutationChanged(result), false, json);
  }
});
