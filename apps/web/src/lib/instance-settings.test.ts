import assert from "node:assert/strict";
import test from "node:test";
import { publicInstanceQuery } from "./queries/admin.ts";
import { aiEnabledQuery, selectAiEnabled } from "./queries/instance-settings.ts";

type Instance = Parameters<typeof selectAiEnabled>[0];

function instance(ai: unknown): Instance {
  return { version: 1, values: { features: { ai } } } as unknown as Instance;
}

test("AI gate reuses the public instance cache entry", () => {
  assert.deepEqual(aiEnabledQuery.queryKey, publicInstanceQuery.queryKey);
  assert.equal(aiEnabledQuery.queryFn, publicInstanceQuery.queryFn);
});

test("AI gate opens only on an explicit true", () => {
  assert.equal(selectAiEnabled(instance(true)), true);
  assert.equal(selectAiEnabled(instance(false)), false);
  assert.equal(selectAiEnabled(instance(undefined)), false);
  assert.equal(selectAiEnabled(instance("true")), false);
});
