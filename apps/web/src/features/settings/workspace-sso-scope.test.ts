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

test("the connected Vue workspace settings page gates its only SSO section with the policy", () => {
  const page = readFileSync(
    path.join(import.meta.dirname, "../../vue/pages/WorkspaceSettingsPage.vue"),
    "utf8",
  );
  assert.match(page, /import \{ showsWorkspaceSso \} from "@\/features\/settings\/workspace-sso-scope"/);
  assert.match(page, /const showSso = computed\(\(\) =>\s*workspace\.value \? showsWorkspaceSso\(workspace\.value\.kind, canManage\.value\) : false/);
  assert.equal((page.match(/<WorkspaceSsoSection\b/g) ?? []).length, 1);
  assert.match(page, /<WorkspaceSsoSection\s+v-if="showSso"\s+:workspace-id="workspace\.id"/);
});
