import assert from "node:assert/strict";
import test from "node:test";
import { inflateSync } from "node:zlib";
import JSZip from "jszip";
import { checkDocxPackage } from "./docx-limits.ts";
import { buildFixtureDocx, crc32, DEFAULT_DOCX_TEXT, solidPng } from "./docx-test-fixture.ts";

test("the fixture ZIP opens with CRC checks in an independent reader", async () => {
  const bytes = buildFixtureDocx();
  const zip = await JSZip.loadAsync(bytes, { checkCRC32: true });
  const names = Object.keys(zip.files).sort();
  assert.deepEqual(names, [
    "[Content_Types].xml",
    "_rels/.rels",
    "word/_rels/document.xml.rels",
    "word/document.xml",
    "word/media/blue.png",
    "word/numbering.xml",
    "word/styles.xml",
  ]);
  const document = await zip.file("word/document.xml")!.async("string");
  for (const text of [DEFAULT_DOCX_TEXT.heading, DEFAULT_DOCX_TEXT.body.trim(), DEFAULT_DOCX_TEXT.secondPage]) {
    assert.ok(document.includes(text), text);
  }
  assert.match(document, /<w:br w:type="page"\/>/);
  assert.equal(await checkDocxPackage(bytes, () => true), "ok");
});

test("the fixture PNG has valid chunk CRCs and a decodable image stream", () => {
  assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);
  const png = solidPng(4, 2, [0, 0, 255]);
  const view = new DataView(png.buffer, png.byteOffset);
  let at = 8;
  const types: string[] = [];
  let idat = new Uint8Array(0);
  while (at < png.byteLength) {
    const length = view.getUint32(at);
    const type = new TextDecoder().decode(png.subarray(at + 4, at + 8));
    const data = png.subarray(at + 8, at + 8 + length);
    assert.equal(view.getUint32(at + 8 + length), crc32(png.subarray(at + 4, at + 8 + length)), type);
    if (type === "IDAT") idat = data;
    types.push(type);
    at += 12 + length;
  }
  assert.deepEqual(types, ["IHDR", "IDAT", "IEND"]);
  const raw = inflateSync(idat);
  assert.equal(raw.byteLength, 2 * (1 + 4 * 3));
  assert.deepEqual([...raw.subarray(1, 4)], [0, 0, 255]);
});
