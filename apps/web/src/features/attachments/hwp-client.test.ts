import { assertPresent } from "./test-invariants.ts";
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
const wasm = fs.readFileSync(
  path.join(path.dirname(require.resolve("@rhwp/core")), "rhwp_bg.wasm"),
);
const module = new WebAssembly.Module(wasm);
const repoRoot = path.resolve(import.meta.dirname, "../../../../..");
const fixture = (name: string) =>
  new Uint8Array(fs.readFileSync(path.join(repoRoot, "compat/fixtures", name)));

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
    Promise.resolve(this.answer(message))
      .then((response) => {
        if (response && this.terminated === 0)
          this.onmessage?.({ data: response } as MessageEvent<HwpResponse>);
      })
      .catch((error: unknown) => {
        assert.fail(String(error));
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
      return assertPresent(worker);
    },
  };
}

const opened = (request: HwpRequest): HwpResponse =>
  request.op === "open"
    ? { id: request.id, ok: true, op: "open", pageCount: 3 }
    : request.op === "startPage"
      ? { id: request.id, ok: true, op: "startPage", page: 2 }
      : {
          id: request.id,
          ok: true,
          op: "render",
          svg: new Blob(["<svg/>"], { type: "image/svg+xml" }),
        };

async function rejectsWith(promise: Promise<unknown>, reason: string): Promise<void> {
  await assert.rejects(
    promise,
    (error) => error instanceof HwpClientError && error.reason === reason,
  );
}

await test("open transfers the bytes, and requests are answered through the worker", async () => {
  const f = fake(opened);
  const bytes = new Uint8Array([1, 2, 3]);
  const { client, pageCount } = await HwpDocumentClient.open(bytes, module, {
    createWorker: f.createWorker,
  });
  assert.equal(pageCount, 3);
  assert.deepEqual(assertPresent(f.worker.received[0]).transfer, [bytes.buffer]);
  assert.equal(await client.startPage(1), 2);
  assert.equal(await (await client.renderPage(0)).text(), "<svg/>");
  client.close();
  client.close();
  assert.equal(f.worker.terminated, 1);
  await rejectsWith(client.renderPage(0), "closed");
});

await test("a failed open terminates its worker", async () => {
  for (const error of ["tooLarge", "invalid", "failed"] as const) {
    const f = fake((request) => ({ id: request.id, ok: false, error }));
    await rejectsWith(
      HwpDocumentClient.open(new Uint8Array(1), module, { createWorker: f.createWorker }),
      error,
    );
    assert.equal(f.worker.terminated, 1);
  }
});

await test("a parse past its deadline is terminated", async () => {
  const f = fake(() => null);
  await rejectsWith(
    HwpDocumentClient.open(new Uint8Array(1), module, {
      createWorker: f.createWorker,
      openTimeoutMs: 5,
    }),
    "timeout",
  );
  assert.equal(f.worker.terminated, 1);
});

