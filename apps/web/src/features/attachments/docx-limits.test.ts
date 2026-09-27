import assert from "node:assert/strict";
import test from "node:test";
import JSZip from "jszip";
import { checkDocxPackage } from "./docx-limits.ts";

async function zip(files: Record<string, Uint8Array | string>): Promise<Uint8Array> {
  const archive = new JSZip();
  for (const [name, body] of Object.entries(files)) archive.file(name, body);
  return archive.generateAsync({ type: "uint8array", compression: "DEFLATE" });
}

const alive = () => true;

test("a package within the inflated-size cap passes", async () => {
  const bytes = await zip({ "word/document.xml": "<w:document/>", "[Content_Types].xml": "<Types/>" });
  assert.equal(await checkDocxPackage(bytes, alive, 1024), "ok");
});

test("a small ZIP that inflates past the cap is rejected before rendering", async () => {
  const bytes = await zip({ "word/document.xml": new Uint8Array(4 * 1024 * 1024) });
  assert.ok(bytes.byteLength < 64 * 1024, "fixture must be highly compressible");
  assert.equal(await checkDocxPackage(bytes, alive, 1024 * 1024), "tooLarge");
});

test("the cap is the total across parts, not per part", async () => {
  const part = new Uint8Array(600 * 1024);
  const bytes = await zip({ "a.xml": part, "b.xml": part });
  assert.equal(await checkDocxPackage(bytes, alive, 1024 * 1024), "tooLarge");
  assert.equal(await checkDocxPackage(bytes, alive, 2 * 1024 * 1024), "ok");
});

test("too many parts, non-ZIP bytes and a cancelled check are not rendered", async () => {
  const many: Record<string, string> = {};
  for (let i = 0; i < 12; i += 1) many[`p${i}.xml`] = "x";
  assert.equal(await checkDocxPackage(await zip(many), alive, 1024, 10), "tooLarge");
  assert.equal(await checkDocxPackage(new TextEncoder().encode("not a zip"), alive), "invalid");
  const bytes = await zip({ "word/document.xml": "<w:document/>" });
  assert.equal(await checkDocxPackage(bytes, () => false), "invalid");
});
