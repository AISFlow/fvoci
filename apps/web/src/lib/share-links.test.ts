import assert from "node:assert/strict";
import test from "node:test";
import {
  isSafeShareHref,
  SHARE_POLICY_DEFAULT,
  selectedShareExpires,
  shareExpiresOptions,
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

test("shareExpiresOptions follows the instance policy like the source dialog", () => {
  assert.deepEqual(shareExpiresOptions(SHARE_POLICY_DEFAULT), [7, 30, 90, 365]);
  // A non-preset default joins the list; presets above the max drop out.
  assert.deepEqual(
    shareExpiresOptions({ enabled: true, defaultExpiresDays: 14, maxExpiresDays: 30 }),
    [7, 14, 30],
  );
  assert.deepEqual(
    shareExpiresOptions({ enabled: true, defaultExpiresDays: 3, maxExpiresDays: 5 }),
    [3],
  );
  assert.deepEqual(
    shareExpiresOptions({ enabled: true, defaultExpiresDays: 30, maxExpiresDays: 365 }),
    [7, 30, 90, 365],
  );
});

test("selectedShareExpires keeps a still-offered pick and otherwise uses the policy default", () => {
  const narrow = { enabled: true, defaultExpiresDays: 14, maxExpiresDays: 30 };
  assert.equal(selectedShareExpires(null, narrow), 14);
  assert.equal(selectedShareExpires(7, narrow), 7);
  // A pick made under an older, wider policy is not sent once it is over the max.
  assert.equal(selectedShareExpires(365, narrow), 14);
  assert.equal(selectedShareExpires(null, SHARE_POLICY_DEFAULT), 7);
});