await test("aborting a pending open terminates the worker at once, not at the deadline", async () => {
  const f = fake(() => null);
  const controller = new AbortController();
  const open = HwpDocumentClient.open(new Uint8Array(1), module, {
    createWorker: f.createWorker,
    openTimeoutMs: 60_000,
    signal: controller.signal,
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(f.worker.terminated, 0);
  controller.abort();
  assert.equal(f.worker.terminated, 1);
  assert.equal(f.worker.onmessage, null);
  assert.equal(f.worker.onerror, null);
  assert.equal(f.worker.onmessageerror, null);
  await rejectsWith(open, "closed");
  assert.equal(f.worker.terminated, 1);
});

await test("an already aborted signal starts no worker; one aborted after the open leaves the client open", async () => {
  FakeWorker.all = [];
  const aborted = AbortSignal.abort();
  const f = fake(opened);
  await rejectsWith(
    HwpDocumentClient.open(new Uint8Array(1), module, {
      createWorker: f.createWorker,
      signal: aborted,
    }),
    "closed",
  );
  assert.deepEqual(FakeWorker.all, []);

  const controller = new AbortController();
  const g = fake(opened);
  const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, {
    createWorker: g.createWorker,
    signal: controller.signal,
  });
  controller.abort();
  assert.equal(client.closed, false);
  assert.equal(await client.startPage(0), 2);
  client.close();
  assert.equal(g.worker.terminated, 1);
});

await test("a render past its deadline terminates the worker and fails later requests", async () => {
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

await test("closing cancels pending requests; a worker error fails them", async () => {
  let release: (() => void) | null = null;
  const f = fake((request) =>
    request.op === "open"
      ? opened(request)
      : new Promise<HwpResponse>(
          (resolve) =>
            (release = () => {
              resolve(opened(request));
            }),
        ),
  );
  const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, {
    createWorker: f.createWorker,
  });
  const pending = client.renderPage(1);
  client.close();
  await rejectsWith(pending, "closed");
  assertPresent(release)();
  await rejectsWith(client.renderPage(1), "closed");

  const g = fake((request) => (request.op === "open" ? opened(request) : null));
  const second = await HwpDocumentClient.open(new Uint8Array(1), module, {
    createWorker: g.createWorker,
  });
  const waiting = second.client.renderPage(0);
  g.worker.onerror?.({} as ErrorEvent);
  await rejectsWith(waiting, "failed");
  assert.equal(g.worker.terminated, 1);
});

await test("replacing documents leaves no worker running", async () => {
  FakeWorker.all = [];
  const clients: HwpDocumentClient[] = [];
  for (let n = 0; n < 3; n += 1) {
    const f = fake(opened);
    const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, {
      createWorker: f.createWorker,
    });
    clients.at(-1)?.close();
    clients.push(client);
    assert.equal(FakeWorker.running().length, 1);
  }
  assertPresent(clients.at(-1)).close();
  assert.deepEqual(FakeWorker.running(), []);
  assert.ok(FakeWorker.all.every((worker) => worker.terminated === 1));
});

await test("client and real-wasm session: the Scripts bomb is refused and its worker terminated", async () => {
  // The worker body in-process: the same session code the module worker runs.
  const api = {
    opens: 0,
    init: (m: WebAssembly.Module) => {
      initSync({ module: m });
      return Promise.resolve();
    },
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
    ...[0, 1, 2, 3].map((n) => ({
      name: `Scripts/s${String(n)}.js`,
      data: new Uint8Array(32 * 1024 * 1024),
    })),
  ]);
  const f = real();
  await rejectsWith(
    HwpDocumentClient.open(bomb, module, { createWorker: f.createWorker }),
    "tooLarge",
  );
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

await test("edits travel through the worker; a revert gets the open deadline, a replace the request deadline", async () => {
  let pending: ((response: HwpResponse) => void) | null = null;
  const f = fake((request) => {
    if (request.op === "open") return { id: request.id, ok: true, op: "open", pageCount: 3 };
    if (request.op === "replace") {
      return {
        id: request.id,
        ok: true,
        op: "replace",
        outcome: request.all ? "changed" : "rejected",
        pageCount: 4,
      };
    }
    if (request.op === "export")
      return { id: request.id, ok: true, op: "export", bytes: new Uint8Array([7]) };
    if (request.op === "revert") {
      // Answered after the request deadline but within the open deadline.
      return new Promise((resolve) =>
        setTimeout(() => {
          resolve({ id: request.id, ok: true, op: "revert", pageCount: 3 });
        }, 30),
      );
    }
    return new Promise((resolve) => (pending = resolve));
  });
  const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, {
    createWorker: f.createWorker,
    openTimeoutMs: 1_000,
    requestTimeoutMs: 10,
  });
  assert.deepEqual(await client.replace("a", "b", true), { outcome: "changed", pageCount: 4 });
  assert.deepEqual(await client.replace("a", "b", false), { outcome: "rejected", pageCount: 4 });
  assert.deepEqual(assertPresent(f.worker.received[1]).message, {
    id: 2,
    op: "replace",
    find: "a",
    replacement: "b",
    all: true,
  });
  assert.deepEqual([...(await client.exportDocument("hwpx"))], [7]);
  assert.deepEqual(assertPresent(f.worker.received[3]).message, {
    id: 4,
    op: "export",
    format: "hwpx",
  });
  assert.equal(await client.revert(), 3);
  assert.equal(client.closed, false);
  // A render left unanswered past its deadline takes the document with it.
  await rejectsWith(client.renderPage(0), "timeout");
  assertPresent(pending);
  await rejectsWith(client.exportDocument("hwp"), "closed");
  assert.equal(f.worker.terminated, 1);
});

await test("a failed export rejects only that request; a failed replace terminates the worker", async () => {
  const f = fake((request) =>
    request.op === "open"
      ? { id: request.id, ok: true, op: "open", pageCount: 1 }
      : request.op === "export"
        ? { id: request.id, ok: false, error: "tooLarge" }
        : { id: request.id, ok: false, error: "failed" },
  );
  const { client } = await HwpDocumentClient.open(new Uint8Array(1), module, {
    createWorker: f.createWorker,
  });
  await rejectsWith(client.exportDocument("hwp"), "tooLarge");
  assert.equal(client.closed, false);
  assert.equal(f.worker.terminated, 0);
  // The document may be half edited: it is gone, and the export after it never reaches the worker.
  await rejectsWith(client.replace("a", "b", true), "failed");
  assert.equal(client.closed, true);
  assert.equal(f.worker.terminated, 1);
  const sent = f.worker.received.length;
  await rejectsWith(client.exportDocument("hwp"), "closed");
  assert.equal(f.worker.received.length, sent);
});
