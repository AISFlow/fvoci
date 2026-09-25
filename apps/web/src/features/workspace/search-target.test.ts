import assert from "node:assert/strict";
import test from "node:test";
import { searchItemHref } from "./search-target.ts";

test("searchItemHref sends comments to the parent display id with the comments anchor", () => {
  assert.equal(
    searchItemHref("Acme", {
      type: "comment",
      id: "c1",
      displayId: "WIKI-12",
      documentId: "d1",
    }),
    "/w/acme/WIKI-12#document-comments",
  );
  assert.equal(
    searchItemHref("Acme", { type: "document", id: "d1", displayId: "LAB-2" }),
    "/w/acme/LAB-2",
  );
  assert.equal(searchItemHref("Acme", { type: "comment", id: "c1" }), null);
});

test("searchItemHref opens attachments on the viewer path and keeps chunk query", () => {
  assert.equal(
    searchItemHref("Acme", { type: "attachment", id: "a1", chunkNo: 3 }),
    "/w/acme/a/a1/view?chunk=3",
  );
  assert.equal(
    searchItemHref("Acme", { type: "attachment", id: "a1", chunkNo: null }),
    "/w/acme/a/a1/view",
  );
});
