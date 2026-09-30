import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { showsWorkspaceSso } from "./workspace-sso-scope.ts";

test("the SSO section is shown only to a team workspace's admins", () => {
  assert.equal(showsWorkspaceSso("team", true), true);
  assert.equal(showsWorkspaceSso("team", false), false);
  assert.equal(showsWorkspaceSso("personal", true), false);
  assert.equal(showsWorkspaceSso("personal", false), false);
  assert.equal(showsWorkspaceSso("", true), false);
});

test("the workspace settings page renders the SSO section behind that rule", () => {
  const page = readFileSync(
    path.join(import.meta.dirname, "../../pages/WorkspaceSettingsPage.tsx"),
    "utf8",
  );
  const uses = page.match(/<WorkspaceSsoSection\b/g) ?? [];
  assert.equal(uses.length, 1);
  assert.match(
    page,
    /\{showsWorkspaceSso\(workspace\.kind, canManage\) \? \(\s*<WorkspaceSsoSection\b/,
  );
});

test("the Vue workspace settings page renders the SSO section behind that rule", () => {
  const page = readFileSync(
    path.join(import.meta.dirname, "../../vue/pages/WorkspaceSettingsPage.vue"),
    "utf8",
  );
  assert.match(page, /showsWorkspaceSso\(/);
  assert.match(page, /<WorkspaceSsoSection\b/);
});
