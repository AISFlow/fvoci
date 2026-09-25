import assert from "node:assert/strict";
import { test } from "node:test";
import { installNodeRelativeRequestShim } from "../../../test/node-api-fetch.ts";

const restoreNodeRequest = installNodeRelativeRequestShim();

const WS = "11111111-1111-7111-8111-111111111111";
const DOC = "22222222-2222-7222-8222-222222222222";
const ATT = "33333333-3333-7333-8333-333333333333";
const PART_SIZE = 1024;

function partUrl(n: number): string {
  return `/api/v1/workspaces/${WS}/attachments/${ATT}/parts/${n}`;
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function createOutput(partCount: number) {
  return {
    attachmentId: ATT,
    partSizeBytes: PART_SIZE,
    parts: Array.from({ length: partCount }, (_, i) => ({
      partNumber: i + 1,
      url: partUrl(i + 1),
    })),
  };
}

const storedOutput = {
  id: ATT,
  name: "f.bin",
  mime: "application/octet-stream",
  sizeBytes: 0,
  image: false,
  scanStatus: "skipped",
  preview: null,
  createdAt: new Date(0).toISOString(),
  completedAt: new Date(0).toISOString(),
};

function requestPath(url: string): string {
  return url.replace(/^https?:\/\/[^/]+/, "");
}

function requestUrl(input: RequestInfo | URL): string {
  if (input instanceof Request) return requestPath(input.url);
  return requestPath(String(input));
}

const originalFetch = globalThis.fetch;

function requestInit(input: RequestInfo | URL, init?: RequestInit): RequestInit | undefined {
  if (input instanceof Request) {
    return {
      method: input.method,
      body: input.body,
      headers: input.headers,
      signal: input.signal,
    };
  }
  return init;
}

async function readJsonBody(init?: RequestInit): Promise<unknown> {
  const body = init?.body;
  if (typeof body === "string") return JSON.parse(body);
  const text = await new Response(body as BodyInit | null).text();
  return JSON.parse(text);
}

function isAbortError(err: unknown): boolean {
  return err instanceof Error && err.name === "AbortError";
}

function installFetch(
  handler: (url: string, init?: RequestInit) => Response | Promise<Response>,
): void {
  globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = requestUrl(input);
    return handler(url, requestInit(input, init));
  };
}

async function loadBridge(
  pipelineDeps: Parameters<typeof import("./attachment-upload.ts").createAttachmentBridge>[2] = {},
) {
  const { createAttachmentBridge } = await import("./attachment-upload.ts");
  return createAttachmentBridge(WS, DOC, pipelineDeps);
}

