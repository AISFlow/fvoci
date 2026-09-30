import assert from "node:assert/strict";
import test from "node:test";
import type { TreeNode } from "@/lib/queries/documents";
import { resolveTreeDrop } from "./tree-drop";
const node = (id: string, parentId: string | null = null, projectId: string | null = null) => ({ id, parentId, projectId }) as TreeNode;
const nodes = [node("a"), node("b"), node("c"), node("child", "a"), node("root", null, "p")];
test("wiki edges sort siblings and center reparents without root/cycle/no-op moves", () => {
  assert.deepEqual(resolveTreeDrop(nodes, "c", "a", "top"), { type: "sort", afterId: null });
  assert.deepEqual(resolveTreeDrop(nodes, "a", "b", "bottom"), { type: "sort", afterId: "b" });
  assert.deepEqual(resolveTreeDrop(nodes, "b", "a", "onto"), { type: "move", newParentId: "a" });
  assert.equal(resolveTreeDrop(nodes, "a", "child", "onto"), null);
  assert.equal(resolveTreeDrop(nodes, "child", "a", "onto"), null);
  assert.equal(resolveTreeDrop(nodes, "root", "a", "onto"), null);
  assert.equal(resolveTreeDrop(nodes, "b", "a", "bottom"), null);
  assert.equal(resolveTreeDrop(nodes, "a", "a", "top"), null);
});
