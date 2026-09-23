import assert from "node:assert/strict";
import test from "node:test";
import { childrenByParent, childrenOf } from "./wiki-tree.ts";

test("childrenByParent indexes nested wiki nodes for recursive rendering", () => {
  const nodes = [
    { id: "root", parentId: null },
    { id: "child", parentId: "root" },
    { id: "grandchild", parentId: "child" },
  ];
  const byParent = childrenByParent(nodes);
  assert.deepEqual(childrenOf(byParent, null).map((item) => item.id), ["root"]);
  assert.deepEqual(childrenOf(byParent, "root").map((item) => item.id), ["child"]);
  assert.deepEqual(childrenOf(byParent, "child").map((item) => item.id), ["grandchild"]);
  assert.deepEqual(childrenOf(byParent, "grandchild"), []);
});
