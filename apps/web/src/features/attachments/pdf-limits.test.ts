import assert from "node:assert/strict";
import test from "node:test";
import {
  PDF_ZOOM_MAX,
  PDF_ZOOM_MIN,
  readCapped,
  renderScale,
  zoomIn,
  zoomOut,
} from "./pdf-limits.ts";

test("zoom steps by 25% and clamps to 50%–300%", () => {
  assert.equal(zoomIn(1), 1.25);
  assert.equal(zoomOut(1), 0.75);
  assert.equal(zoomIn(PDF_ZOOM_MAX), PDF_ZOOM_MAX);
  assert.equal(zoomOut(PDF_ZOOM_MIN), PDF_ZOOM_MIN);
});

test("renderScale applies zoom and device pixel ratio under the pixel cap", () => {
  assert.equal(renderScale(612, 792, 1, 2), 2);
  assert.equal(renderScale(612, 792, 1.5, 1), 1.5);
  assert.equal(renderScale(612, 792, 1, Number.NaN), 1);
  assert.equal(renderScale(612, 792, 1, 0), 1);
});

test("renderScale lowers the scale so the canvas stays within the cap", () => {
  const scale = renderScale(10_000, 10_000, 3, 2, 4096 * 4096);
  assert.ok(10_000 * 10_000 * scale * scale <= 4096 * 4096 + 1);
  assert.ok(scale < 1);
});

function streamed(
  chunks: number[],
  headers: Record<string, string> = {},
): {
  response: Response;
  pulled: () => number;
  cancelled: () => boolean;
} {
  let index = 0;
  let wasCancelled = false;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      const size = chunks[index];
      if (size === undefined) {
        controller.close();
        return;
      }
      index += 1;
      controller.enqueue(new Uint8Array(size).fill(index));
    },
    cancel() {
      wasCancelled = true;
    },
  });
  return {
    response: new Response(body, { headers }),
    pulled: () => index,
    cancelled: () => wasCancelled,
  };
}

test("readCapped returns the whole body under the cap", async () => {
  const { response } = streamed([3, 2]);
  const out = await readCapped(response, 5);
  assert.equal(out.status, "bytes");
  assert.deepEqual(out.status === "bytes" ? [...out.bytes] : [], [1, 1, 1, 2, 2]);
});

test("readCapped stops at a declared oversized Content-Length without reading", async () => {
  const s = streamed([4], { "content-length": "100" });
  assert.deepEqual(await readCapped(s.response, 10), { status: "tooLarge" });
  assert.equal(s.pulled(), 0);
  assert.equal(s.cancelled(), true);
});

test("readCapped cancels the stream once an undeclared body passes the cap", async () => {
  const s = streamed([4, 4, 4, 4]);
  assert.deepEqual(await readCapped(s.response, 6), { status: "tooLarge" });
  assert.ok(s.pulled() < 4);
  assert.equal(s.cancelled(), true);
});
