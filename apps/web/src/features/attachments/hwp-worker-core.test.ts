import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import { HwpDocument, initSync } from "@rhwp/core";
import { createHwpSession, type HwpRequest, type RhwpApi } from "./hwp-worker-core.ts";
import { buildFixtureHwpx, FIXTURE_PAGES, hancomBytes, readZip, writeZip } from "./hwp-test-fixture.ts";

const require = createRequire(import.meta.url);
const wasm = fs.readFileSync(path.join(path.dirname(require.resolve("@rhwp/core")), "rhwp_bg.wasm"));
const module = new WebAssembly.Module(wasm);

/** The worker's rhwp binding under Node, counting parses. */
function realApi(): RhwpApi & { opened: number } {
  const api = {
    opened: 0,
    init: async (m: WebAssembly.Module) => {
      initSync({ module: m });
    },
    open: (bytes: Uint8Array) => {
      api.opened += 1;
      return new HwpDocument(bytes);
    },
  };
  return api;
}

const open = (id: number, bytes: Uint8Array): HwpRequest => ({ id, op: "open", bytes, module });

test("a session opens, finds the chunk's page and renders inert SVG blobs", async () => {
  const session = createHwpSession(realApi());
  const hwpx = buildFixtureHwpx(hancomBytes("hwpx"), FIXTURE_PAGES);
  assert.deepEqual(await session(open(1, hwpx)), { id: 1, ok: true, op: "open", pageCount: 3 });
  for (const chunk of [0, 1, 2]) {
    assert.deepEqual(await session({ id: 2, op: "startPage", chunk }), { id: 2, ok: true, op: "startPage", page: chunk });
  }
  assert.deepEqual(await session({ id: 3, op: "startPage", chunk: 9 }), { id: 3, ok: true, op: "startPage", page: 0 });
  const rendered = await session({ id: 4, op: "render", page: 7 });
  assert.equal(rendered.ok && rendered.op, "render");
  if (!rendered.ok || rendered.op !== "render") return;
  assert.equal(rendered.svg.type, "image/svg+xml");
  const svg = await rendered.svg.text();
  assert.match(svg, /^<svg[\s>]/);
  assert.doesNotMatch(svg, /<script|<foreignObject|\son[a-z]+=|javascript:/i);
  // A second open in the same worker is refused: one worker, one document.
  assert.deepEqual(await session(open(5, hwpx)), { id: 5, ok: false, error: "failed" });
});

test("binary HWP opens in a session", async () => {
  const session = createHwpSession(realApi());
  assert.deepEqual(await session(open(1, hancomBytes("hwp"))), { id: 1, ok: true, op: "open", pageCount: 1 });
});

test("an over-budget HWPX never reaches rhwp; undecodable bytes are invalid", async () => {
  const api = realApi();
  const bomb = writeZip([
    ...readZip(hancomBytes("hwpx")),
    ...[0, 1, 2, 3].map((n) => ({ name: `Scripts/s${n}.js`, data: new Uint8Array(32 * 1024 * 1024) })),
  ]);
  assert.deepEqual(await createHwpSession(api)(open(1, bomb)), { id: 1, ok: false, error: "tooLarge" });
  assert.equal(api.opened, 0);
  const garbage = new Uint8Array([0xd0, 0xcf, 0x11, 0xe0, 1, 2, 3, 4, 5, 6, 7, 8]);
  assert.deepEqual(await createHwpSession(api)(open(2, garbage)), { id: 2, ok: false, error: "invalid" });
});

test("requests before a document and a failing wasm init report failed", async () => {
  const session = createHwpSession(realApi());
  assert.deepEqual(await session({ id: 1, op: "render", page: 0 }), { id: 1, ok: false, error: "failed" });
  const broken = createHwpSession({
    init: async () => {
      throw new Error("no wasm");
    },
    open: () => {
      throw new Error("unreachable");
    },
  });
  assert.deepEqual(await broken(open(2, hancomBytes("hwp"))), { id: 2, ok: false, error: "failed" });
});
