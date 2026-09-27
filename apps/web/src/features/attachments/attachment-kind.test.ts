import assert from "node:assert/strict";
import test from "node:test";
import { chunkSearch, isDocx, isExtractableText, viewerKind } from "./attachment-kind.ts";

test("chunkSearch accepts a non-negative integer chunk query", () => {
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=3")), { chunk: 3 });
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=0")), { chunk: 0 });
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=-1")), {});
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=1.5")), {});
  assert.deepEqual(chunkSearch(new URLSearchParams()), {});
});

test("viewerKind prefers images, then PDF, then DOCX, then extractable text, then download", () => {
  assert.equal(viewerKind({ name: "a.png", mime: "image/png", image: true }), "image");
  assert.equal(viewerKind({ name: "a.pdf", mime: "application/pdf", image: false }), "pdf");
  assert.equal(viewerKind({ name: "A.PDF", mime: "application/octet-stream", image: false }), "pdf");
  assert.equal(viewerKind({ name: "scan", mime: "Application/PDF", image: false }), "pdf");
  assert.equal(viewerKind({ name: "notes.pdf.txt", mime: "text/plain", image: false }), "text");
  assert.equal(viewerKind({ name: "note.txt", mime: "text/plain", image: false }), "text");
  assert.equal(viewerKind({ name: "data.json", mime: "application/json", image: false }), "text");
  assert.equal(viewerKind({ name: "bin.dat", mime: "application/octet-stream", image: false }), "download");
  assert.equal(isExtractableText("readme.md", "application/octet-stream"), true);
  const docxMime = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
  assert.equal(viewerKind({ name: "report.docx", mime: docxMime, image: false }), "docx");
  assert.equal(viewerKind({ name: "REPORT.DOCX", mime: "application/octet-stream", image: false }), "docx");
  assert.equal(viewerKind({ name: "upload", mime: docxMime.toUpperCase(), image: false }), "docx");
  // Other Office/HWP kinds have no layout viewer yet: they stay download, never text.
  for (const name of ["deck.pptx", "sheet.xlsx", "memo.odt", "old.doc", "form.hwp", "form.hwpx"]) {
    assert.equal(viewerKind({ name, mime: "application/octet-stream", image: false }), "download", name);
  }
  assert.equal(isDocx("template.dotx", ""), false);
});
