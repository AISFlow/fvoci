import assert from "node:assert/strict";
import test from "node:test";
import { deflateRawSync } from "node:zlib";
import JSZip from "jszip";
import { checkDocxPackage } from "./docx-limits.ts";
import { writeZip } from "./docx-test-fixture.ts";

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

// --- Bounded order: nothing inflates the whole package before the caps ------

/** One raw-DEFLATE part of `size` zero bytes with a deliberately wrong CRC. */
function badCrcZip(size: number): Uint8Array {
  const data = Buffer.alloc(size);
  return writeZip([{ name: "word/document.xml", deflated: deflateRawSync(data), crc: 0x12345678, size }]);
}

/** Records the `checkCRC32` option of every JSZip.loadAsync call made by `run`. */
async function recordLoads<T>(run: () => Promise<T>): Promise<{ result: T; crcLoads: boolean[] }> {
  const original = JSZip.loadAsync;
  const crcLoads: boolean[] = [];
  JSZip.loadAsync = ((data: Parameters<typeof original>[0], options?: JSZip.JSZipLoadOptions) => {
    crcLoads.push(options?.checkCRC32 === true);
    return original.call(JSZip, data, options);
  }) as typeof original;
  try {
    return { result: await run(), crcLoads };
  } finally {
    JSZip.loadAsync = original;
  }
}

test("negative control: JSZip's CRC load inflates a whole part before any cap can apply", async () => {
  // This is the order the previous check used: the mismatch is only found after full inflation.
  await assert.rejects(JSZip.loadAsync(badCrcZip(4 * 1024 * 1024), { checkCRC32: true }), /CRC32 mismatch/);
});

test("the inflated-size cap is enforced before, and instead of, the whole-package CRC pass", async () => {
  const { result, crcLoads } = await recordLoads(() =>
    checkDocxPackage(badCrcZip(4 * 1024 * 1024), alive, 1024 * 1024),
  );
  // A CRC prepass would have answered "invalid"; the capped stream answers first.
  assert.equal(result, "tooLarge");
  assert.deepEqual(crcLoads, [false]);
});

test("the part-count cap is enforced from the central directory alone", async () => {
  const many: Record<string, string> = {};
  for (let i = 0; i < 12; i += 1) many[`p${i}.xml`] = "x";
  const bytes = await zip(many);
  const counted = await recordLoads(() => checkDocxPackage(bytes, alive, 1024, 10));
  assert.equal(counted.result, "tooLarge");
  assert.deepEqual(counted.crcLoads, [false]);
});

test("integrity is still checked once the package is bounded", async () => {
  const bad = await recordLoads(() => checkDocxPackage(badCrcZip(1024), alive));
  assert.equal(bad.result, "invalid");
  assert.deepEqual(bad.crcLoads, [false, true]);
  const good = await recordLoads(async () => checkDocxPackage(await zip({ "a.xml": "<a/>" }), alive));
  assert.equal(good.result, "ok");
  assert.deepEqual(good.crcLoads, [false, true]);
});

test("cancellation before the check inflates nothing; mid-stream cancellation skips the CRC pass", async () => {
  const early = await recordLoads(() => checkDocxPackage(badCrcZip(4 * 1024 * 1024), () => false));
  assert.equal(early.result, "invalid");
  assert.deepEqual(early.crcLoads, []);

  let chunks = 0;
  const bytes = await zip({ "word/document.xml": new Uint8Array(2 * 1024 * 1024) });
  const late = await recordLoads(() =>
    checkDocxPackage(bytes, () => {
      chunks += 1;
      return chunks < 4;
    }),
  );
  assert.equal(late.result, "invalid");
  assert.deepEqual(late.crcLoads, [false]);
});
