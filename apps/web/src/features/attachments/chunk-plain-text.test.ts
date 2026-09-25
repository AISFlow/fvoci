import assert from "node:assert/strict";
import test from "node:test";
import { chunkPlainText } from "./chunk-plain-text.ts";

test("chunkPlainText splits on paragraph boundaries near the target size", () => {
  const para = "가".repeat(900);
  const text = `${para}\n\n${para}\n\n${para}`;
  const chunks = chunkPlainText(text);
  assert.ok(chunks.length >= 2);
  assert.equal(chunks[0]?.chunkNo, 0);
  assert.equal(text.slice(chunks[0]!.start, chunks[0]!.end), chunks[0]!.text);
});

test("chunkPlainText uses UTF-16 string offsets", () => {
  const text = "a👍b";
  const chunks = chunkPlainText(text);
  assert.equal(chunks.length, 1);
  assert.equal(chunks[0]?.end, text.length);
});
