import assert from "node:assert/strict";
import test from "node:test";
import { deflateRawSync, crc32 as zlibCrc32 } from "node:zlib";
import { getSlides, loadPresentation } from "@office-kit/pptx";
import { Zip, ZipDeflate, unzipSync } from "fflate";
import { crc32, writeZip } from "./docx-test-fixture.ts";
import { repackPptx } from "./pptx-limits.ts";
import { buildFixturePptx } from "./pptx-test-fixture.ts";

const MiB = 1024 * 1024;
const alive = () => true;

/** Central-directory view of the renderer's own reader, inflating nothing. */
function directory(bytes: Uint8Array) {
  const records: { name: string; size: number; originalSize: number; compression: number }[] = [];
  unzipSync(bytes, {
    filter: (file) => {
      records.push({ ...file });
      return false;
    },
  });
  return records;
}

function fixtureParts(): Record<string, Uint8Array> {
  return unzipSync(buildFixturePptx());
}

type RawRecord = { name: string; method: 0 | 8; body: Uint8Array; crc: number; size: number };

function u16(value: number) {
  return [value & 0xff, (value >>> 8) & 0xff];
}
function u32(value: number) {
  return [value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff];
}

function localHeader(record: RawRecord): Uint8Array {
  const name = new TextEncoder().encode(record.name);
  return new Uint8Array([
    ...u32(0x04034b50), ...u16(20), ...u16(0x0800), ...u16(record.method), ...u16(0), ...u16(0x5b21),
    ...u32(record.crc), ...u32(record.body.byteLength), ...u32(record.size), ...u16(name.byteLength), ...u16(0),
    ...name,
  ]);
}

function centralRecord(record: RawRecord, offset: number): Uint8Array {
  const name = new TextEncoder().encode(record.name);
  return new Uint8Array([
    ...u32(0x02014b50), ...u16(20), ...u16(20), ...u16(0x0800), ...u16(record.method), ...u16(0), ...u16(0x5b21),
    ...u32(record.crc), ...u32(record.body.byteLength), ...u32(record.size), ...u16(name.byteLength), ...u16(0),
    ...u16(0), ...u16(0), ...u16(0), ...u32(0), ...u32(offset),
    ...name,
  ]);
}

function join(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.byteLength, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.byteLength;
  }
  return out;
}

/**
 * The fixture's parts as stored entries, one more stored "carrier" picture
 * whose bytes are a complete local header + DEFLATE bomb, and `hidden`
 * central-directory records that all point into the carrier at that
 * embedded header. The local-header sequence never contains the bomb.
 */
function packageWithHiddenRecords(hidden: number, bombBytes: number) {
  const inflated = new Uint8Array(bombBytes);
  const bomb: RawRecord = {
    name: "ppt/hidden.xml",
    method: 8,
    body: deflateRawSync(inflated),
    crc: zlibCrc32(inflated),
    size: inflated.byteLength,
  };
  const carrierData = join([localHeader(bomb), bomb.body]);
  const records: RawRecord[] = [
    ...Object.entries(fixtureParts()).map(([name, body]) => ({ name, method: 0 as const, body, crc: crc32(body), size: body.byteLength })),
    { name: "ppt/media/carrier.png", method: 0, body: carrierData, crc: crc32(carrierData), size: carrierData.byteLength },
  ];
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;
  let carrierDataOffset = 0;
  for (const record of records) {
    const header = localHeader(record);
    centrals.push(centralRecord(record, offset));
    if (record.name === "ppt/media/carrier.png") carrierDataOffset = offset + header.byteLength;
    locals.push(header, record.body);
    offset += header.byteLength + record.body.byteLength;
  }
  for (let i = 0; i < hidden; i += 1) {
    centrals.push(centralRecord({ ...bomb, name: `ppt/hidden-${i}.xml` }, carrierDataOffset));
  }
  const cd = join(centrals);
  const count = records.length + hidden;
  const end = new Uint8Array([
    ...u32(0x06054b50), ...u16(0), ...u16(0), ...u16(count), ...u16(count), ...u32(cd.byteLength), ...u32(offset), ...u16(0),
  ]);
  return { bytes: join([...locals, cd, end]), names: records.map((r) => r.name) };
}

