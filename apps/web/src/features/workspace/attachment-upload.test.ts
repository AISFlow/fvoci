import assert from "node:assert/strict";
import { test } from "node:test";
import { installNodeRelativeRequestShim } from "../../../test/node-api-fetch.ts";

const restoreNodeRequest = installNodeRelativeRequestShim();

const WS = "11111111-1111-7111-8111-111111111111";
const DOC = "22222222-2222-7222-8222-222222222222";
const ATT = "33333333-3333-7333-8333-333333333333";
const PART_SIZE = 1024;

function partUrl(n: number): string {
  return `/api/v1/workspaces/${WS}/attachments/${ATT}/parts/${String(n)}`;
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

await test("attachment-upload orchestration", { concurrency: 1 }, async (t) => {
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
        const body = (await readJsonBody(init)) as {
          parts: { partNumber: number; etag: string }[];
        };
        completed = body.parts;
        return jsonResponse(storedOutput);
      }
      const n = Number(url.split("/").at(-1));
      if (Number.isFinite(n) && url.includes("/parts/")) {
        const bodyBlob = init?.body;
        assert.ok(bodyBlob instanceof Blob);
        sizes.set(n, bodyBlob.size);
        return jsonResponse({ etag: `etag-${String(n)}` });
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
      return jsonResponse({ etag: `etag-${String(n)}` });
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
        const n = Number(requestUrl(input).split("/").at(-1));
        return jsonResponse({ etag: `etag-${String(n)}` });
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
    assert.ok(partCalls < 1000, `part calls ${String(partCalls)}`);
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
        new Promise((resolve) => {
          setTimeout(resolve, ms);
        }),
    });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const controller = new AbortController();
    const upload = bridge.upload(file, () => undefined, controller.signal);
    setTimeout(() => {
      controller.abort();
    }, 5);
    await assert.rejects(upload, (err: unknown) => isAbortError(err));
    assert.equal(partCalls, 1);
  });

  await t.test(
    "complete fetch rejection reconciles stored metadata without a second POST",
    async () => {
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
        return jsonResponse({ etag: `etag-${String(n)}` });
      });
      const bridge = await loadBridge({ delay: () => Promise.resolve() });
      const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
      const result = await bridge.upload(file, () => undefined);
      assert.equal(result.id, ATT);
      assert.equal(completeCalls, 1);
    },
  );

  await t.test(
    "ambiguous complete reuses stored metadata instead of retrying complete",
    async () => {
      let completeCalls = 0;
      installFetch((url, init) => {
        if (url.endsWith("/uploads") && init?.method === "POST") {
          return Promise.resolve(jsonResponse(createOutput(1), 201));
        }
        if (url.endsWith("/complete") && init?.method === "POST") {
          completeCalls += 1;
          return Promise.resolve(
            jsonResponse({ code: "upload_is_not_in_the_required_state" }, 409),
          );
        }
        if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
          return Promise.resolve(
            jsonResponse({
              ...storedOutput,
              completedAt: new Date(0).toISOString(),
            }),
          );
        }
        const n = Number(url.split("/").at(-1));
        return Promise.resolve(jsonResponse({ etag: `etag-${String(n)}` }));
      });
      const bridge = await loadBridge({ delay: () => Promise.resolve() });
      const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
      const result = await bridge.upload(file, () => undefined);
      assert.equal(result.id, ATT);
      assert.equal(completeCalls, 1);
    },
  );

  // Cloudflare-style gateway failures answer with an HTML page, not a problem body.
  function gatewayResponse(status: number): Response {
    return new Response(`<html><body>error code: ${String(status)}</body></html>`, {
      status,
      headers: { "Content-Type": "text/html" },
    });
  }

  function notYetStored(): Response {
    return jsonResponse({ ...storedOutput, completedAt: null });
  }

  await t.test(
    "complete 524 before the server commits re-sends the idempotent complete",
    async () => {
      let completeCalls = 0;
      let metaCalls = 0;
      installFetch((url, init) => {
        if (url.endsWith("/uploads") && init?.method === "POST") {
          return Promise.resolve(jsonResponse(createOutput(2), 201));
        }
        if (url.endsWith("/complete") && init?.method === "POST") {
          completeCalls += 1;
          return Promise.resolve(
            completeCalls === 1 ? gatewayResponse(524) : jsonResponse(storedOutput),
          );
        }
        if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
          metaCalls += 1;
          return Promise.resolve(notYetStored());
        }
        const n = Number(url.split("/").at(-1));
        return Promise.resolve(jsonResponse({ etag: `etag-${String(n)}` }));
      });
      const bridge = await loadBridge({ delay: () => Promise.resolve() });
      const file = new File([new Uint8Array(PART_SIZE * 2)], "f.bin");
      const result = await bridge.upload(file, () => undefined);
      assert.equal(result.id, ATT);
      assert.equal(completeCalls, 2);
      assert.equal(metaCalls, 1);
    },
  );

  await t.test("complete connection loss with nothing stored re-sends complete", async () => {
    let completeCalls = 0;
    const submitted: unknown[] = [];
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        completeCalls += 1;
        submitted.push(await readJsonBody(init));
        if (completeCalls === 1) return Promise.reject(new TypeError("Failed to fetch"));
        return jsonResponse(storedOutput);
      }
      if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
        return Promise.reject(new TypeError("Failed to fetch"));
      }
      const n = Number(url.split("/").at(-1));
      return jsonResponse({ etag: `etag-${String(n)}` });
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const result = await bridge.upload(file, () => undefined);
    assert.equal(result.id, ATT);
    assert.equal(completeCalls, 2);
    assert.deepEqual(submitted[0], submitted[1]);
  });

  await t.test("complete gateway failures are retried a bounded number of times", async () => {
    let completeCalls = 0;
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return Promise.resolve(jsonResponse(createOutput(1), 201));
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        completeCalls += 1;
        return Promise.resolve(gatewayResponse(524));
      }
      if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
        return Promise.resolve(notYetStored());
      }
      const n = Number(url.split("/").at(-1));
      return Promise.resolve(jsonResponse({ etag: `etag-${String(n)}` }));
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    await assert.rejects(
      bridge.upload(file, () => undefined),
      (err: unknown) => {
        assert.ok(err instanceof Error);
        assert.equal((err as { status?: number }).status, 524);
        return true;
      },
    );
    assert.equal(completeCalls, 3);
  });

  await t.test("complete denials and quota refusals are not re-sent", async () => {
    for (const status of [400, 402, 403, 409, 413, 500]) {
      let completeCalls = 0;
      installFetch((url, init) => {
        if (url.endsWith("/uploads") && init?.method === "POST") {
          return Promise.resolve(jsonResponse(createOutput(1), 201));
        }
        if (url.endsWith("/complete") && init?.method === "POST") {
          completeCalls += 1;
          return Promise.resolve(jsonResponse({ code: "invalid_input" }, status));
        }
        if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
          return Promise.resolve(notYetStored());
        }
        const n = Number(url.split("/").at(-1));
        return Promise.resolve(jsonResponse({ etag: `etag-${String(n)}` }));
      });
      const bridge = await loadBridge({ delay: () => Promise.resolve() });
      const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
      await assert.rejects(bridge.upload(file, () => undefined));
      assert.equal(completeCalls, 1, `status ${String(status)}`);
    }
  });

  // Metadata answers that must never replace the complete's own outcome.
  const unusableMeta: [string, () => Response | Promise<Response>][] = [
    ["rejects", () => Promise.reject(new TypeError("Failed to fetch"))],
    ["html 200", () => new Response("<html>interstitial</html>", { status: 200 })],
    ["malformed json", () => jsonResponse({ unexpected: true })],
    ["404 problem", () => jsonResponse({ code: "not_found" }, 404)],
    ["503 html", () => gatewayResponse(503)],
  ];

  await t.test("a failing metadata lookup keeps the complete refusal authoritative", async () => {
    for (const status of [400, 402, 409, 413, 500]) {
      for (const [label, meta] of unusableMeta) {
        let completeCalls = 0;
        let metaCalls = 0;
        installFetch(async (url, init) => {
          if (url.endsWith("/uploads") && init?.method === "POST") {
            return jsonResponse(createOutput(1), 201);
          }
          if (url.endsWith("/complete") && init?.method === "POST") {
            completeCalls += 1;
            return jsonResponse({ code: "invalid_input" }, status);
          }
          if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
            metaCalls += 1;
            return meta();
          }
          const n = Number(url.split("/").at(-1));
          return jsonResponse({ etag: `etag-${String(n)}` });
        });
        const bridge = await loadBridge({ delay: () => Promise.resolve() });
        const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
        await assert.rejects(
          bridge.upload(file, () => undefined),
          (err: unknown) => {
            assert.equal((err as Error).name, "ProblemError", `${String(status)} / ${label}`);
            assert.equal(
              (err as { status?: number }).status,
              status,
              `${String(status)} / ${label}`,
            );
            return true;
          },
        );
        assert.equal(completeCalls, 1, `${String(status)} / ${label}`);
        assert.equal(metaCalls, 1, `${String(status)} / ${label}`);
      }
    }
  });

  await t.test("a complete refusal without a problem body keeps its status", async () => {
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        return new Response(null, { status: 413 });
      }
      if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
        return Promise.reject(new TypeError("Failed to fetch"));
      }
      const n = Number(url.split("/").at(-1));
      return jsonResponse({ etag: `etag-${String(n)}` });
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    await assert.rejects(
      bridge.upload(file, () => undefined),
      (err: unknown) => {
        assert.equal((err as { status?: number }).status, 413);
        return true;
      },
    );
  });

  await t.test(
    "a failing metadata lookup keeps gateway retries bounded and the final 524",
    async () => {
      for (const [label, meta] of unusableMeta) {
        let completeCalls = 0;
        installFetch(async (url, init) => {
          if (url.endsWith("/uploads") && init?.method === "POST") {
            return jsonResponse(createOutput(1), 201);
          }
          if (url.endsWith("/complete") && init?.method === "POST") {
            completeCalls += 1;
            return gatewayResponse(524);
          }
          if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
            return meta();
          }
          const n = Number(url.split("/").at(-1));
          return jsonResponse({ etag: `etag-${String(n)}` });
        });
        const bridge = await loadBridge({ delay: () => Promise.resolve() });
        const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
        await assert.rejects(
          bridge.upload(file, () => undefined),
          (err: unknown) => {
            assert.equal((err as Error).name, "ProblemError", label);
            assert.equal((err as { status?: number }).status, 524, label);
            return true;
          },
        );
        assert.equal(completeCalls, 3, label);
      }
    },
  );

  await t.test("abort during the metadata lookup stops without another complete", async () => {
    let completeCalls = 0;
    const controller = new AbortController();
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return jsonResponse(createOutput(1), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        completeCalls += 1;
        return gatewayResponse(524);
      }
      if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
        const signal = init.signal;
        return new Promise<Response>((_, reject) => {
          signal?.addEventListener("abort", () => {
            reject(signal.reason instanceof Error ? signal.reason : new Error("Aborted"));
          });
          controller.abort();
        });
      }
      const n = Number(url.split("/").at(-1));
      return jsonResponse({ etag: `etag-${String(n)}` });
    });
    const bridge = await loadBridge({ delay: () => Promise.resolve() });
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const upload = bridge.upload(file, () => undefined, controller.signal);
    await assert.rejects(upload, (err: unknown) => isAbortError(err));
    assert.equal(completeCalls, 1);
  });

  await t.test("abort during the complete retry delay stops without another complete", async () => {
    let completeCalls = 0;
    const controller = new AbortController();
    installFetch((url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST") {
        return Promise.resolve(jsonResponse(createOutput(1), 201));
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        completeCalls += 1;
        // Aborts inside the (real, 1 s) retry delay that follows.
        setTimeout(() => {
          controller.abort();
        }, 20);
        return Promise.resolve(gatewayResponse(502));
      }
      if (url.endsWith(`/attachments/${ATT}`) && init?.method === "GET") {
        return Promise.resolve(notYetStored());
      }
      const n = Number(url.split("/").at(-1));
      return Promise.resolve(jsonResponse({ etag: `etag-${String(n)}` }));
    });
    const bridge = await loadBridge();
    const file = new File([new Uint8Array(PART_SIZE)], "f.bin");
    const upload = bridge.upload(file, () => undefined, controller.signal);
    await assert.rejects(upload, (err: unknown) => isAbortError(err));
    assert.equal(completeCalls, 1);
  });

  await t.test(
    "a part answered 524 after the server stored it is re-sent and completes",
    async () => {
      const putCounts = new Map<number, number>();
      installFetch((url, init) => {
        if (url.endsWith("/uploads") && init?.method === "POST") {
          return Promise.resolve(jsonResponse(createOutput(2), 201));
        }
        if (url.endsWith("/complete") && init?.method === "POST") {
          return Promise.resolve(jsonResponse(storedOutput));
        }
        const n = Number(url.split("/").at(-1));
        putCounts.set(n, (putCounts.get(n) ?? 0) + 1);
        if (n === 2 && putCounts.get(2) === 1) return Promise.resolve(gatewayResponse(524));
        return Promise.resolve(jsonResponse({ etag: `etag-${String(n)}` }));
      });
      const bridge = await loadBridge({ delay: () => Promise.resolve() });
      const file = new File([new Uint8Array(PART_SIZE * 2)], "f.bin");
      const result = await bridge.upload(file, () => undefined);
      assert.equal(result.id, ATT);
      assert.equal(putCounts.get(1), 1);
      assert.equal(putCounts.get(2), 2);
    },
  );

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
        const body = (await readJsonBody(init)) as {
          parts: { partNumber: number; etag: string }[];
        };
        completed = body.parts;
        return jsonResponse(storedOutput);
      }
      const n = Number(url.split("/").at(-1));
      putCounts.set(n, (putCounts.get(n) ?? 0) + 1);
      if (n === 2 && (putCounts.get(2) ?? 0) <= 3) {
        return new Response("boom", { status: 500 });
      }
      return jsonResponse({ etag: `etag-${String(n)}` });
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

  // ---- presigned sessions (#149 B): parts go to storage, never to the API.

  const STORAGE = "https://files.example.test/fvoci/key";
  const signedUrl = (n: number, gen = 1) =>
    `${STORAGE}?partNumber=${String(n)}&uploadId=u&X-Amz-Signature=sig${String(gen)}`;
  const presignedCreate = (partCount: number, expiresAt: number) => ({
    attachmentId: ATT,
    partSizeBytes: PART_SIZE,
    transfer: "presigned",
    partUrlsExpireAt: new Date(expiresAt).toISOString(),
    parts: Array.from({ length: partCount }, (_, i) => ({
      partNumber: i + 1,
      url: signedUrl(i + 1),
    })),
  });
  type StoragePut = { url: string; init?: RequestInit };
  /** API routes through `globalThis.fetch`; storage PUTs through `fetchImpl`. */
  function presignedServer(opts: {
    create: unknown;
    resume?: () => unknown;
    storage: (put: StoragePut, n: number) => Response | Promise<Response>;
  }) {
    const state = {
      resumes: 0,
      apiParts: 0,
      completed: [] as { partNumber: number; etag: string }[],
      puts: [] as StoragePut[],
    };
    installFetch(async (url, init) => {
      if (url.endsWith("/uploads") && init?.method === "POST")
        return jsonResponse(opts.create, 201);
      if (url.endsWith("/upload") && init?.method === "GET") {
        state.resumes += 1;
        return jsonResponse(opts.resume?.());
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        state.completed = ((await readJsonBody(init)) as { parts: typeof state.completed }).parts;
        return jsonResponse(storedOutput);
      }
      if (url.includes("/parts/")) state.apiParts += 1;
      throw new Error(`unexpected API fetch: ${url}`);
    });
    const fetchImpl = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
      const url = input instanceof Request ? input.url : input instanceof URL ? input.href : input;
      assert.ok(url.startsWith(STORAGE), `storage PUT only: ${url}`);
      const put = { url, init };
      state.puts.push(put);
      return opts.storage(put, Number(new URL(url).searchParams.get("partNumber")));
    };
    return { state, fetchImpl };
  }
  const stored = (n: number) =>
    new Response(null, { status: 200, headers: { ETag: `"etag-${String(n)}"` } });

  await t.test("presigned parts go to storage with the signed URL alone", async () => {
    const now = Date.now();
    const { state, fetchImpl } = presignedServer({
      create: presignedCreate(3, now + 900_000),
      storage: (_put, n) => stored(n),
    });
    const bridge = await loadBridge({ fetchImpl, delay: () => Promise.resolve() });
    // Untyped: Bun's Blob.slice keeps the File's type even when given "" as
    // the content type, where browsers (File API) use "". That typed files'
    // parts go without Content-Type is checked in Chromium by e2e-s3 (PNG,
    // PDF and text files); here, that nothing adds a type.
    const file = new File([new Uint8Array(PART_SIZE * 2 + 7)], "f.bin");
    const result = await bridge.upload(file, () => undefined);
    assert.equal(result.id, ATT);
    assert.equal(state.apiParts, 0);
    assert.equal(state.puts.length, 3);
    for (const { init } of state.puts) {
      assert.equal(init?.method, "PUT");
      assert.equal(init.credentials, "omit", "no FVOCI cookies to the storage origin");
      assert.equal(init.headers, undefined, "no Content-Type or Authorization");
      assert.ok(init.body instanceof Blob);
      assert.equal(init.body.type, "", "a typed Blob would add Content-Type");
    }
    const sizes = state.puts.map((p) => (p.init?.body as Blob).size).sort((a, b) => a - b);
    assert.deepEqual(sizes, [7, PART_SIZE, PART_SIZE]);
    assert.deepEqual(
      [...state.completed].sort((a, b) => a.partNumber - b.partNumber),
      [1, 2, 3].map((n) => ({ partNumber: n, etag: `"etag-${String(n)}"` })),
    );
    assert.equal(state.resumes, 0);
  });

  await t.test("a presigned part refused with 403 is re-issued through resume", async () => {
    const now = Date.now();
    const { state, fetchImpl } = presignedServer({
      create: presignedCreate(2, now + 900_000),
      resume: () => ({
        ...presignedCreate(0, now + 900_000),
        uploadedParts: [{ partNumber: 1, etag: "etag-1" }],
        parts: [{ partNumber: 2, url: signedUrl(2, 2) }],
      }),
      storage: (put, n) =>
        n === 2 && put.url.endsWith("sig1") ? new Response(null, { status: 403 }) : stored(n),
    });
    const bridge = await loadBridge({ fetchImpl, delay: () => Promise.resolve() });
    await bridge.upload(new File([new Uint8Array(PART_SIZE * 2)], "f.bin"), () => undefined);
    assert.equal(state.resumes, 1);
    assert.equal(state.apiParts, 0);
    assert.deepEqual(
      state.puts.map((p) => p.url).filter((u) => u.includes("partNumber=2")),
      [signedUrl(2, 1), signedUrl(2, 2)],
    );
    assert.deepEqual(
      [...state.completed].sort((a, b) => a.partNumber - b.partNumber),
      [
        { partNumber: 1, etag: "etag-1" },
        { partNumber: 2, etag: '"etag-2"' },
      ],
    );
  });

  await t.test("URLs close to expiry are re-issued before any byte is sent", async () => {
    const t0 = 1_000_000;
    // Create answered at t0; 850 s pass before the part is sent (the URL has
    // 50 s left); the re-issued URLs arrive at t0 + 900 s.
    const clock = [t0, t0 + 850_000, t0 + 900_000, t0 + 901_000];
    const { state, fetchImpl } = presignedServer({
      create: presignedCreate(1, t0 + 900_000),
      resume: () => ({
        ...presignedCreate(0, t0 + 1_800_000),
        uploadedParts: [],
        parts: [{ partNumber: 1, url: signedUrl(1, 2) }],
      }),
      storage: (_put, n) => stored(n),
    });
    const bridge = await loadBridge({
      fetchImpl,
      delay: () => Promise.resolve(),
      now: () => {
        const instant = clock.length > 1 ? clock.shift() : clock[0];
        assert.ok(instant !== undefined, "clock fixture has an instant");
        return instant;
      },
    });
    await bridge.upload(new File([new Uint8Array(10)], "f.bin"), () => undefined);
    assert.equal(state.resumes, 1);
    assert.deepEqual(
      state.puts.map((p) => p.url),
      [signedUrl(1, 2)],
      "the stale URL is never used",
    );
  });

  await t.test("a URL storage keeps refusing is re-issued once, not forever", async () => {
    const now = Date.now();
    const { state, fetchImpl } = presignedServer({
      create: presignedCreate(1, now + 900_000),
      resume: () => ({ ...presignedCreate(1, now + 900_000), uploadedParts: [] }),
      storage: () => new Response(null, { status: 403 }),
    });
    const bridge = await loadBridge({ fetchImpl, delay: () => Promise.resolve() });
    await assert.rejects(
      bridge.upload(new File([new Uint8Array(10)], "f.bin"), () => undefined),
      /part 1: storage refused the signed URL \(HTTP 403\)/,
    );
    assert.equal(state.resumes, 1);
    assert.equal(state.puts.length, 2);
    assert.equal(state.apiParts, 0);
  });

  await t.test("a stored part without an exposed ETag fails without resending", async () => {
    const now = Date.now();
    const { state, fetchImpl } = presignedServer({
      create: presignedCreate(1, now + 900_000),
      storage: () => new Response(null, { status: 200 }),
    });
    const bridge = await loadBridge({ fetchImpl, delay: () => Promise.resolve() });
    await assert.rejects(
      bridge.upload(new File([new Uint8Array(10)], "f.bin"), () => undefined),
      /ExposeHeaders/,
    );
    assert.equal(state.puts.length, 1);
    assert.equal(state.resumes, 0);
  });

  await t.test("presigned network failures retry, then resume once, still to storage", async () => {
    const now = Date.now();
    let failures = 0;
    const { state, fetchImpl } = presignedServer({
      create: presignedCreate(1, now + 900_000),
      resume: () => ({ ...presignedCreate(1, now + 900_000), uploadedParts: [] }),
      storage: (_put, n) => {
        if (failures < 3) {
          failures += 1;
          throw new TypeError("Failed to fetch");
        }
        return stored(n);
      },
    });
    const bridge = await loadBridge({ fetchImpl, delay: () => Promise.resolve() });
    await bridge.upload(new File([new Uint8Array(10)], "f.bin"), () => undefined);
    assert.equal(state.puts.length, 4);
    assert.equal(state.resumes, 1);
    assert.equal(state.apiParts, 0);
  });

  await t.test("a missing upload (404) is not resumed", async () => {
    const now = Date.now();
    const { state, fetchImpl } = presignedServer({
      create: presignedCreate(1, now + 900_000),
      storage: () => new Response("<Error><Code>NoSuchUpload</Code></Error>", { status: 404 }),
    });
    const bridge = await loadBridge({ fetchImpl, delay: () => Promise.resolve() });
    await assert.rejects(bridge.upload(new File([new Uint8Array(10)], "f.bin"), () => undefined));
    assert.equal(state.puts.length, 1);
    assert.equal(state.resumes, 0);
  });

  await t.test("downloadUrl uses the workspace attachment download route", async () => {
    const bridge = await loadBridge();
    assert.equal(bridge.downloadUrl(ATT), `/api/v1/workspaces/${WS}/attachments/${ATT}/download`);
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
