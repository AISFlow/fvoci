import assert from "node:assert/strict";
import test from "node:test";
import { downloadCapped, startViewerPrefetch, VIEWER_MAX_BYTES, type ViewerBytes } from "./viewer-download.ts";
import { DOCX_MAX_BYTES } from "./docx-limits.ts";
import { HWP_MAX_BYTES } from "./hwp-page.ts";
import { PDF_MAX_BYTES } from "./pdf-limits.ts";
import { PPTX_MAX_BYTES } from "./pptx-limits.ts";
import { XLSX_MAX_BYTES } from "./xlsx-limits.ts";

function withFetch(stub: typeof fetch, run: () => Promise<void>): Promise<void> {
  const original = globalThis.fetch;
  globalThis.fetch = stub;
  return run().finally(() => {
    globalThis.fetch = original;
  });
}

test("each layout kind downloads under the cap its viewer enforces", () => {
  assert.deepEqual(VIEWER_MAX_BYTES, {
    pdf: PDF_MAX_BYTES,
    docx: DOCX_MAX_BYTES,
    hwp: HWP_MAX_BYTES,
    pptx: PPTX_MAX_BYTES,
    xlsx: XLSX_MAX_BYTES,
  });
});

test("downloadCapped sends the session cookie and caps the body", async () => {
  const seen: RequestInit[] = [];
  await withFetch(
    async (_url, init) => {
      seen.push(init ?? {});
      return new Response(new Uint8Array(10));
    },
    async () => {
      const signal = new AbortController().signal;
      assert.deepEqual(await downloadCapped("/f", 10, signal), { status: "bytes", bytes: new Uint8Array(10) });
      assert.deepEqual(await downloadCapped("/f", 9, signal), { status: "tooLarge" });
      assert.equal(seen[0]?.credentials, "same-origin");
      assert.equal(seen[0]?.signal, signal);
    },
  );
});

test("downloadCapped reports a non-2xx response as failed and cancels its body", async () => {
  let cancelled = false;
  const body = new ReadableStream<Uint8Array>({
    cancel() {
      cancelled = true;
    },
  });
  await withFetch(
    async () => new Response(body, { status: 403 }),
    async () => {
      assert.deepEqual(await downloadCapped("/f", 10, new AbortController().signal), { status: "failed" });
      assert.equal(cancelled, true);
    },
  );
});

test("the prefetch starts at once and is handed to exactly one taker", async () => {
  const calls: { url: string; max: number; signal: AbortSignal }[] = [];
  const result: ViewerBytes = { status: "bytes", bytes: new Uint8Array([1]) };
  const prefetch = startViewerPrefetch("/f", 7, new AbortController().signal, async (url, max, signal) => {
    calls.push({ url, max, signal });
    return result;
  });
  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.url, "/f");
  assert.equal(calls[0]?.max, 7);
  const taken = prefetch.take(new AbortController().signal);
  assert.equal(await taken, result);
  // A retry or a remount downloads again instead of reusing bytes a viewer may have handed to a worker.
  assert.equal(prefetch.take(new AbortController().signal), null);
});

test("the parent's signal and the taker's signal both abort the download", async () => {
  const parent = new AbortController();
  let downloadSignal: AbortSignal | null = null;
  startViewerPrefetch("/f", 1, parent.signal, async (_u, _m, signal) => {
    downloadSignal = signal;
    return { status: "tooLarge" };
  });
  parent.abort();
  assert.equal((downloadSignal as AbortSignal | null)?.aborted, true);

  let second: AbortSignal | null = null;
  const prefetch = startViewerPrefetch("/f", 1, new AbortController().signal, async (_u, _m, signal) => {
    second = signal;
    return { status: "tooLarge" };
  });
  const taker = new AbortController();
  void prefetch.take(taker.signal);
  assert.equal((second as AbortSignal | null)?.aborted, false);
  taker.abort();
  assert.equal((second as AbortSignal | null)?.aborted, true);
});

test("an untaken failed prefetch is not an unhandled rejection", async () => {
  const parent = new AbortController();
  startViewerPrefetch("/f", 1, parent.signal, async () => {
    throw new DOMException("aborted", "AbortError");
  });
  parent.abort();
  await new Promise((resolve) => setTimeout(resolve, 0));
});
