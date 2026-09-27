import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import test from "node:test";
import { HwpDocument, initSync } from "@rhwp/core";
import { HwpClientError, HwpDocumentClient, type HwpWorkerPort } from "./hwp-client.ts";
import { createHwpSession, type HwpRequest, type HwpResponse } from "./hwp-worker-core.ts";
import { readZip, writeZip } from "./hwp-test-fixture.ts";

const require = createRequire(import.meta.url);
const wasm = fs.readFileSync(path.join(path.dirname(require.resolve("@rhwp/core")), "rhwp_bg.wasm"));
const module = new WebAssembly.Module(wasm);
const repoRoot = path.resolve(import.meta.dirname, "../../../../..");
const fixture = (name: string) => new Uint8Array(fs.readFileSync(path.join(repoRoot, "compat/fixtures", name)));

/** Every fake worker started, so a test can tell which are still running. */
class FakeWorker implements HwpWorkerPort {
  static all: FakeWorker[] = [];
  onmessage: ((event: MessageEvent<HwpResponse>) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  onmessageerror: ((event: MessageEvent) => void) | null = null;
  readonly received: { message: HwpRequest; transfer: Transferable[] }[] = [];
  terminated = 0;
  readonly answer: (request: HwpRequest) => HwpResponse | Promise<HwpResponse> | null;

  constructor(answer: (request: HwpRequest) => HwpResponse | Promise<HwpResponse> | null) {
    this.answer = answer;
    FakeWorker.all.push(this);
  }

  postMessage(message: HwpRequest, transfer: Transferable[] = []): void {
    this.received.push({ message, transfer });
    void Promise.resolve(this.answer(message)).then((response) => {
      if (response && this.terminated === 0) this.onmessage?.({ data: response } as MessageEvent<HwpResponse>);
    });
  }

  terminate(): void {
    this.terminated += 1;
  }

  static running(): FakeWorker[] {
    return FakeWorker.all.filter((worker) => worker.terminated === 0);
  }
}

function fake(answer: (request: HwpRequest) => HwpResponse | Promise<HwpResponse> | null) {
  let worker: FakeWorker | null = null;
  return {
    createWorker: () => (worker = new FakeWorker(answer)),
    get worker() {
      return worker!;
    },
  };
}

const opened = (request: HwpRequest): HwpResponse =>
  request.op === "open"
    ? { id: request.id, ok: true, op: "open", pageCount: 3 }
    : request.op === "startPage"
      ? { id: request.id, ok: true, op: "startPage", page: 2 }
      : { id: request.id, ok: true, op: "render", svg: new Blob(["<svg/>"], { type: "image/svg+xml" }) };

async function rejectsWith(promise: Promise<unknown>, reason: string): Promise<void> {
  await assert.rejects(promise, (error) => error instanceof HwpClientError && error.reason === reason);
}

test("open transfers the bytes, and requests are answered through the worker", async () => {
  const f = fake(opened);
  const bytes = new Uint8Array([1, 2, 3]);
  const { client, pageCount } = await HwpDocumentClient.open(bytes, module, { createWorker: f.createWorker });
  assert.equal(pageCount, 3);
  assert.deepEqual(f.worker.received[0]!.transfer, [bytes.buffer]);
  assert.equal(await client.startPage(1), 2);
  assert.equal(await (await client.renderPage(0)).text(), "<svg/>");
  client.close();
  client.close();
  assert.equal(f.worker.terminated, 1);
  await rejectsWith(client.renderPage(0), "closed");
});

test("a failed open terminates its worker", async () => {
  for (const error of ["tooLarge", "invalid", "failed"] as const) {
    const f = fake((request) => ({ id: request.id, ok: false, error }));
    await rejectsWith(HwpDocumentClient.open(new Uint8Array(1), module, { createWorker: f.createWorker }), error);
    assert.equal(f.worker.terminated, 1);
  }
});

test("a parse past its deadline is terminated", async () => {
  const f = fake(() => null);
  await rejectsWith(
    HwpDocumentClient.open(new Uint8Array(1), module, { createWorker: f.createWorker, openTimeoutMs: 5 }),
    "timeout",
  );
  assert.equal(f.worker.terminated, 1);
});

test("a render past its deadline terminates the worker and fails later requests", async () => {
  const f = fake((request) => (request.op === "render" ? null : opened(request)));
  const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, {
    createWorker: f.createWorker,
    requestTimeoutMs: 5,
  });
  const text = client.startPage(0);
  await rejectsWith(client.renderPage(0), "timeout");
  assert.equal(await text, 2);
  assert.equal(f.worker.terminated, 1);
  assert.equal(client.closed, true);
  await rejectsWith(client.startPage(0), "closed");
});

test("closing cancels pending requests; a worker error fails them", async () => {
  let release: (() => void) | null = null;
  const f = fake((request) =>
    request.op === "open" ? opened(request) : new Promise<HwpResponse>((resolve) => (release = () => resolve(opened(request)))),
  );
  const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, { createWorker: f.createWorker });
  const pending = client.renderPage(1);
  client.close();
  await rejectsWith(pending, "closed");
  release!();
  await rejectsWith(client.renderPage(1), "closed");

  const g = fake((request) => (request.op === "open" ? opened(request) : null));
  const second = await HwpDocumentClient.open(new Uint8Array(1), module, { createWorker: g.createWorker });
  const waiting = second.client.renderPage(0);
  g.worker.onerror?.({} as ErrorEvent);
  await rejectsWith(waiting, "failed");
  assert.equal(g.worker.terminated, 1);
});

test("replacing documents leaves no worker running", async () => {
  FakeWorker.all = [];
  const clients: HwpDocumentClient[] = [];
  for (let n = 0; n < 3; n += 1) {
    const f = fake(opened);
    const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, { createWorker: f.createWorker });
    clients.at(-1)?.close();
    clients.push(client);
    assert.equal(FakeWorker.running().length, 1);
  }
  clients.at(-1)!.close();
  assert.deepEqual(FakeWorker.running(), []);
  assert.ok(FakeWorker.all.every((worker) => worker.terminated === 1));
});

test("client and real-wasm session: the Scripts bomb is refused and its worker terminated", async () => {
  // The worker body in-process: the same session code the module worker runs.
  const api = {
    opens: 0,
    init: async (m: WebAssembly.Module) => void initSync({ module: m }),
    open: (bytes: Uint8Array) => {
      api.opens += 1;
      return new HwpDocument(bytes);
    },
  };
  const real = () => {
    const session = createHwpSession(api);
    return fake((request) => session(request));
  };
  const bomb = writeZip([
    ...readZip(fixture("sample.hwpx")),
    ...[0, 1, 2, 3].map((n) => ({ name: `Scripts/s${n}.js`, data: new Uint8Array(32 * 1024 * 1024) })),
  ]);
  const f = real();
  await rejectsWith(HwpDocumentClient.open(bomb, module, { createWorker: f.createWorker }), "tooLarge");
  assert.equal(api.opens, 0);
  assert.equal(f.worker.terminated, 1);

  const g = real();
  const { client, pageCount } = await HwpDocumentClient.open(fixture("sample.hwpx"), module, {
    createWorker: g.createWorker,
  });
  assert.equal(pageCount, 1);
  assert.match(await (await client.renderPage(0)).text(), /^<svg[\s>]/);
  client.close();
  assert.equal(g.worker.terminated, 1);
});
