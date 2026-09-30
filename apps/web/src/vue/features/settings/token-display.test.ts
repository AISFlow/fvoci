import assert from "node:assert/strict";
import test from "node:test";
import { formatExpiry, scopeDomId, tokenScopeLabels } from "./token-display.ts";

test("scopeDomId is a stable checkbox id fragment", () => {
  assert.equal(scopeDomId("documents.read"), "documents-read");
  assert.equal(scopeDomId("workspace.manage"), "workspace-manage");
});

test("formatExpiry uses the unlimited copy for a null expiry", () => {
  assert.equal(formatExpiry(null).length > 0, true);
  const dated = formatExpiry("2026-12-31T00:00:00.000Z");
  assert.match(dated, /2026/);
});

test("tokenScopeLabels joins known scopes", () => {
  const labels = tokenScopeLabels(["documents.read", "tasks.write"]);
  assert.equal(labels.includes(", "), true);
  assert.equal(labels.split(", ").length, 2);
});
