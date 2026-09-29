import assert from "node:assert/strict";
import test from "node:test";
import { once } from "node:events";
import { Worker } from "node:worker_threads";
import { createDeflateRaw } from "node:zlib";
import { writeZip } from "./docx-test-fixture.ts";
import { XLSX_OPEN_TIMEOUT_MS, XLSX_PAGE_TIMEOUT_MS } from "./xlsx-limits.ts";
import {
  openXlsxInWorker,
  XlsxWorkerError,
  type XlsxWorkerPort,
  type XlsxWorkerRequest,
  type XlsxWorkerResponse,
} from "./xlsx-client.ts";
import { buildFixtureXlsx, gridSheet } from "./xlsx-test-fixture.ts";
import { checkXlsxPackage } from "./xlsx-workbook.ts";

// --- The real xlsx-worker.ts, run in a node worker thread --------------------

const WORKER_URL = new URL("./xlsx-worker.ts", import.meta.url).href;
/**
 * Gives the worker module a `self` like a dedicated worker's, loads it, then
 * posts a harness-only "booted" message (never forwarded to the client).
 */
const BOOT = `
import { parentPort } from "node:worker_threads";
globalThis.self = {
  postMessage: (message) => parentPort.postMessage(message),
  set onmessage(handler) { parentPort.on("message", (data) => handler({ data })); },
};
await import(${JSON.stringify(WORKER_URL)});
parentPort.postMessage("booted");
`;

type ThreadPort = XlsxWorkerPort & { booted: Promise<void>; exited: Promise<number>; terminated: () => boolean };

function threadWorker(): ThreadPort {
  const thread = new Worker(new URL(`data:text/javascript,${encodeURIComponent(BOOT)}`));
  let terminated = false;
  let booted!: () => void;
  const port: ThreadPort = {
    onmessage: null,
    onerror: null,
    onmessageerror: null,
    postMessage: (message, transfer) => thread.postMessage(message, transfer as never),
    terminate: () => {
      terminated = true;
      void thread.terminate();
    },
    booted: new Promise((resolve) => (booted = resolve)),
    exited: new Promise((resolve) => thread.once("exit", resolve)),
    terminated: () => terminated,
  };
  thread.on("message", (data: XlsxWorkerResponse | "booted") => {
    if (data === "booted") booted();
    else port.onmessage?.({ data } as MessageEvent<XlsxWorkerResponse>);
  });
  thread.on("error", (error) => port.onerror?.(error as unknown as ErrorEvent));
  return port;
}

test("the worker opens and pages a workbook, then close terminates it", async () => {
  const worker = threadWorker();
  const opened = await openXlsxInWorker(await buildFixtureXlsx([gridSheet("Grid", 401, 65)], { deflate: true }), {
    createWorker: () => worker,
  });
  assert.equal(opened.status, "ok");
  if (opened.status !== "ok") return;
  assert.deepEqual(opened.book.sheets, [{ name: "Grid", kind: "worksheet" }]);
  const first = (await opened.book.page(0, 0, 0))!;
  assert.equal(first.rowPages, 3);
  assert.equal(first.rows[199]![63], "R200C64");
  // Out-of-range indexes still clamp to the last page, as in xlsx-workbook.ts.
  assert.deepEqual((await opened.book.page(0, 9, 9))!.rows, [["R401C65"]]);
  assert.equal(await opened.book.page(5, 0, 0), null);
  opened.book.close();
  assert.ok(worker.terminated());
  await worker.exited;
  await assert.rejects(opened.book.page(0, 0, 0), (error) => error instanceof XlsxWorkerError && error.reason === "failed");
});

test("cap and format failures come back from the worker, which is then terminated", async () => {
  const large = threadWorker();
  const rows = await openXlsxInWorker(await buildFixtureXlsx([gridSheet("Rows", 20_001, 1)]), { createWorker: () => large });
  assert.equal(rows.status, "tooLarge");
  assert.ok(large.terminated());
  const bad = threadWorker();
  assert.equal((await openXlsxInWorker(new TextEncoder().encode("not a zip"), { createWorker: () => bad })).status, "invalid");
  assert.ok(bad.terminated());
});

// --- A hostile DEFLATE stream behind a broken directory -----------------------

/**
 * `mib` MiB of zeros as one raw DEFLATE stream, fed to zlib 1 MiB at a time.
 * Level 9 gives the same 521,826 bytes for 512 MiB under Bun's and Node's
 * zlib; level 1 does not (5.2 MB under Bun, over the 4 MiB bound below).
 */
async function zeroStream(mib: number): Promise<Uint8Array> {
  const deflate = createDeflateRaw({ level: 9 });
  const chunks: Buffer[] = [];
  deflate.on("data", (chunk: Buffer) => chunks.push(chunk));
  const zeros = Buffer.alloc(1024 * 1024);
  for (let i = 0; i < mib; i += 1) if (!deflate.write(zeros)) await once(deflate, "drain");
  deflate.end();
  await once(deflate, "end");
  return Buffer.concat(chunks);
}

let hostile: Promise<Uint8Array> | undefined;

/**
 * A part declaring 1 byte whose stream decodes to 512 MiB, behind a broken CD
 * signature: the library's fallback decodes the whole stream (dropping all
 * but the first byte) before it fails the package as having no content types.
 */
