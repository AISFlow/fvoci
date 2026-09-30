import assert from "node:assert/strict";
import test from "node:test";
import { tProblemTitle } from "@fvoci/i18n";
import { ProblemError, problemMessage } from "@/lib/api";
import { saveBrandingAsset } from "./admin-requests";
import { adminActionMessage } from "./admin-users";
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

await test("branding rejects files above 512 KiB before reading or sending them with the existing UI error", async () => {
  const originalFetch = globalThis.fetch;
  const restore = installNodeRelativeRequestShim();
  let reads = 0;
  let requests = 0;
  class ObservedFile extends File {
    override async arrayBuffer(): Promise<ArrayBuffer> {
      reads += 1;
      return super.arrayBuffer();
    }
  }
  globalThis.fetch = () => {
    requests += 1;
    return Promise.resolve(Response.json({}));
  };
  try {
    const file = new ObservedFile([new Uint8Array(524289)], "oversized.png", { type: "image/png" });
    assert.equal(file.size, 524289);
    await assert.rejects(saveBrandingAsset("logo", file), (error: unknown) => {
      assert.ok(error instanceof ProblemError);
      assert.equal(error.status, 413);
      assert.equal(error.code, "invalid_input");
      assert.equal(adminActionMessage(error), tProblemTitle("invalid_input"));
      assert.equal(problemMessage(error, "error.network"), tProblemTitle("invalid_input"));
      return true;
    });
    assert.equal(reads, 0, "oversized picker input must not be materialized");
    assert.equal(requests, 0, "oversized picker input must not reach the transport");
  } finally {
    globalThis.fetch = originalFetch;
    restore();
  }
});

await test("branding accepts exactly 512 KiB and sends every original octet", async () => {
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
    const bytes = Uint8Array.from({ length: 524288 }, (_, index) => index % 256);
    const file = new File([bytes], "boundary.png", { type: "image/png" });
    assert.equal(file.size, 524288);
    await saveBrandingAsset("favicon", file);
    assert.equal(requests.length, 1);
    const upload = requests[0];
    assert.ok(upload);
    assert.equal(upload.method, "POST");
    assert.equal(upload.url, "http://fvoci.test/api/v1/admin/branding/assets/favicon");
    assert.equal(upload.headers.get("content-type"), "application/octet-stream");
    assert.deepEqual(bodies[0], bytes);
  } finally {
    globalThis.fetch = originalFetch;
    restore();
  }
});
