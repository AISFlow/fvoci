import assert from "node:assert/strict";
import test from "node:test";
import {
  canonicalizeProjectKey,
  parseRef,
  parseWikiRef,
  projectKeyIssue,
  projectTasksPath,
  projectsPath,
  searchPath,
  attachmentPath,
  COMMENTS_ANCHOR_ID,
} from "./href.ts";

test("parseRef distinguishes project keys from KEY-n items and wiki refs", () => {
  assert.deepEqual(parseRef("LAB"), { kind: "project", key: "LAB" });
  assert.deepEqual(parseRef("lab"), { kind: "project", key: "LAB" });
  assert.deepEqual(parseRef("LAB-2"), {
    kind: "item",
    prefix: "LAB",
    number: 2,
    displayId: "LAB-2",
  });
  assert.deepEqual(parseWikiRef("WIKI-12"), { prefix: "WIKI", number: 12 });
  assert.equal(parseWikiRef("LAB-2"), null);
  assert.equal(parseRef("WIKI"), null);
  assert.equal(parseRef("projects"), null);
  assert.equal(parseRef("OPS-01"), null);
  assert.equal(parseRef("OPS-5")?.kind, "item");
});

test("project key validation matches source reserved and KEY-n rejection", () => {
  assert.equal(projectKeyIssue("LAB"), null);
  assert.equal(projectKeyIssue("lab"), null);
  assert.equal(canonicalizeProjectKey("lab"), "LAB");
  assert.equal(projectKeyIssue("WIKI"), "reserved");
  assert.equal(projectKeyIssue("wiki"), "reserved");
  assert.equal(projectKeyIssue("OPS-5"), "pattern");
  assert.equal(projectKeyIssue("L"), "pattern");
});

test("canonical project and task paths lower-case slug and upper-case key", () => {
  assert.equal(projectsPath("Acme"), "/w/acme/projects");
  assert.equal(projectTasksPath("Acme", "lab"), "/w/acme/LAB/tasks");
  assert.equal(searchPath("Acme"), "/w/acme/search");
  assert.equal(searchPath("Acme", { q: "ㄱㅅ", tab: "document" }), "/w/acme/search?q=%E3%84%B1%E3%85%85&tab=document");
  assert.equal(attachmentPath("Acme", "att-1"), "/w/acme/a/att-1/view");
  assert.equal(attachmentPath("Acme", "att-1", 2), "/w/acme/a/att-1/view?chunk=2");
  assert.equal(COMMENTS_ANCHOR_ID, "document-comments");
});
