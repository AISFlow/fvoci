import assert from "node:assert/strict";
import test from "node:test";
import { cloneDefaultName, projectFormPayload } from "./project-form.ts";

const VALID = {
  key: "LAB",
  name: "연구실",
  visibility: "workspace" as const,
  description: "",
  icon: "",
};

test("project form: a valid form gives the strict payload", () => {
  assert.deepEqual(
    projectFormPayload({ ...VALID, name: "  연구실 ", description: " 설명 ", leadUserId: "u1" }),
    {
      ok: true,
      body: {
        key: "LAB",
        name: "연구실",
        visibility: "workspace",
        description: "설명",
        icon: null,
        leadUserId: "u1",
      },
    },
  );
  assert.deepEqual(projectFormPayload({ ...VALID, leadUserId: "" }), {
    ok: true,
    body: { key: "LAB", name: "연구실", visibility: "workspace", description: null, icon: null },
  });
});

test("project form: the first failing field wins, key before name, description and icon", () => {
  const all = { ...VALID, key: "", name: " ", description: "x".repeat(2001), icon: "x".repeat(51) };
  assert.deepEqual(projectFormPayload(all), { ok: false, field: "key", message: "form.too_small" });
  assert.deepEqual(projectFormPayload({ ...all, key: "LAB" }), {
    ok: false,
    field: "name",
    message: "form.too_small",
  });
  assert.deepEqual(projectFormPayload({ ...all, key: "LAB", name: "a" }), {
    ok: false,
    field: "description",
    message: "form.too_big",
  });
  assert.deepEqual(projectFormPayload({ ...all, key: "LAB", name: "a", description: "" }), {
    ok: false,
    field: "icon",
    message: "form.too_big",
  });
  assert.deepEqual(projectFormPayload({ ...VALID, name: "x".repeat(201) }), {
    ok: false,
    field: "name",
    message: "form.too_big",
  });
});

test("project form: reserved and malformed keys get the key messages", () => {
  assert.deepEqual(projectFormPayload({ ...VALID, key: "WIKI" }), {
    ok: false,
    field: "key",
    message: "project.key.reserved",
  });
  assert.deepEqual(projectFormPayload({ ...VALID, key: "MY-TASKS" }), {
    ok: false,
    field: "key",
    message: "project.key.reserved",
  });
  for (const key of ["L", "1AB", "OPS-5", "A_B"]) {
    assert.deepEqual(
      projectFormPayload({ ...VALID, key }),
      { ok: false, field: "key", message: "form.pattern.key" },
      key,
    );
  }
});

test("clone default name: marks the copy and keeps the name limit", () => {
  assert.equal(cloneDefaultName("Home Wiki"), "Home Wiki (복사)");
  const long = "가".repeat(200);
  const named = cloneDefaultName(long);
  assert.equal(named.length, 200);
  assert.ok(named.endsWith(" (복사)"));
});
