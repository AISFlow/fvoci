import assert from "node:assert/strict";
import test from "node:test";
import { canManageMember, inviteRolesFor, roleAtLeast, roleLabel } from "./workspace-role.ts";

test("roleAtLeast: admin can manage, guest cannot", () => {
  assert.equal(roleAtLeast("admin", "admin"), true);
  assert.equal(roleAtLeast("owner", "admin"), true);
  assert.equal(roleAtLeast("member", "admin"), false);
  assert.equal(roleAtLeast("guest", "member"), false);
  assert.equal(roleAtLeast("member", "member"), true);
});

test("inviteRolesFor stays at or below the actor", () => {
  assert.deepEqual(inviteRolesFor("owner"), ["owner", "admin", "member", "guest"]);
  assert.deepEqual(inviteRolesFor("admin"), ["admin", "member", "guest"]);
  assert.deepEqual(inviteRolesFor("member"), ["member", "guest"]);
});

test("canManageMember refuses self and higher-ranked targets", () => {
  assert.equal(
    canManageMember({
      currentUserRole: "admin",
      currentUserId: "me",
      memberUserId: "other",
      memberRole: "member",
    }),
    true,
  );
  assert.equal(
    canManageMember({
      currentUserRole: "admin",
      currentUserId: "me",
      memberUserId: "me",
      memberRole: "admin",
    }),
    false,
  );
  assert.equal(
    canManageMember({
      currentUserRole: "admin",
      currentUserId: "me",
      memberUserId: "boss",
      memberRole: "owner",
    }),
    false,
  );
});

test("roleLabel uses the Korean role copy", () => {
  assert.equal(roleLabel("owner").length > 0, true);
  assert.equal(roleLabel("guest").length > 0, true);
  assert.notEqual(roleLabel("owner"), roleLabel("guest"));
});
