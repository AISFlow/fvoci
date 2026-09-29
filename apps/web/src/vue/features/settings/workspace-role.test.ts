import assert from "node:assert/strict";
import test from "node:test";
import { roleAtLeast } from "./workspace-role.ts";

test("roleAtLeast: admin can manage, guest cannot", () => {
  assert.equal(roleAtLeast("admin", "admin"), true);
  assert.equal(roleAtLeast("owner", "admin"), true);
  assert.equal(roleAtLeast("member", "admin"), false);
  assert.equal(roleAtLeast("guest", "member"), false);
  assert.equal(roleAtLeast("member", "member"), true);
});
