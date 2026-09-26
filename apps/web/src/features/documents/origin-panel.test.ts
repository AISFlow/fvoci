import assert from "node:assert/strict";
import test from "node:test";
import { originCreateSurface } from "../collections/origin-create-surface.ts";

test("empty picker lets a member create a project and hides the form from a guest", () => {
  assert.equal(
    originCreateSurface({ isLoading: false, isError: false, itemCount: 0, canCreateProject: true }),
    "create-project",
  );
  assert.equal(
    originCreateSurface({ isLoading: false, isError: false, itemCount: 0, canCreateProject: false }),
    "unavailable",
  );
});

test("editable projects show the create-task form", () => {
  assert.equal(
    originCreateSurface({ isLoading: false, isError: false, itemCount: 2, canCreateProject: false }),
    "create-task",
  );
  assert.equal(originCreateSurface({ isLoading: true, isError: false }), "loading");
  assert.equal(originCreateSurface({ isLoading: false, isError: true }), "error");
});
