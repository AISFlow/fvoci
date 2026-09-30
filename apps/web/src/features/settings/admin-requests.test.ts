import assert from "node:assert/strict";
import test from "node:test";
import { saveBrandingAsset } from "./admin-requests";
import { installNodeRelativeRequestShim } from "../../../test/node-api-fetch";

await test("branding upload sends the original octets; clearing sends DELETE without a body", async () => {
  const originalFetch = globalThis.fetch;
  const restore = installNodeRelativeRequestShim();
  const requests: Request[] = [];
  const bodies: Uint8Array[] = [];
  globalThis.fetch = async (input: RequestInfo | URL) => {
    assert.ok(input instanceof Request);
    requests.push(input);
    bodies.push(new Uint8Array(await input.arrayBuffer()));
    return Response.json({});
  };
  try {
    const bytes = new Uint8Array([0, 1, 127, 128, 255, 13, 10]);
    await saveBrandingAsset("logo", new File([bytes], "logo.png", { type: "image/png" }));
    const upload = requests[0];
    assert.ok(upload);
    assert.equal(upload.method, "POST");
    assert.equal(upload.url, "http://fvoci.test/api/v1/admin/branding/assets/logo");
    assert.equal(upload.headers.get("content-type"), "application/octet-stream");
    assert.deepEqual(bodies[0], bytes, "binary payload is neither JSON nor text");
    await saveBrandingAsset("logo", null);
    const clear = requests[1];
    assert.ok(clear);
    assert.equal(clear.method, "DELETE");
    assert.equal(clear.url, upload.url);
    assert.equal(clear.headers.get("content-type"), null);
    assert.deepEqual(bodies[1], new Uint8Array());
  } finally {
    globalThis.fetch = originalFetch;
    restore();
  }
});