test("the fixture repacks STORE-only with identical parts and still loads two slides", async () => {
  const original = buildFixturePptx();
  const result = await repackPptx(original, alive);
  assert.equal(result.status, "ok");
  if (result.status !== "ok") return;
  const records = directory(result.bytes);
  assert.equal(records.length, 14);
  for (const record of records) {
    assert.equal(record.compression, 0, record.name);
    assert.equal(record.size, record.originalSize, record.name);
  }
  const before = unzipSync(original);
  const after = unzipSync(result.bytes);
  assert.deepEqual(Object.keys(after).sort(), Object.keys(before).sort());
  for (const [name, data] of Object.entries(before)) assert.deepEqual(after[name], data, name);
  assert.equal(result.entries, 14);
  assert.equal(result.expanded, Object.values(before).reduce((n, d) => n + d.byteLength, 0));
  const pres = await loadPresentation(result.bytes);
  assert.equal(getSlides(pres).length, 2);
});

test("a DEFLATE bomb stops at the inflated-size cap", async () => {
  const inflated = new Uint8Array(200 * MiB);
  const bytes = writeZip([
    { name: "[Content_Types].xml", bytes: new TextEncoder().encode("<Types/>") },
    { name: "ppt/presentation.xml", deflated: deflateRawSync(inflated), crc: zlibCrc32(inflated), size: inflated.byteLength },
  ]);
  assert.ok(bytes.byteLength < MiB);
  assert.deepEqual(await repackPptx(bytes, alive), { status: "tooLarge" });
});

test("declared sizes are not trusted: a small declared size still counts real output", async () => {
  const inflated = new Uint8Array(2 * MiB);
  const bytes = writeZip([
    { name: "ppt/presentation.xml", deflated: deflateRawSync(inflated), crc: zlibCrc32(inflated), size: 16 },
  ]);
  assert.deepEqual(await repackPptx(bytes, alive, MiB), { status: "tooLarge" });
  assert.equal((await repackPptx(bytes, alive, 4 * MiB)).status, "ok");
});

test("hidden and overlapping central-directory records never reach the renderer", async () => {
  const { bytes, names } = packageWithHiddenRecords(20, 64 * MiB);
  // The loader's own reader would inflate every one of these records.
  const raw = directory(bytes);
  const hidden = raw.filter((r) => r.name.startsWith("ppt/hidden-"));
  assert.equal(hidden.length, 20);
  assert.equal(hidden.reduce((n, r) => n + r.originalSize, 0), 20 * 64 * MiB);

  const result = await repackPptx(bytes, alive);
  assert.equal(result.status, "ok");
  if (result.status !== "ok") return;
  const seen = directory(result.bytes);
  assert.deepEqual(seen.map((r) => r.name).sort(), [...names].sort());
  assert.ok(seen.every((r) => r.compression === 0));
  assert.ok(result.expanded < 2 * MiB);
  assert.equal(getSlides(await loadPresentation(result.bytes)).length, 2);
});

test("duplicate names (case-insensitive, as OPC part names) are rejected", async () => {
  const enc = (s: string) => new TextEncoder().encode(s);
  for (const second of ["ppt/slides/slide1.xml", "PPT/Slides/Slide1.xml"]) {
    const bytes = writeZip([
      { name: "ppt/slides/slide1.xml", bytes: enc("<a/>") },
      { name: second, bytes: enc("<b/>") },
    ]);
    assert.deepEqual(await repackPptx(bytes, alive), { status: "invalid" }, second);
  }
});

test("entry count cap, unknown compression, truncation and non-ZIP input", async () => {
  const fixture = buildFixturePptx();
  assert.deepEqual(await repackPptx(fixture, alive, undefined, 3), { status: "tooLarge" });

  const bzip = writeZip([{ name: "a.xml", bytes: new TextEncoder().encode("<a/>") }]);
  new DataView(bzip.buffer).setUint16(8, 12, true);
  assert.deepEqual(await repackPptx(bzip, alive), { status: "invalid" });

  assert.deepEqual(await repackPptx(fixture.subarray(0, Math.floor(fixture.byteLength / 2)), alive), {
    status: "invalid",
  });
  assert.deepEqual(await repackPptx(new TextEncoder().encode("not a zip at all"), alive), { status: "invalid" });
  assert.deepEqual(await repackPptx(new Uint8Array(0), alive), { status: "invalid" });
});

