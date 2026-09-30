import assert from "node:assert/strict";
import test from "node:test";
// Load the real singleton before the fixture: its cached constructor is native.
import { api } from "../../lib/api";
import { installNodeRelativeRequestShim } from "../../../test/node-api-fetch";

await test("relative API fixture overrides an already-cached constructor and restores nested owners", async () => {
  const originalRequest = globalThis.Request;
  const originalGet = api.GET;
  const originalFetch = globalThis.fetch;
  const first = installNodeRelativeRequestShim();
  const second = installNodeRelativeRequestShim();
  const installedRequest = globalThis.Request;
  const requests: Request[] = [];
  globalThis.fetch = (input: RequestInfo | URL) => {
    assert.ok(input instanceof Request, "actual openapi transport constructs a Request");
    requests.push(input);
    return Promise.resolve(Response.json({}));
  };
  try {
    first();
    first();
    assert.equal(globalThis.Request, installedRequest, "another owner keeps the fixture alive");
    await api.GET("/api/v1/instance");
    assert.equal(requests[0]?.url, "http://fvoci.test/api/v1/instance");
    const absolute = new Request("https://storage.example/part", { method: "PUT" });
    assert.equal(absolute.url, "https://storage.example/part", "absolute storage URLs stay intact");
  } finally {
    first();
    second();
    globalThis.fetch = originalFetch;
  }
  assert.equal(globalThis.Request, originalRequest);
  assert.equal(api.GET, originalGet, "the client method is restored after the last owner");
});
