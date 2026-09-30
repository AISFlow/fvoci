import assert from "node:assert/strict";
import test from "node:test";
import {
  chunkSearch,
  isDocx,
  isExtractableText,
  isHwp,
  isPptx,
  isXlsx,
  viewerKind,
} from "./attachment-kind.ts";

await test("chunkSearch accepts a non-negative integer chunk query", () => {
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=3")), { chunk: 3 });
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=0")), { chunk: 0 });
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=-1")), {});
  assert.deepEqual(chunkSearch(new URLSearchParams("chunk=1.5")), {});
  assert.deepEqual(chunkSearch(new URLSearchParams()), {});
});

await test("viewerKind prefers images, then PDF, then HWP, then DOCX, then PPTX, then XLSX, then extractable text, then download", () => {
  assert.equal(viewerKind({ name: "a.png", mime: "image/png", image: true }), "image");
  assert.equal(viewerKind({ name: "a.pdf", mime: "application/pdf", image: false }), "pdf");
  assert.equal(
    viewerKind({ name: "A.PDF", mime: "application/octet-stream", image: false }),
    "pdf",
  );
  assert.equal(viewerKind({ name: "scan", mime: "Application/PDF", image: false }), "pdf");
  assert.equal(viewerKind({ name: "notes.pdf.txt", mime: "text/plain", image: false }), "text");
  assert.equal(viewerKind({ name: "note.txt", mime: "text/plain", image: false }), "text");
  assert.equal(viewerKind({ name: "data.json", mime: "application/json", image: false }), "text");
  assert.equal(
    viewerKind({ name: "bin.dat", mime: "application/octet-stream", image: false }),
    "download",
  );
  assert.equal(isExtractableText("readme.md", "application/octet-stream"), true);
  const docxMime = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
  assert.equal(viewerKind({ name: "report.docx", mime: docxMime, image: false }), "docx");
  assert.equal(
    viewerKind({ name: "REPORT.DOCX", mime: "application/octet-stream", image: false }),
    "docx",
  );
  assert.equal(viewerKind({ name: "upload", mime: docxMime.toUpperCase(), image: false }), "docx");
  const xlsxMime = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
  assert.equal(viewerKind({ name: "book.xlsx", mime: xlsxMime, image: false }), "xlsx");
  assert.equal(
    viewerKind({ name: "BOOK.XLSX", mime: "application/octet-stream", image: false }),
    "xlsx",
  );
  assert.equal(viewerKind({ name: "upload", mime: xlsxMime.toUpperCase(), image: false }), "xlsx");
  // Source order: DOCX is checked first.
  assert.equal(viewerKind({ name: "report.docx", mime: xlsxMime, image: false }), "docx");
  // A spreadsheet MIME wins over a text-looking name, as in the source.
  assert.equal(viewerKind({ name: "export.csv", mime: xlsxMime, image: false }), "xlsx");
  assert.equal(viewerKind({ name: "data.csv", mime: "text/csv", image: false }), "text");
  const pptxMime = "application/vnd.openxmlformats-officedocument.presentationml.presentation";
  assert.equal(viewerKind({ name: "deck.pptx", mime: pptxMime, image: false }), "pptx");
  assert.equal(
    viewerKind({ name: "DECK.PPTX", mime: "application/octet-stream", image: false }),
    "pptx",
  );
  assert.equal(viewerKind({ name: "upload", mime: pptxMime.toUpperCase(), image: false }), "pptx");
  // Source order: DOCX, then PPTX, then XLSX.
  assert.equal(viewerKind({ name: "report.docx", mime: pptxMime, image: false }), "docx");
  assert.equal(viewerKind({ name: "deck.pptx", mime: xlsxMime, image: false }), "pptx");
  assert.equal(viewerKind({ name: "notes.txt", mime: pptxMime, image: false }), "pptx");
  // HWP/HWPX by name or the source `application/x-hwp*` MIME prefix, before Office and text.
  assert.equal(
    viewerKind({ name: "form.hwp", mime: "application/octet-stream", image: false }),
    "hwp",
  );
  assert.equal(viewerKind({ name: "FORM.HWPX", mime: "", image: false }), "hwp");
  assert.equal(viewerKind({ name: "upload", mime: "Application/X-HWP-V5", image: false }), "hwp");
  assert.equal(viewerKind({ name: "form.hwp.txt", mime: "text/plain", image: false }), "text");
  assert.equal(viewerKind({ name: "form.docx", mime: "application/x-hwp", image: false }), "hwp");
  assert.equal(isHwp("form.hwpml", "application/xml"), false);
  assert.equal(isHwp("form.docx", "application/hwpx"), false);
  // Other Office kinds have no layout viewer: they stay download, never text.
  // No new ODS/XLS/XLSM or PPT/PPSX/POTX/ODP support: only the source kinds open.
  for (const name of [
    "memo.odt",
    "old.doc",
    "calc.ods",
    "old.xls",
    "macro.xlsm",
    "old.ppt",
    "show.ppsx",
    "tpl.potx",
    "deck.odp",
    "macro.pptm",
  ]) {
    assert.equal(
      viewerKind({ name, mime: "application/octet-stream", image: false }),
      "download",
      name,
    );
  }
  assert.equal(isDocx("template.dotx", ""), false);
  assert.equal(isXlsx("template.xltx", ""), false);
  assert.equal(isPptx("template.potx", ""), false);
  assert.equal(isPptx("deck.odp", "application/vnd.oasis.opendocument.presentation"), false);
});
