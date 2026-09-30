import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient } from "@tanstack/query-core";
import { installNodeRelativeRequestShim } from "../../../test/node-api-fetch.ts";

const restoreRequest = installNodeRelativeRequestShim();
const { ProblemError } = await import("../api.ts");
const { publicInstanceQuery, refreshPublicInstance } = await import("./instance.ts");
const originalFetch = globalThis.fetch;

test.after(() => {
  globalThis.fetch = originalFetch;
  restoreRequest();
});

await test("instance refresh bypasses stale cache, updates the shared key, and preserves failures", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { gcTime: Infinity } } });
  const requests: Request[] = [];
  let responseStatus = 200;
  globalThis.fetch = (input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(input);
    requests.push(request);
    return Promise.resolve(
      new Response(
        JSON.stringify(
          responseStatus === 200
            ? { version: requests.length, values: { features: { ai: true } } }
            : { type: "about:blank", title: "Unavailable", status: 503, code: "unavailable" },
        ),
        { status: responseStatus, headers: { "Content-Type": "application/json" } },
      ),
    );
  };
  try {
    const first = await refreshPublicInstance(client);
    assert.equal(first.version, 1);
    assert.deepEqual(client.getQueryData(publicInstanceQuery.queryKey), first);
    const refreshed = await refreshPublicInstance(client);
    assert.equal(refreshed.version, 2, "even just-fetched data is revalidated");
    assert.deepEqual(client.getQueryData(publicInstanceQuery.queryKey), refreshed);
    assert.equal(requests.length, 2);
    for (const request of requests) {
      assert.equal(new URL(request.url).pathname, "/api/v1/instance");
      assert.equal(request.cache, "no-cache");
      assert.equal(request.credentials, "include");
    }
    responseStatus = 503;
    await assert.rejects(
      refreshPublicInstance(client),
      (error: unknown) =>
        error instanceof ProblemError && error.status === 503 && error.code === "unavailable",
    );
    assert.equal(requests.length, 3, "a failed refresh does not retry implicitly");
    assert.deepEqual(client.getQueryData(publicInstanceQuery.queryKey), refreshed);
  } finally {
    client.clear();
    globalThis.fetch = originalFetch;
  }
});
