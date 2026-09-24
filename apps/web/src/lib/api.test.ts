import assert from "node:assert/strict";
import { test } from "node:test";
import { createApiClient } from "./api.ts";

test("default api client does not set a test-only baseUrl", () => {
  const client = createApiClient();
  assert.equal((client as { baseUrl?: string }).baseUrl, undefined);
});
