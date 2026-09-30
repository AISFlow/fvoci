import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { getDocument } from "pdfjs-dist/legacy/build/pdf.mjs";
import { isPdfjsAsset, PDFJS_ASSET_DIRS, pdfjsAssetBase } from "./pdf-assets.ts";
import { buildFixturePdf } from "./pdf-test-fixture.ts";

const pkgDir = path.dirname(createRequire(import.meta.url).resolve("pdfjs-dist/package.json"));

function shipped(dir: (typeof PDFJS_ASSET_DIRS)[number]): string[] {
  return fs.readdirSync(path.join(pkgDir, dir)).filter((name) => isPdfjsAsset(dir, name));
}

test("pdfjsAssetBase is versioned and rejects odd versions", () => {
  assert.equal(pdfjsAssetBase("6.3.289"), "assets/pdfjs-dist-6.3.289/");
  assert.throws(() => pdfjsAssetBase("../6"));
});

test("shipped pdf.js data covers every CMap, standard font, decoder and license, not scripting", () => {
  const cmaps = fs.readdirSync(path.join(pkgDir, "cmaps"));
  assert.deepEqual(shipped("cmaps").sort(), cmaps.sort());
  assert.ok(shipped("cmaps").includes("UniKS-UCS2-H.bcmap"));
  assert.ok(shipped("cmaps").includes("Adobe-Korea1-UCS2.bcmap"));
  assert.deepEqual(
    shipped("standard_fonts").sort(),
    fs.readdirSync(path.join(pkgDir, "standard_fonts")).sort(),
  );
  const wasm = shipped("wasm");
  for (const name of [
    "jbig2.wasm",
    "openjpeg.wasm",
    "qcms_bg.wasm",
    "jbig2_nowasm_fallback.js",
    "openjpeg_nowasm_fallback.js",
    "LICENSE_OPENJPEG",
  ]) {
    assert.ok(wasm.includes(name), name);
  }
  assert.ok(!wasm.some((name) => name.startsWith("quickjs")));
  assert.deepEqual(shipped("iccs").sort(), fs.readdirSync(path.join(pkgDir, "iccs")).sort());
});

async function koreanText(options: { cMapUrl?: string }): Promise<string> {
  const data = buildFixturePdf([{ text: "한글 문서", script: "korean", band: "top-red" }]);
  const task = getDocument({ data, verbosity: 0, ...options });
  try {
    const doc = await task.promise;
    const content = await (await doc.getPage(1)).getTextContent();
    return content.items.map((item) => ("str" in item ? item.str : "")).join("");
  } finally {
    await task.destroy();
  }
}

test("non-embedded Korean text needs the shipped packed CMaps", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fvoci-pdfjs-cmaps-"));
  try {
    for (const name of shipped("cmaps")) {
      fs.copyFileSync(path.join(pkgDir, "cmaps", name), path.join(dir, name));
    }
    assert.equal(await koreanText({ cMapUrl: `${dir}/` }), "한글 문서");
    assert.equal(await koreanText({}), "");
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