function hostileStream(): Promise<Uint8Array> {
  hostile ??= zeroStream(512).then((deflated) => {
    const zip = writeZip([{ name: "xl/workbook.xml", deflated, crc: 0, size: 1 }]);
    const view = new DataView(zip.buffer);
    zip[view.getUint32(zip.byteLength - 22 + 16, true)] = 0;
    return zip;
  });
  return hostile.then((zip) => zip.slice());
}

test("a stream that decodes far past its declared size passes the metadata check", async () => {
  const zip = await hostileStream();
  assert.ok(zip.byteLength < 4 * 1024 * 1024);
  // Declared sizes are within every cap; only decoding the stream costs.
  assert.equal(checkXlsxPackage(zip), "ok");
});

// Decoding the 512 MiB takes about 1.3 s under V8 and 10 s under Bun's
// JavaScriptCore, past bun test's 5 s default; the browser runs it under V8.
test("negative control: left to finish, the worker decodes it and reports invalid, off the main thread", { timeout: 60_000 }, async () => {
  const worker = threadWorker();
  await worker.booted;
  let ticks = 0;
  const interval = setInterval(() => (ticks += 1), 1);
  try {
    const opened = await openXlsxInWorker(await hostileStream(), { createWorker: () => worker });
    assert.equal(opened.status, "invalid");
    // This thread's timers kept firing while the worker decoded.
    assert.ok(ticks > 0);
  } finally {
    clearInterval(interval);
  }
});

test("the open timeout terminates a booted worker in the middle of that decode", async () => {
  const worker = threadWorker();
  await worker.booted;
  const opened = await openXlsxInWorker(await hostileStream(), { createWorker: () => worker, openTimeoutMs: 50 });
  // Neither the metadata check ("ok") nor the finished decode ("invalid", above)
  // answers tooLarge: only the timeout does.
  assert.equal(opened.status, "tooLarge");
  assert.ok(worker.terminated());
  await worker.exited;
});

// --- Client protocol, with a scripted worker ----------------------------------

/** Answers `open` with one worksheet and never answers anything else. */
function silentPager(): XlsxWorkerPort & { requests: XlsxWorkerRequest[]; terminated: boolean } {
  const port = {
    onmessage: null as XlsxWorkerPort["onmessage"],
    onerror: null as XlsxWorkerPort["onerror"],
    onmessageerror: null as XlsxWorkerPort["onmessageerror"],
    requests: [] as XlsxWorkerRequest[],
    terminated: false,
    postMessage(message: XlsxWorkerRequest) {
      port.requests.push(message);
      if (message.type === "open") {
        queueMicrotask(() =>
          port.onmessage?.({ data: { type: "opened", status: "ok", sheets: [{ name: "S", kind: "worksheet" }] } } as never),
        );
      }
    },
    terminate() {
      port.terminated = true;
    },
  };
  return port;
}

test("a page request past its timeout terminates the worker", async () => {
  const worker = silentPager();
  const opened = await openXlsxInWorker(new Uint8Array(8), { createWorker: () => worker, pageTimeoutMs: 20 });
  assert.equal(opened.status, "ok");
  if (opened.status !== "ok") return;
  const first = opened.book.page(0, 0, 0);
  const queued = opened.book.page(0, 1, 0);
  await assert.rejects(first, (error) => error instanceof XlsxWorkerError && error.reason === "timeout");
  await assert.rejects(queued, (error) => error instanceof XlsxWorkerError && error.reason === "failed");
  assert.ok(worker.terminated);
  // Requests are answered one at a time: the queued one was never sent.
  assert.deepEqual(
    worker.requests.map((r) => r.type),
    ["open", "page"],
  );
});

test("an abort, a worker error or a mismatched reply terminates the worker", async () => {
  const aborted = silentPager();
  const controller = new AbortController();
  controller.abort();
  assert.equal((await openXlsxInWorker(new Uint8Array(8), { signal: controller.signal, createWorker: () => aborted })).status, "invalid");
  assert.equal(aborted.requests.length, 0);

  const late = silentPager();
  const lateController = new AbortController();
  const opened = await openXlsxInWorker(new Uint8Array(8), { signal: lateController.signal, createWorker: () => late });
  assert.equal(opened.status, "ok");
  if (opened.status !== "ok") return;
  const page = opened.book.page(0, 0, 0);
  lateController.abort();
  await assert.rejects(page, (error) => error instanceof XlsxWorkerError && error.reason === "failed");
  assert.ok(late.terminated);

  const crashing = silentPager();
  crashing.postMessage = (message) => {
    crashing.requests.push(message);
    queueMicrotask(() => crashing.onerror?.({} as ErrorEvent));
  };
  assert.equal((await openXlsxInWorker(new Uint8Array(8), { createWorker: () => crashing })).status, "invalid");
  assert.ok(crashing.terminated);

  const confused = silentPager();
  const book = await openXlsxInWorker(new Uint8Array(8), { createWorker: () => confused });
  if (book.status !== "ok") return assert.fail("expected ok");
  const reply = book.book.page(0, 0, 0);
  confused.onmessage?.({ data: { type: "page", id: 99, page: null } } as never);
  await assert.rejects(reply, (error) => error instanceof XlsxWorkerError && error.reason === "failed");
  assert.ok(confused.terminated);
});

test("the default bounds", () => {
  assert.equal(XLSX_OPEN_TIMEOUT_MS, 20_000);
  assert.equal(XLSX_PAGE_TIMEOUT_MS, 10_000);
});
