import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import { HwpDocument, initSync } from "@rhwp/core";
import { HWPX_MAX_EXPANDED_BYTES, prepareHwpBytes } from "./hwp-package.ts";
import { readZip, writeZip } from "./hwp-test-fixture.ts";

const require = createRequire(import.meta.url);
const coreDir = path.dirname(require.resolve("@rhwp/core"));
const repoRoot = path.resolve(import.meta.dirname, "../../../../..");
const fixture = (name: string) =>
  new Uint8Array(fs.readFileSync(path.join(repoRoot, "compat/fixtures", name)));
const MiB = 1024 * 1024;
const alive = () => true;

/** The Hancom sample plus extra records (the review's `Scripts/*` counterexample). */
function withExtra(extra: { name: string; data: Uint8Array; declaredSize?: number }[]): Uint8Array {
  return writeZip([...readZip(fixture("sample.hwpx")), ...extra]);
}

function names(bytes: Uint8Array): string[] {
  return readZip(bytes).map((entry) => entry.name);
}

await test("the review's small Scripts bomb is rejected before rhwp runs", async () => {
  // Four unreferenced 32 MiB scripts: about 140 KB on disk, about 324 MiB of
  // wasm heap when rhwp 0.8.6 opens it directly (review B1 table).
  const bomb = withExtra(
    [0, 1, 2, 3].map((n) => ({ name: `Scripts/s${String(n)}.js`, data: new Uint8Array(32 * MiB) })),
  );
  assert.ok(bomb.length < 256 * 1024);
  assert.equal(HWPX_MAX_EXPANDED_BYTES, 128 * MiB);
  assert.deepEqual(await prepareHwpBytes(bomb, alive), { status: "tooLarge" });
});

await test("rhwp itself opens an over-budget package; the check is what refuses it", async () => {
  initSync({ module: fs.readFileSync(path.join(coreDir, "rhwp_bg.wasm")) });
  const bomb = withExtra([{ name: "Scripts/s.js", data: new Uint8Array(8 * MiB) }]);
  const doc = new HwpDocument(bomb);
  try {
    assert.equal(doc.pageCount(), 1);
  } finally {
    doc.free();
  }
  assert.deepEqual(await prepareHwpBytes(bomb, alive, 4 * MiB), { status: "tooLarge" });
  const ok = await prepareHwpBytes(bomb, alive, 16 * MiB);
  assert.equal(ok.status, "ok");
});

await test("rhwp gets a re-written package holding only the measured parts", async () => {
  const forged = withExtra([
    // Declared empty: JSZip keeps no data, so the re-written part is empty too.
    { name: "Scripts/empty.js", data: new Uint8Array(8 * MiB), declaredSize: 0 },
    // A directory record carrying data.
    { name: "Scripts/dir/", data: new Uint8Array(8 * MiB) },
    // A shadowed duplicate: the last record of a name is the part (as in the zip crate).
    { name: "Scripts/dup.js", data: new Uint8Array(8 * MiB) },
    { name: "Scripts/dup.js", data: new TextEncoder().encode("last") },
  ]);
  const checked = await prepareHwpBytes(forged, alive, 1 * MiB);
  assert.equal(checked.status, "ok");
  const parts = readZip(checked.bytes);
  const scripts = parts.filter((part) => part.name.startsWith("Scripts/"));
  assert.deepEqual(
    scripts.map((part) => [part.name, part.data.byteLength]),
    [
      ["Scripts/empty.js", 0],
      ["Scripts/dir/", 0],
      ["Scripts/dup.js", 4],
    ],
  );
  assert.ok(parts.reduce((sum, part) => sum + part.data.byteLength, 0) < 1 * MiB);
  // The document parts are carried over byte for byte.
  const original = new Map(readZip(fixture("sample.hwpx")).map((part) => [part.name, part.data]));
  for (const part of parts.filter((p) => original.has(p.name))) {
    assert.deepEqual(part.data, original.get(part.name), part.name);
  }
});

await test("the part count is capped and malformed or cancelled packages are invalid", async () => {
  const sample = fixture("sample.hwpx");
  assert.deepEqual(
    await prepareHwpBytes(sample, alive, HWPX_MAX_EXPANDED_BYTES, names(sample).length - 1),
    {
      status: "tooLarge",
    },
  );
  assert.equal(
    (await prepareHwpBytes(sample, alive, HWPX_MAX_EXPANDED_BYTES, names(sample).length)).status,
    "ok",
  );
  const truncated = sample.slice(0, sample.length - 30);
  assert.deepEqual(await prepareHwpBytes(truncated, alive), { status: "invalid" });
  assert.deepEqual(await prepareHwpBytes(sample, () => false), { status: "invalid" });
});

await test("non-ZIP formats (HWP 5) are passed to rhwp unchanged", async () => {
  const hwp = fixture("sample.hwp");
  const checked = await prepareHwpBytes(hwp, alive);
  assert.equal(checked.status, "ok");
  assert.equal(checked.bytes, hwp);
});
