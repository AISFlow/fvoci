import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import { HwpDocument, initSync, version } from "@rhwp/core";
import { decodePageText, pageOfChunk } from "./hwp-page.ts";
import { buildFixtureHwpx, FIXTURE_PAGES, hancomBytes, readZip } from "./hwp-test-fixture.ts";

const require = createRequire(import.meta.url);
const coreDir = path.dirname(require.resolve("@rhwp/core"));
initSync({ module: fs.readFileSync(path.join(coreDir, "rhwp_bg.wasm")) });

function pagesOf(doc: HwpDocument): string[] {
  return Array.from({ length: doc.pageCount() }, (_, i) => decodePageText(doc.getPageText(i)));
}

/** The page SVG must be self-contained: no script, event handler or external reference. */
function assertInertSvg(svg: string): void {
  assert.match(svg, /^<svg[\s>]/);
  assert.doesNotMatch(svg, /<script|<foreignObject|\son[a-z]+=|javascript:/i);
  assert.doesNotMatch(svg, /(?:href|src)="(?!#|data:)/i);
  assert.doesNotMatch(svg, /url\((?!#|["']?data:)/i);
}

test("pinned @rhwp/core is the source contract version", () => {
  assert.equal(version(), "0.8.6");
});

test("the user-authored Hancom HWP and HWPX samples lay out their Korean text", () => {
  for (const kind of ["hwp", "hwpx"] as const) {
    const doc = new HwpDocument(hancomBytes(kind));
    try {
      assert.equal(doc.pageCount(), 1, kind);
      assert.equal(pagesOf(doc)[0], "안녕\n", kind);
      assertInertSvg(doc.renderPageSvg(0));
    } finally {
      doc.free();
    }
  }
});

test("synthetic HWPX has one page per fixture page and chunk N opens page N", () => {
  const hwpx = buildFixtureHwpx(
    hancomBytes("hwpx"),
    FIXTURE_PAGES,
  );
  assert.equal(readZip(hwpx)[0]?.name, "mimetype");
  const doc = new HwpDocument(hwpx);
  try {
    const pages = pagesOf(doc);
    assert.deepEqual(pages, FIXTURE_PAGES.map((text) => `${text}\n`));
    assert.deepEqual([0, 1, 2].map((chunk) => pageOfChunk(pages, chunk)), [0, 1, 2]);
    for (let page = 0; page < pages.length; page += 1) assertInertSvg(doc.renderPageSvg(page));
    // The binary HWP 5.0 form of the same document keeps the pages.
    const hwp = new HwpDocument(doc.exportHwp());
    try {
      assert.deepEqual(pagesOf(hwp), pages);
    } finally {
      hwp.free();
    }
  } finally {
    doc.free();
  }
});
