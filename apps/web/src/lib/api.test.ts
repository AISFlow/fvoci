import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

test("product api module does not embed a test-only baseUrl fallback", () => {
  const source = readFileSync(new URL("./api.ts", import.meta.url), "utf8");
  assert.equal(source.includes("test.local"), false);
  assert.equal(source.includes("installApiClient"), false);
});
