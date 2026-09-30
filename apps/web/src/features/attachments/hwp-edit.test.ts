import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import { HwpDocument, initSync } from "@rhwp/core";
import { hwpExportFormat } from "./hwp-edit.ts";
import { decodePageText, HWP_MAX_BYTES } from "./hwp-page.ts";
import { createHwpSession, type HwpRequest, type HwpResponse, type RhwpApi, type RhwpDocument } from "./hwp-worker-core.ts";
import { buildFixtureHwpx, FIXTURE_PAGES } from "./hwp-test-fixture.ts";

const require = createRequire(import.meta.url);
const wasm = fs.readFileSync(path.join(path.dirname(require.resolve("@rhwp/core")), "rhwp_bg.wasm"));
const module = new WebAssembly.Module(wasm);
const repoRoot = path.resolve(import.meta.dirname, "../../../../..");
const fixture = (name: string) => new Uint8Array(fs.readFileSync(path.join(repoRoot, "compat/fixtures", name)));

const realApi: RhwpApi = {
  init: async (m) => void initSync({ module: m }),
  open: (bytes) => new HwpDocument(bytes),
};

type Session = (request: HwpRequest) => Promise<HwpResponse>;
let nextId = 0;
const call = (session: Session, body: Record<string, unknown>) =>
  session({ id: (nextId += 1), ...body } as HwpRequest);

async function openSession(bytes: Uint8Array, api: RhwpApi = realApi): Promise<Session> {
  const session = createHwpSession(api);
  const opened = await call(session, { op: "open", bytes, module });
  assert.equal(opened.ok, true);
  return session;
}

async function replace(session: Session, find: string, replacement: string, all: boolean) {
  const response = await call(session, { op: "replace", find, replacement, all });
  assert.ok(response.ok && response.op === "replace", JSON.stringify(response));
  return response.outcome;
}

async function exported(session: Session, format: "hwp" | "hwpx"): Promise<Uint8Array> {
  const response = await call(session, { op: "export", format });
  assert.ok(response.ok && response.op === "export", JSON.stringify(response));
  return response.bytes;
}

/** The pages of `bytes` as a fresh rhwp document reads them (what reopening the copy shows). */
function reopen(bytes: Uint8Array): string[] {
  const doc = new HwpDocument(bytes);
  try {
    return Array.from({ length: doc.pageCount() }, (_, i) => decodePageText(doc.getPageText(i)));
  } finally {
    doc.free();
  }
}

const occurrences = (text: string, word: string) => text.split(word).length - 1;
const pagesText = FIXTURE_PAGES.map((text) => `${text}\n`);

test("export format follows the file name, not the MIME type", () => {
  assert.deepEqual(hwpExportFormat("보고서.hwpx"), { format: "hwpx", mime: "application/hwpx" });
  assert.deepEqual(hwpExportFormat("REPORT.HWPX"), { format: "hwpx", mime: "application/hwpx" });
  assert.deepEqual(hwpExportFormat("보고서.hwp"), { format: "hwp", mime: "application/x-hwp" });
  assert.deepEqual(hwpExportFormat("hwpx"), { format: "hwp", mime: "application/x-hwp" });
  assert.deepEqual(hwpExportFormat("a.hwpx.hwp"), { format: "hwp", mime: "application/x-hwp" });
});

test("HWPX: replace all on one page, export as HWPX, reopen: only that page changed", async () => {
  const session = await openSession(buildFixtureHwpx(fixture("sample.hwpx"), FIXTURE_PAGES));
  const hits = occurrences(FIXTURE_PAGES[1]!, "하늘과");
  assert.ok(hits > 1);
  assert.equal(await replace(session, "하늘과", "구름과", true), "changed");
  const bytes = await exported(session, "hwpx");
  assert.deepEqual([...bytes.subarray(0, 4)], [0x50, 0x4b, 0x03, 0x04]);
  const pages = reopen(bytes);
  assert.deepEqual(pages, [pagesText[0], pagesText[1]!.replaceAll("하늘과", "구름과"), pagesText[2]]);
  assert.equal(occurrences(pages[1]!, "구름과"), hits);
  // The copy is itself a document the viewer opens.
  const again = createHwpSession(realApi);
  assert.deepEqual(await call(again, { op: "open", bytes, module }), { id: nextId, ok: true, op: "open", pageCount: 3 });
});

test("HWP: replace one and replace all, export as HWP 5.0, reopen", async () => {
  const session = await openSession(fixture("sample.hwp"));
  assert.equal(await replace(session, "안", "잘", false), "changed");
  const bytes = await exported(session, "hwp");
  // HWP 5.0 is an OLE compound file.
  assert.deepEqual([...bytes.subarray(0, 4)], [0xd0, 0xcf, 0x11, 0xe0]);
  assert.deepEqual(reopen(bytes), ["잘녕\n"]);

  // A three-page binary HWP source keeps its other pages.
  const hwp = new HwpDocument(buildFixtureHwpx(fixture("sample.hwpx"), FIXTURE_PAGES));
  const source = hwp.exportHwp();
  hwp.free();
  const multi = await openSession(source);
  assert.equal(await replace(multi, "백두산이", "한라산이", true), "changed");
  assert.deepEqual(reopen(await exported(multi, "hwp")), [
    pagesText[0],
    pagesText[1],
    pagesText[2]!.replaceAll("백두산이", "한라산이"),
  ]);
});

