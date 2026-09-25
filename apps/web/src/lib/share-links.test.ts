import assert from "node:assert/strict";
import test from "node:test";
import {
  isSafeShareHref,
  sharePathFromUrl,
  shareTreeChildren,
  shareTreeRoots,
  starItemDisplayId,
} from "./share-links.ts";

test("shareTreeRoots treats a parent outside the shared subtree as a root", () => {
  const nodes = [
    { id: "root", parentId: "outside" },
    { id: "child", parentId: "root" },
    { id: "grand", parentId: "child" },
  ];
  assert.deepEqual(shareTreeRoots(nodes).map((n) => n.id), ["root"]);
  assert.deepEqual(shareTreeChildren(nodes, "root").map((n) => n.id), ["child"]);
  assert.deepEqual(
    shareTreeRoots([{ id: "a", parentId: null }, { id: "b", parentId: "a" }]).map((n) => n.id),
    ["a"],
  );
  assert.deepEqual(shareTreeRoots([]), []);
});

test("isSafeShareHref allows http/https/mailto/relative only", () => {
  assert.equal(isSafeShareHref("https://example.com/a"), true);
  assert.equal(isSafeShareHref("HTTP://example.com"), true);
  assert.equal(isSafeShareHref("mailto:a@example.com"), true);
  assert.equal(isSafeShareHref("/w/acme/WIKI-1"), true);
  assert.equal(isSafeShareHref("#section"), true);
  assert.equal(isSafeShareHref("javascript:alert(1)"), false);
  assert.equal(isSafeShareHref(" JavaScript:alert(1)"), false);
  assert.equal(isSafeShareHref("data:text/html,x"), false);
  assert.equal(isSafeShareHref("//evil.example"), false);
  assert.equal(isSafeShareHref("\\\\evil.example"), false);
  assert.equal(isSafeShareHref("/\\evil.example"), false);
  assert.equal(isSafeShareHref("\\/evil.example"), false);
  assert.equal(isSafeShareHref(""), false);
});

test("sharePathFromUrl keeps only the /s/:token path", () => {
  assert.equal(sharePathFromUrl("http://127.0.0.1:0/s/abc_DEF-1"), "/s/abc_DEF-1");
  assert.equal(sharePathFromUrl("/s/tok"), "/s/tok");
  assert.equal(sharePathFromUrl("https://x.example/w/acme"), null);
  assert.equal(sharePathFromUrl("https://x.example/s/a/b"), null);
});

test("starItemDisplayId uses WIKI for wiki items and the project key otherwise", () => {
  const keys = new Map([["p1", "LAB"]]);
  assert.equal(starItemDisplayId({ projectId: null, number: 3 }, keys), "WIKI-3");
  assert.equal(starItemDisplayId({ projectId: "p1", number: 7 }, keys), "LAB-7");
  assert.equal(starItemDisplayId({ projectId: "p2", number: 7 }, keys), null);
});