test("streamed ZIPs with data descriptors repack; directory entries are dropped", async () => {
  const chunks: Uint8Array[] = [];
  const zip = new Zip((error, data) => {
    if (error) throw error;
    chunks.push(data);
  });
  for (const [name, data] of Object.entries(fixtureParts())) {
    const entry = new ZipDeflate(name, { level: 6 });
    zip.add(entry);
    entry.push(data, true);
  }
  zip.end();
  const streamed = join(chunks);
  assert.ok(directory(streamed).every((r) => r.compression === 8));
  const result = await repackPptx(streamed, alive);
  assert.equal(result.status, "ok");
  if (result.status === "ok") assert.equal(getSlides(await loadPresentation(result.bytes)).length, 2);

  const withDir = writeZip([
    { name: "ppt/", bytes: new Uint8Array(0) },
    { name: "ppt/a.xml", bytes: new TextEncoder().encode("<a/>") },
  ]);
  const repacked = await repackPptx(withDir, alive);
  assert.equal(repacked.status, "ok");
  if (repacked.status === "ok") assert.deepEqual(Object.keys(unzipSync(repacked.bytes)), ["ppt/a.xml"]);
});

test("cancellation stops the read", async () => {
  const inflated = new Uint8Array(8 * MiB).fill(7);
  const bytes = writeZip([
    { name: "a.bin", deflated: deflateRawSync(inflated, { level: 0 }), crc: zlibCrc32(inflated), size: inflated.byteLength },
  ]);
  let calls = 0;
  assert.deepEqual(await repackPptx(bytes, () => (calls += 1) < 3), { status: "invalid" });
  assert.ok(calls <= 3 + 1);
  assert.deepEqual(await repackPptx(bytes, () => false), { status: "invalid" });
});

// XLSX review F1 counterexamples: the guard reads local headers only, so a
// broken central directory or a forged ZIP64 entry count changes nothing.
test("a malformed central directory does not bypass the inflated-size cap", async () => {
  const inflated = new Uint8Array(200 * MiB);
  const deflated = deflateRawSync(inflated);
  const bytes = writeZip([
    { name: "a.xml", deflated, crc: zlibCrc32(inflated), size: inflated.byteLength },
    { name: "b.xml", deflated, crc: zlibCrc32(inflated), size: inflated.byteLength },
  ]);
  const view = new DataView(bytes.buffer, bytes.byteOffset);
  const cd = view.getUint32(bytes.byteLength - 22 + 16, true);
  assert.equal(view.getUint32(cd, true), 0x02014b50);
  bytes[cd] = 0; // first central-directory signature byte flipped
  assert.deepEqual(await repackPptx(bytes, alive), { status: "tooLarge" });
});

test("a forged ZIP64 entry count is never walked", async () => {
  // ZIP64 end record: 0xFFFFFFFF entries, directory "at" offset 1000 (past the
  // end), so fflate's `unzipSync` would walk ~4e9 all-zero records (~140 ms per
  // million here) before failing.
  const zip64End = new Uint8Array([
    ...u32(0x06064b50), ...u32(44), ...u32(0), ...u16(45), ...u16(45), ...u32(0), ...u32(0),
    ...u32(0xffffffff), ...u32(0), ...u32(0xffffffff), ...u32(0), ...u32(0), ...u32(0), ...u32(1000), ...u32(0),
  ]);
  const locator = new Uint8Array([...u32(0x07064b50), ...u32(0), ...u32(0), ...u32(0), ...u32(1)]);
  const end = new Uint8Array([...u32(0x06054b50), ...u16(0), ...u16(0), ...u16(1), ...u16(1), ...u32(0), ...u32(0), ...u16(0)]);
  const bytes = join([zip64End, locator, end]);
  const started = Date.now();
  assert.deepEqual(await repackPptx(bytes, alive), { status: "invalid" });
  assert.ok(Date.now() - started < 2_000);
});
