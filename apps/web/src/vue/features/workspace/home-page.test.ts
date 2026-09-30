import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const dir = import.meta.dirname;

function source(file: string): string {
  return readFileSync(path.join(dir, file), "utf8");
}

await test("the Vue home page lists, creates, and leaves with full loads", () => {
  const page = source("../../pages/HomePage.vue");
  assert.match(page, /wikiPath\(workspace\.slug\)/);
  assert.match(page, /api\.POST\("\/api\/v1\/workspaces"/);
  assert.match(page, /logoutRequest\(\)/);
  assert.match(page, /redirectTo\("\/login"\)/);
  assert.match(page, /redirectTo\(loginPath\(window\.location\)\)/);
  assert.match(page, /href="\/settings\/admin"/);
  assert.match(page, /href="\/settings\/account"/);
  assert.match(page, /queryKey: \["me", "workspaces"\]/);
  assert.doesNotMatch(page, /RouterLink/);
  assert.doesNotMatch(page, /from ["']react["']/);
  assert.doesNotMatch(page, /from ["']@tanstack\/react-query["']/);
});

await test("empty workspace and the create dialog submit through the shared schema", () => {
  const empty = source("EmptyWorkspace.vue");
  assert.match(empty, /workspaceCreateInput/);
  assert.match(empty, /<form\b/);
  assert.match(empty, /novalidate/);
  assert.doesNotMatch(empty, /\bv-model\b/);
  assert.doesNotMatch(empty, /:value=/);
  assert.doesNotMatch(empty, /\b(method|action)=/);

  const dialog = source("WorkspaceCreateDialog.vue");
  assert.match(dialog, /workspaceCreateInput/);
  assert.match(dialog, /role="dialog"|NativeModal/);
  assert.match(dialog, /id="create-workspace-name"/);
  assert.match(dialog, /id="create-workspace-slug"/);
  assert.doesNotMatch(dialog, /\bv-model\b/);
  assert.doesNotMatch(dialog, /:value=/);
});
