import { assertPresent } from "./test-invariants.ts";
import assert from "node:assert/strict";
import test from "node:test";
import { chunkPlainText } from "./chunk-plain-text.ts";
import { clampPage, decodePageText, pageOfChunk, visiblePageCount } from "./hwp-page.ts";

await test("decodePageText unwraps rhwp's JSON string and normalizes line ends", () => {
  assert.equal(decodePageText('"안녕\\r\\n"'), "안녕\n");
  assert.equal(decodePageText("plain\r\ntext"), "plain\ntext");
  assert.equal(decodePageText("42"), "42");
});

await test("pageOfChunk opens the page where the chunk's new text begins", () => {
  // rhwp page text ends with a paragraph break; joined pages meet at "\n\n".
  const pages = ["첫째 ", "둘째 ", "셋째 "].map((head, i) => `${head}${String(i).repeat(1000)}\n`);
  const joined = pages.join("\n");
  const chunks = chunkPlainText(joined);
  assert.equal(chunks.length, 3);
  // The chunk start sits in the overlap copied from the previous page …
  assert.ok(assertPresent(chunks[1]).start < assertPresent(pages[0]).length);
  // … but the jump follows the chunk's own text.
  assert.equal(pageOfChunk(pages, 0), 0);
  assert.equal(pageOfChunk(pages, 1), 1);
  assert.equal(pageOfChunk(pages, 2), 2);
  for (const chunk of chunks.slice(1)) {
    const page = pageOfChunk(pages, chunk.chunkNo);
    assert.ok(
      joined
        .slice(assertPresent(chunks[chunk.chunkNo - 1]).end)
        .startsWith(assertPresent(pages[page]).slice(0, 3)),
    );
  }
});

await test("pageOfChunk opens the first page for unknown chunks and empty documents", () => {
  assert.equal(pageOfChunk(["한 쪽"], 5), 0);
  assert.equal(pageOfChunk([], 0), 0);
  assert.equal(pageOfChunk(["", ""], 0), 0);
});

await test("visiblePageCount and clampPage keep navigation inside the document", () => {
  assert.equal(visiblePageCount(0), 1);
  assert.equal(visiblePageCount(Number.NaN), 1);
  assert.equal(visiblePageCount(3), 3);
  assert.equal(clampPage(-1, 3), 0);
  assert.equal(clampPage(5, 3), 2);
  assert.equal(clampPage(1, 3), 1);
});