test("attachment-upload orchestration", { concurrency: 1 }, async (t) => {
  t.after(() => {
    restoreNodeRequest();
  });
  t.afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  await t.test("part boundaries use the trailing remainder size", async () => {
    const sizes = new Map<number, number>();
    let completed: { partNumber: number; etag: string }[] = [];
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(3), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        const body = (await readJsonBody(init)) as { parts: { partNumber: number; etag: string }[] };
        completed = body.parts;
        return jsonResponse(storedOutput);
      }
      const n = Number(url.split("/").at(-1));
      if (Number.isFinite(n) && url.includes("/parts/")) {
        const bodyBlob = init?.body;
        assert.ok(bodyBlob instanceof Blob);
        sizes.set(n, bodyBlob.size);
        return jsonResponse({ etag: `etag-${n}` });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE * 2 + 512)], "f.bin");
    const result = await bridge.upload(file, () => undefined);
    assert.deepEqual(result, { id: ATT, name: "f.bin", image: false });
    assert.equal(sizes.get(1), PART_SIZE);
    assert.equal(sizes.get(2), PART_SIZE);
    assert.equal(sizes.get(3), 512);
    assert.deepEqual(completed.map((part) => part.partNumber).sort(), [1, 2, 3]);
  });

  await t.test("parallel uploads stay within three in-flight parts", async () => {
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(8), 201);
      }
      if (url.endsWith("/complete")) return jsonResponse(storedOutput);
      const n = Number(url.split("/").at(-1));
      return jsonResponse({ etag: `etag-${n}` });
    });
    let inFlight = 0;
    let maxInFlight = 0;
    const bridge = await loadBridge({
      delay: () => Promise.resolve(),
      fetchImpl: async (input) => {
        inFlight += 1;
        maxInFlight = Math.max(maxInFlight, inFlight);
        await new Promise((resolve) => setTimeout(resolve, 5));
        inFlight -= 1;
        const n = Number(String(input).split("/").at(-1));
        return jsonResponse({ etag: `etag-${n}` });
      },
    });
    const file = new File([new Uint8Array(PART_SIZE * 8)], "f.bin");
    await bridge.upload(file, () => undefined);
    assert.ok(maxInFlight <= 3);
  });

  await t.test("413 part responses do not resume the upload session", async () => {
    let resumeCalls = 0;
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/upload") && init?.method === "GET") {
        resumeCalls += 1;
        throw new Error("unexpected resume");
      }
      if (url.includes("/parts/")) {
        return new Response("too large", { status: 413 });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    await assert.rejects(bridge.upload(file, () => undefined));
    assert.equal(resumeCalls, 0);
  });

  await t.test("401/403/404 part responses are not retried as transient transport", async () => {
    let partCalls = 0;
    let resumeCalls = 0;
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/upload") && init?.method === "GET") {
        resumeCalls += 1;
        return jsonResponse({
          attachmentId: ATT,
          partSizeBytes: PART_SIZE,
          uploadedParts: [],
          parts: [{ partNumber: 1, url: partUrl(1) }],
        });
      }
      if (url.includes("/parts/")) {
        partCalls += 1;
        return new Response("forbidden", { status: 403 });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    await assert.rejects(bridge.upload(file, () => undefined));
    assert.equal(partCalls, 1);
    assert.equal(resumeCalls, 0);
  });

  await t.test("capacity 503 waits out Retry-After without using transport retries", async () => {
    let partCalls = 0;
    const delays: number[] = [];
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        return jsonResponse(storedOutput);
      }
      if (url.includes("/parts/")) {
        partCalls += 1;
        if (partCalls <= 5) {
          return new Response(JSON.stringify({ code: "upload_capacity_exceeded" }), {
            status: 503,
            headers: { "Content-Type": "application/problem+json", "Retry-After": "2" },
          });
        }
        return jsonResponse({ etag: "etag-1" });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    const bridge = await loadBridge({
      delay: (ms) => {
        delays.push(ms);
        return Promise.resolve();
      },
    });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const result = await bridge.upload(file, () => undefined);
    assert.equal(result.id, ATT);
    assert.equal(partCalls, 6);
    assert.deepEqual(delays.slice(0, 5), [2000, 2000, 2000, 2000, 2000]);
  });

  await t.test("capacity 503 with Retry-After 0 still ends within the wait budget", async () => {
    let partCalls = 0;
    let waitedMs = 0;
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.includes("/parts/")) {
        partCalls += 1;
        return new Response(JSON.stringify({ code: "upload_capacity_exceeded" }), {
          status: 503,
          headers: { "Content-Type": "application/problem+json", "Retry-After": "0" },
        });
      }
      if (url.includes(`/attachments/${ATT}`)) {
        return new Response("gone", { status: 404 });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    const bridge = await loadBridge({
      delay: (ms) => {
        waitedMs += ms;
        return Promise.resolve();
      },
    });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    await assert.rejects(bridge.upload(file, () => undefined));
    // 120 one-second capacity waits per putPart round at most, then the
    // transport retries; the whole upload must give up.
    assert.ok(partCalls < 1000, `part calls ${partCalls}`);
    assert.ok(waitedMs > 0);
  });

  await t.test("abort during part retry delay stops the upload", async () => {
    let partCalls = 0;
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.includes("/parts/")) {
        partCalls += 1;
        return new Response("boom", { status: 500 });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    const bridge = await loadBridge({
      delay: (ms) =>
        new Promise((resolve, reject) => {
          setTimeout(resolve, ms);
        }),
    });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const controller = new AbortController();
    const upload = bridge.upload(file, () => undefined, controller.signal);
    setTimeout(() => controller.abort(), 5);
    await assert.rejects(upload, (err: unknown) => isAbortError(err));
    assert.equal(partCalls, 1);
  });

  await t.test("complete fetch rejection reconciles stored metadata without a second POST", async () => {
    let completeCalls = 0;
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        completeCalls += 1;
        return Promise.reject(new TypeError("Failed to fetch"));
      }
      if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
        return jsonResponse({
          ...storedOutput,
          completedAt: new Date(0).toISOString(),
        });
      }
      const n = Number(url.split("/").at(-1));
      return jsonResponse({ etag: `etag-${n}` });
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const result = await bridge.upload(file, () => undefined);
    assert.equal(result.id, ATT);
    assert.equal(completeCalls, 1);
  });

  await t.test("ambiguous complete reuses stored metadata instead of retrying complete", async () => {
    let completeCalls = 0;
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        completeCalls += 1;
        return jsonResponse({ code: "upload_is_not_in_the_required_state" }, 409);
      }
      if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
        return jsonResponse({
          ...storedOutput,
          completedAt: new Date(0).toISOString(),
        });
      }
      const n = Number(url.split("/").at(-1));
      return jsonResponse({ etag: `etag-${n}` });
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const result = await bridge.upload(file, () => undefined);
    assert.equal(result.id, ATT);
    assert.equal(completeCalls, 1);
  });

  await t.test("resume completes with uploaded and remaining parts", async () => {
    let completed: { partNumber: number; etag: string }[] = [];
    const putCounts = new Map<number, number>();
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(3), 201);
      }
      if (url.endsWith("/upload") && init?.method === "GET") {
        return jsonResponse({
          attachmentId: ATT,
          partSizeBytes: PART_SIZE,
          uploadedParts: [
            { partNumber: 1, etag: "etag-1" },
            { partNumber: 3, etag: "etag-3" },
          ],
          parts: [{ partNumber: 2, url: partUrl(2) }],
        });
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        const body = (await readJsonBody(init)) as { parts: { partNumber: number; etag: string }[] };
        completed = body.parts;
        return jsonResponse(storedOutput);
      }
      const n = Number(url.split("/").at(-1));
      putCounts.set(n, (putCounts.get(n) ?? 0) + 1);
      if (n === 2 && (putCounts.get(2) ?? 0) <= 3) {
        return new Response("boom", { status: 500 });
      }
      return jsonResponse({ etag: `etag-${n}` });
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE * 3)], "f.bin");
    const result = await bridge.upload(file, () => undefined);
    assert.equal(result.id, ATT);
    assert.equal(putCounts.get(1), 1);
    assert.equal(putCounts.get(3), 1);
    assert.equal(putCounts.get(2), 4);
    assert.deepEqual(completed.map((part) => part.partNumber).sort(), [1, 2, 3]);
  });

  await t.test("downloadUrl uses the workspace attachment download route", async () => {
    const bridge = await loadBridge();
    assert.equal(
      bridge.downloadUrl(ATT),
      `/api/v1/workspaces/${WS}/attachments/${ATT}/download`,
    );
  });

  await t.test("attachmentMeta maps stored attachment fields", async () => {
    installFetch((url) => {
      if (url.endsWith(`/attachments/${ATT}`)) {
        return jsonResponse({
          ...storedOutput,
          sizeBytes: 1040,
          preview: { width: 640, height: 360 },
        });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    const bridge = await loadBridge();
    const meta = await bridge.attachmentMeta?.(ATT);
    assert.deepEqual(meta, {
      sizeBytes: 1040,
      mime: "application/octet-stream",
      preview: { width: 640, height: 360 },
    });
  });
});