test("no match changes nothing: replaceAll count 0 and a refused replaceOne", async () => {
  const session = await openSession(fixture("sample.hwpx"));
  assert.equal(await replace(session, "없는문자열", "X", true), "unchanged");
  // rhwp 0.8.6 answers {"ok":false} to a replaceOne with no match.
  assert.equal(await replace(session, "없는문자열", "X", false), "rejected");
  assert.equal(await replace(session, "", "X", true), "unchanged");
  assert.deepEqual(reopen(await exported(session, "hwpx")), ["안녕\n"]);
});

test("revert re-parses the original and drops every edit", async () => {
  const session = await openSession(buildFixtureHwpx(fixture("sample.hwpx"), FIXTURE_PAGES));
  assert.equal(await replace(session, "첫째", "처음", true), "changed");
  assert.equal(await replace(session, "셋째", "끝", false), "changed");
  assert.deepEqual(await call(session, { op: "revert" }), { id: nextId, ok: true, op: "revert", pageCount: 3 });
  assert.deepEqual(reopen(await exported(session, "hwpx")), pagesText);
  // Editing goes on from the original.
  assert.equal(await replace(session, "둘째", "두번째", false), "changed");
  assert.equal(reopen(await exported(session, "hwpx"))[1], pagesText[1]!.replace("둘째", "두번째"));
});

/** A stand-in document whose edit and export answers a test controls. */
function stubApi(doc: Partial<RhwpDocument>, opens: { count: number; fail?: number } = { count: 0 }): RhwpApi {
  return {
    init: async () => undefined,
    open: () => {
      opens.count += 1;
      if (opens.fail === opens.count) throw new Error("trap");
      return {
        pageCount: () => 2,
        getPageText: () => '""',
        renderPageSvg: () => "<svg/>",
        replaceOne: () => '{"ok":true}',
        replaceAll: () => '{"ok":true,"count":1}',
        exportHwp: () => new Uint8Array([1]),
        exportHwpx: () => new Uint8Array([2]),
        free: () => undefined,
        ...doc,
      };
    },
  };
}

test("malformed mutation answers are refused without an edit, and the document stays usable", async () => {
  for (const raw of ["not-json", "[]", '{"ok":true,"count":-1}', '{"ok":true,"count":"2"}']) {
    const session = await openSession(new Uint8Array([9]), stubApi({ replaceAll: () => raw }));
    assert.equal(await replace(session, "a", "b", true), "rejected", raw);
    assert.equal((await exported(session, "hwp")).length, 1, raw);
  }
});

test("a replace that traps drops the half-edited document: nothing renders, exports or reverts after it", async () => {
  // The stand-in applies part of its edit before trapping, as a wasm panic mid-replace can.
  let text = "original";
  const exports: string[] = [];
  let freed = 0;
  const session = await openSession(
    new Uint8Array([9]),
    stubApi({
      replaceAll: () => {
        text = "half-edited";
        throw new Error("unreachable");
      },
      exportHwp: () => {
        exports.push(text);
        return new TextEncoder().encode(text);
      },
      free: () => void (freed += 1),
    }),
  );
  assert.deepEqual(await call(session, { op: "replace", find: "a", replacement: "b", all: true }), {
    id: nextId,
    ok: false,
    error: "failed",
  });
  assert.equal(freed, 1);
  for (const body of [
    { op: "export", format: "hwp" },
    { op: "render", page: 0 },
    { op: "revert" },
    { op: "replace", find: "a", replacement: "b", all: false },
  ]) {
    assert.deepEqual(await call(session, body), { id: nextId, ok: false, error: "failed" }, body.op);
  }
  assert.deepEqual(exports, []);
});

test("an export that throws, is empty, or exceeds the viewer's budget is refused", async () => {
  const throwing = await openSession(
    new Uint8Array([9]),
    stubApi({
      exportHwp: () => {
        throw new Error("serialize");
      },
    }),
  );
  assert.deepEqual(await call(throwing, { op: "export", format: "hwp" }), { id: nextId, ok: false, error: "failed" });
  const empty = await openSession(new Uint8Array([9]), stubApi({ exportHwpx: () => new Uint8Array(0) }));
  assert.deepEqual(await call(empty, { op: "export", format: "hwpx" }), { id: nextId, ok: false, error: "failed" });
  const huge = await openSession(new Uint8Array([9]), stubApi({ exportHwpx: () => new Uint8Array(HWP_MAX_BYTES + 1) }));
  assert.deepEqual(await call(huge, { op: "export", format: "hwpx" }), { id: nextId, ok: false, error: "tooLarge" });
  const exact = await openSession(new Uint8Array([9]), stubApi({ exportHwp: () => new Uint8Array(HWP_MAX_BYTES) }));
  const ok = await call(exact, { op: "export", format: "hwp" });
  assert.ok(ok.ok && ok.op === "export" && ok.bytes.byteLength === HWP_MAX_BYTES);
});

test("a revert whose re-parse fails keeps the edited document", async () => {
  const opens = { count: 0, fail: 2 };
  let freed = 0;
  const session = await openSession(new Uint8Array([9]), stubApi({ free: () => void (freed += 1) }, opens));
  assert.deepEqual(await call(session, { op: "revert" }), { id: nextId, ok: false, error: "failed" });
  assert.equal(freed, 0);
  assert.equal(await replace(session, "a", "b", true), "changed");
  // Before a document is open nothing is edited or exported.
  const empty = createHwpSession(realApi);
  for (const body of [{ op: "revert" }, { op: "export", format: "hwp" }, { op: "replace", find: "a", replacement: "b", all: true }]) {
    assert.deepEqual(await call(empty, body), { id: nextId, ok: false, error: "failed" });
  }
});
