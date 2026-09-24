import assert from "node:assert/strict";
import test from "node:test";
import {
  DOCUMENT_SCHEMA_VERSION,
  emptyDocumentJson,
  isTiptapDoc,
  tiptapDocSchema,
} from "../src/json.ts";

test("schema version 2 · empty doc", () => {
  assert.equal(DOCUMENT_SCHEMA_VERSION, 2);
  assert.deepEqual(emptyDocumentJson(), {
    type: "doc",
    content: [{ type: "paragraph" }],
  });
  assert.equal(isTiptapDoc(emptyDocumentJson()), true);
  assert.equal(isTiptapDoc([]), false);
  assert.equal(isTiptapDoc({ type: "doc" }), true);
  assert.equal(isTiptapDoc({ type: "doc", content: [] }), true);
  assert.equal(isTiptapDoc({ type: "doc", content: {} }), false);
  assert.equal(tiptapDocSchema.safeParse({ type: "doc", content: [] }).success, true);
  assert.equal(tiptapDocSchema.safeParse([{ type: "paragraph" }]).success, false);
});
