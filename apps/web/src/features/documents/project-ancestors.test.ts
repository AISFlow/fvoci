import assert from "node:assert/strict";
import { test } from "node:test";
import { projectAncestors } from "./project-ancestors.ts";

const nodes = [
  { id: "root", parentId: null },
  { id: "a", parentId: "root" },
  { id: "b", parentId: "a" },
  { id: "c", parentId: "b" },
];

test("walks root-first and stops at the project root", () => {
  assert.deepEqual(
    projectAncestors(nodes, "c", "root").map((node) => node.id),
    ["a", "b"],
  );
  assert.deepEqual(projectAncestors(nodes, "a", "root"), []);
});

test("unknown or cyclic data terminates", () => {
  assert.deepEqual(projectAncestors(nodes, "missing", "root"), []);
  const cyclic = [
    { id: "x", parentId: "y" },
    { id: "y", parentId: "x" },
  ];
  assert.ok(projectAncestors(cyclic, "x", null).length <= cyclic.length);
});
