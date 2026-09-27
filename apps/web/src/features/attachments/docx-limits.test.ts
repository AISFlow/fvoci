import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { deflateRawSync } from "node:zlib";
import JSZip from "jszip";
import { checkDocxPackage } from "./docx-limits.ts";
import { crc32, writeZip, type ZipEntry } from "./docx-test-fixture.ts";

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

// --- Nothing inflates the package outside the caps ---------------------------

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

test("a CRC mismatch alone is not a failure: CRC is not an authentication check", async () => {
  // The renderer's own load (JSZip defaults) never checks CRC either.
  const bad = await recordLoads(() => checkDocxPackage(badCrcZip(1024), alive));
  assert.equal(bad.result, "ok");
  assert.deepEqual(bad.crcLoads, [false]);
});

test("malformed DEFLATE data still fails the check", async () => {
  // BTYPE 11 is reserved in RFC 1951; the capped stream reports the error.
  const bytes = writeZip([{ name: "word/document.xml", deflated: new Uint8Array([0xff, 0xff, 0xff]), crc: 0, size: 64 }]);
  assert.equal(await checkDocxPackage(bytes, alive), "invalid");
});

// --- Records shadowed in the renderer's view are neither budgeted nor inflated

const flate = createRequire(import.meta.url)("jszip/lib/flate") as { uncompressWorker: () => unknown };

/** Counts every byte any JSZip DEFLATE stream produces while `run` is pending. */
async function countInflated<T>(run: () => Promise<T>): Promise<{ result: T; inflated: number }> {
  const original = flate.uncompressWorker;
  let inflated = 0;
  flate.uncompressWorker = () => {
    const worker = original() as { push(chunk: { data: Uint8Array }): unknown };
    const push = worker.push;
    worker.push = function (chunk) {
      inflated += chunk.data.length;
      return push.call(this, chunk);
    };
    return worker;
  };
  try {
    return { result: await run(), inflated };
  } finally {
    flate.uncompressWorker = original;
  }
}

/** A raw-DEFLATE part of `size` zero bytes with its correct CRC. */
function zeros(name: string, size: number): ZipEntry {
  const data = Buffer.alloc(size);
  return { name, deflated: deflateRawSync(data), crc: crc32(data), size };
}

/** What docx-preview inflates: its own default load, then every part. */
async function renderView(bytes: Uint8Array): Promise<{ names: string[]; inflated: number }> {
  const { result: names, inflated } = await countInflated(async () => {
    const zip = await JSZip.loadAsync(bytes);
    const parts = Object.values(zip.files).filter((entry) => !entry.dir);
    for (const part of parts) await part.async("uint8array");
    return parts.map((part) => part.name);
  });
  return { names, inflated };
}

const BOMB = 4 * 1024 * 1024;
const CAP = 1024 * 1024;
const TINY = 13;

const shadowed: [string, ZipEntry[]][] = [
  ["a duplicate name (earlier record shadowed)", [zeros("word/document.xml", BOMB), zeros("word/document.xml", TINY)]],
  ["a normalised path collision", [zeros("x/../word/document.xml", BOMB), zeros("word/document.xml", TINY)]],
  ["a directory record with a payload", [zeros("hidden/", BOMB), zeros("word/document.xml", TINY)]],
];

for (const [label, entries] of shadowed) {
  test(`${label} is never inflated by the check or the renderer`, async () => {
    const bytes = writeZip(entries);
    const checked = await countInflated(() => recordLoads(() => checkDocxPackage(bytes, alive, CAP)));
    assert.equal(checked.inflated, TINY);
    assert.equal(checked.result.result, "ok");
    assert.deepEqual(checked.result.crcLoads, [false]);
    assert.deepEqual(await renderView(bytes), { names: ["word/document.xml"], inflated: TINY });
  });
}

test("the visible record of a duplicate name is the one budgeted", async () => {
  const bytes = writeZip([zeros("word/document.xml", TINY), zeros("word/document.xml", BOMB)]);
  const checked = await countInflated(() => checkDocxPackage(bytes, alive, CAP));
  assert.equal(checked.result, "tooLarge");
  // The stop overshoots by at most one 16 KiB compressed block's output (DEFLATE ≤ ~1032:1).
  assert.ok(checked.inflated <= CAP + 1032 * 16 * 1024, `inflated ${checked.inflated}`);
});

test("the part-count cap counts logical parts; shadowed duplicates add no inflation", async () => {
  const part = 64 * 1024;
  const bytes = writeZip(Array.from({ length: 50 }, () => zeros("a.xml", part)));
  const checked = await countInflated(() => checkDocxPackage(bytes, alive, 2 * part, 10));
  assert.equal(checked.result, "ok");
  assert.equal(checked.inflated, part);
  assert.deepEqual(await renderView(bytes), { names: ["a.xml"], inflated: part });
  // Distinct names are logical parts and do hit the cap.
  const distinct = writeZip(Array.from({ length: 50 }, (_, i) => zeros(`p${i}.xml`, TINY)));
  assert.equal(await checkDocxPackage(distinct, alive, 2 * part, 10), "tooLarge");
});

test("cancellation before the check inflates nothing; mid-stream cancellation stops the stream", async () => {
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
