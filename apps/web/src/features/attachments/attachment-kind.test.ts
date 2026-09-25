import assert from "node:assert/strict";
import test from "node:test";
import { chunkSearch, isExtractableText, viewerKind } from "./attachment-kind.ts";

test("chunkSearch accepts a non-negative integer chunk query", () => {
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=3")), { chunk: 3 });
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=0")), { chunk: 0 });
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=-1")), {});
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=1.5")), {});
  assert.deepEqual(chunkSearch(new URLSearchParams()), {});
});

test("viewerKind prefers images, then extractable text, then download", () => {
  assert.equal(viewerKind({ name: "a.png", mime: "image/png", image: true }), "image");
  assert.equal(viewerKind({ name: "note.txt", mime: "text/plain", image: false }), "text");
  assert.equal(viewerKind({ name: "data.json", mime: "application/json", image: false }), "text");
  assert.equal(viewerKind({ name: "bin.dat", mime: "application/octet-stream", image: false }), "download");
  assert.equal(isExtractableText("readme.md", "application/octet-stream"), true);
});
