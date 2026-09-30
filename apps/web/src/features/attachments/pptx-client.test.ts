import assert from "node:assert/strict";
import test from "node:test";
import { Worker } from "node:worker_threads";
import {
  PPTX_MAX_MARKUP_BYTES,
  PPTX_OPEN_TIMEOUT_MS,
  PPTX_RENDER_TIMEOUT_MS,
} from "./pptx-limits.ts";
import {
  openPptxInWorker,
  PptxWorkerError,
  type PptxWorkerPort,
  type PptxWorkerRequest,
  type PptxWorkerResponse,
  type RemotePptxDeck,
} from "./pptx-client.ts";
import { buildChartPptx, HOSTILE_PPTX_MARKUP } from "./pptx-hostile-fixture.ts";
import { innerSlideSvg } from "./pptx-svg.ts";
import { buildFixturePptx, DEFAULT_PPTX_TEXT, FIXTURE_PPTX_FILLER } from "./pptx-test-fixture.ts";

// --- The real pptx-worker.ts, run in a node worker thread --------------------

const WORKER_URL = new URL("./pptx-worker.ts", import.meta.url).href;
/**
 * Gives the worker module a `self` like a dedicated worker's, loads it, then
 * posts a harness-only "booted" message (never forwarded to the client).
 */
const BOOT = `
import { parentPort } from "node:worker_threads";
globalThis.self = {
  postMessage: (message) => parentPort.postMessage(message),
  set onmessage(handler) { parentPort.on("message", (data) => handler({ data })); },
};
await import(${JSON.stringify(WORKER_URL)});
parentPort.postMessage("booted");
`;

type ThreadPort = PptxWorkerPort & {
  booted: Promise<void>;
  exited: Promise<number>;
  terminated: () => boolean;
};

function threadWorker(): ThreadPort {
  const thread = new Worker(new URL(`data:text/javascript,${encodeURIComponent(BOOT)}`));
  let terminated = false;
  let booted!: () => void;
  const port: ThreadPort = {
    onmessage: null,
    onerror: null,
    onmessageerror: null,
    postMessage: (message, transfer) => {
      thread.postMessage(message, transfer as never);
    },
    terminate: () => {
      terminated = true;
      thread.terminate().catch((error: unknown) => {
        assert.fail(String(error));
      });
    },
    booted: new Promise((resolve) => (booted = resolve)),
    exited: new Promise((resolve) => thread.once("exit", resolve)),
    terminated: () => terminated,
  };
  thread.on("message", (data: PptxWorkerResponse | "booted") => {
    if (data === "booted") booted();
    else port.onmessage?.({ data } as MessageEvent<PptxWorkerResponse>);
  });
  thread.on("error", (error) => port.onerror?.(error as ErrorEvent));
  return port;
}

async function openedIn(
  worker: ThreadPort,
  bytes: Uint8Array,
  options = {},
): Promise<RemotePptxDeck> {
  await worker.booted;
  const opened = await openPptxInWorker(bytes, { createWorker: () => worker, ...options });
  assert.equal(opened.status, "ok");
  return (opened as { deck: RemotePptxDeck }).deck;
}

/**
 * Opens `bytes` and lays out slide 1 in the booted worker straight through the
 * port, with no client and no bound, so the layout code is loaded and warm
 * before a client arms a short timer. The client's own `open` then replaces
 * this deck.
 */
async function warmedUp(worker: ThreadPort, bytes: Uint8Array): Promise<void> {
  await worker.booted;
  const reply = (message: PptxWorkerRequest, transfer: Transferable[] = []) =>
    new Promise<PptxWorkerResponse>((resolve) => {
      worker.onmessage = ({ data }) => {
        resolve(data);
      };
      worker.postMessage(message, transfer);
    });
  const copy = bytes.slice();
  assert.deepEqual(await reply({ type: "open", bytes: copy }, [copy.buffer]), {
    type: "opened",
    status: "ok",
    width: 960,
    height: 540,
    slideCount: 2,
  });
  const warm = await reply({ type: "render", id: 0, index: 0 });
  assert.ok(warm.type === "rendered" && warm.slide.status === "ok");
  worker.onmessage = null;
}

/** Slide 2 with this many filler paragraphs lays out in about 0.5 s in Node (review B3: 1,394 → 0.48 s). */
const SLOW_PARAGRAPHS = 1_394;

await test("the worker opens the fixture and returns each slide as the outer image SVG; the caller's bytes stay intact", async () => {
  const worker = threadWorker();
  const bytes = buildFixturePptx();
  const copy = bytes.slice();
  const deck = await openedIn(worker, bytes);
  assert.deepEqual(bytes, copy);
  assert.deepEqual([deck.width, deck.height, deck.slideCount], [960, 540, 2]);
  const first = await deck.render(0);
  assert.equal(first.status, "ok");
  const inner = innerSlideSvg((first as { svg: string }).svg);
  assert.ok(inner?.includes(DEFAULT_PPTX_TEXT.title));
  const second = await deck.render(1);
  assert.ok(
    innerSlideSvg((second as { svg: string }).svg)?.includes(DEFAULT_PPTX_TEXT.secondSlide),
  );
  assert.deepEqual(await deck.render(5), { status: "failed" });
  assert.equal(deck.closed, false);
  deck.close();
  assert.equal(deck.closed, true);
  assert.ok(worker.terminated());
  await worker.exited;
  await assert.rejects(
    deck.render(0),
    (error) => error instanceof PptxWorkerError && error.reason === "closed",
  );
});

await test("hostile chart markup comes back from the worker only inside the image template", async () => {
  const worker = threadWorker();
  const deck = await openedIn(worker, await buildChartPptx(HOSTILE_PPTX_MARKUP.prefixedScript));
  const slide = (await deck.render(0)) as { status: "ok"; svg: string };
  assert.equal(slide.status, "ok");
  assert.ok(!slide.svg.includes("script"));
  assert.ok(innerSlideSvg(slide.svg)?.includes(HOSTILE_PPTX_MARKUP.prefixedScript));
  deck.close();
  await worker.exited;
});

await test("cap and format failures come back from the worker, which is then terminated", async () => {
  const filler = new TextEncoder().encode(FIXTURE_PPTX_FILLER).byteLength;
  const overMarkup = buildFixturePptx(DEFAULT_PPTX_TEXT, {
    slide2Paragraphs: Math.ceil(PPTX_MAX_MARKUP_BYTES / filler),
  });
  for (const [bytes, status] of [
    [overMarkup, "tooLarge"],
    [new TextEncoder().encode("not a zip"), "invalid"],
  ] as const) {
    const worker = threadWorker();
    await worker.booted;
    assert.equal((await openPptxInWorker(bytes, { createWorker: () => worker })).status, status);
    assert.ok(worker.terminated());
    await worker.exited;
  }
});

await test("negative control: left to finish, a slow slide lays out in the worker while this thread keeps running", async () => {
  const worker = threadWorker();
  const deck = await openedIn(
    worker,
    buildFixturePptx(DEFAULT_PPTX_TEXT, { slide2Paragraphs: SLOW_PARAGRAPHS }),
  );
  let ticks = 0;
  const interval = setInterval(() => (ticks += 1), 1);
  try {
    const started = performance.now();
    const slide = await deck.render(1);
    assert.equal(slide.status, "ok");
    // Slow enough that the 50 ms bound below cuts it off.
    assert.ok(performance.now() - started > 150);
    assert.ok(ticks > 0);
  } finally {
    clearInterval(interval);
    deck.close();
  }
  await worker.exited;
});

await test("the render timeout terminates the worker in the middle of that layout", async () => {
  const worker = threadWorker();
  const bytes = buildFixturePptx(DEFAULT_PPTX_TEXT, { slide2Paragraphs: SLOW_PARAGRAPHS });
  // Warm-up outside the client: the 50 ms bound is for the slow slide only (a cold first render can exceed it).
  await warmedUp(worker, bytes);
  const deck = await openedIn(worker, bytes, { renderTimeoutMs: 50 });
  const started = performance.now();
  await assert.rejects(
    deck.render(1),
    (error) => error instanceof PptxWorkerError && error.reason === "timeout",
  );
  assert.ok(performance.now() - started < 150);
  assert.equal(deck.closed, true);
  assert.ok(worker.terminated());
  await worker.exited;
});

await test("the open timeout and an abort terminate a booted worker mid-open", async () => {
  const slowOpen = buildFixturePptx(DEFAULT_PPTX_TEXT, { slide2Paragraphs: 11_000 });
  const timed = threadWorker();
  await timed.booted;
  assert.deepEqual(
    await openPptxInWorker(slowOpen, { createWorker: () => timed, openTimeoutMs: 5 }),
    {
      status: "tooLarge",
    },
  );
  assert.ok(timed.terminated());
  await timed.exited;

  const aborted = threadWorker();
  await aborted.booted;
  const controller = new AbortController();
  const opening = openPptxInWorker(slowOpen, {
    createWorker: () => aborted,
    signal: controller.signal,
  });
  setTimeout(() => {
    controller.abort();
  }, 5);
  assert.deepEqual(await opening, { status: "failed" });
  assert.ok(aborted.terminated());
  await aborted.exited;
});

// --- Client protocol, with a scripted worker ----------------------------------

/** Answers `open` with a two-slide deck and never answers anything else. */
function silentDeck(): PptxWorkerPort & { requests: PptxWorkerRequest[]; terminated: boolean } {
  const port = {
    onmessage: null as PptxWorkerPort["onmessage"],
    onerror: null as PptxWorkerPort["onerror"],
    onmessageerror: null as PptxWorkerPort["onmessageerror"],
    requests: [] as PptxWorkerRequest[],
    terminated: false,
    postMessage(message: PptxWorkerRequest) {
      port.requests.push(message);
      if (message.type === "open") {
        queueMicrotask(() =>
          port.onmessage?.({
            data: { type: "opened", status: "ok", width: 960, height: 540, slideCount: 2 },
          } as never),
        );
      }
    },
    terminate() {
      port.terminated = true;
    },
  };
  return port;
}

await test("close, an abort, a worker error or a mismatched reply terminates the worker", async () => {
  const already = silentDeck();
  const controller = new AbortController();
  controller.abort();
  let created = false;
  assert.deepEqual(
    await openPptxInWorker(new Uint8Array(8), {
      signal: controller.signal,
      createWorker: () => ((created = true), already),
    }),
    { status: "failed" },
  );
  assert.equal(created, false);

  // Leaving a slide mid-layout: close() rejects the pending and the queued render with `closed`.
  const leaving = silentDeck();
  const opened = await openPptxInWorker(new Uint8Array(8), { createWorker: () => leaving });
  if (opened.status !== "ok") return assert.fail("expected ok");
  const pending = opened.deck.render(0);
  const queued = opened.deck.render(1);
  await new Promise((resolve) => setTimeout(resolve, 0));
  opened.deck.close();
  await assert.rejects(
    pending,
    (error) => error instanceof PptxWorkerError && error.reason === "closed",
  );
  await assert.rejects(
    queued,
    (error) => error instanceof PptxWorkerError && error.reason === "closed",
  );
  assert.ok(leaving.terminated);
  // One layout at a time: the queued one was never sent.
  assert.deepEqual(
    leaving.requests.map((r) => r.type),
    ["open", "render"],
  );

  const late = silentDeck();
  const lateController = new AbortController();
  const lateOpen = await openPptxInWorker(new Uint8Array(8), {
    signal: lateController.signal,
    createWorker: () => late,
  });
  if (lateOpen.status !== "ok") return assert.fail("expected ok");
  const render = lateOpen.deck.render(0);
  lateController.abort();
  await assert.rejects(
    render,
    (error) => error instanceof PptxWorkerError && error.reason === "closed",
  );
  assert.ok(late.terminated);

  const crashing = silentDeck();
  crashing.postMessage = (message) => {
    crashing.requests.push(message);
    queueMicrotask(() => crashing.onerror?.({} as ErrorEvent));
  };
  assert.deepEqual(await openPptxInWorker(new Uint8Array(8), { createWorker: () => crashing }), {
    status: "failed",
  });
  assert.ok(crashing.terminated);

  const confused = silentDeck();
  const book = await openPptxInWorker(new Uint8Array(8), { createWorker: () => confused });
  if (book.status !== "ok") return assert.fail("expected ok");
  const reply = book.deck.render(0);
  await new Promise((resolve) => setTimeout(resolve, 0));
  confused.onmessage?.({
    data: { type: "rendered", id: 99, slide: { status: "failed" } },
  } as never);
  await assert.rejects(
    reply,
    (error) => error instanceof PptxWorkerError && error.reason === "failed",
  );
  assert.ok(confused.terminated && book.deck.closed);
});

await test("a worker that dies while idle is closed, and every later render fails with `closed` without a request", async () => {
  for (const event of ["onerror", "onmessageerror"] as const) {
    const idle = silentDeck();
    const opened = await openPptxInWorker(new Uint8Array(8), { createWorker: () => idle });
    if (opened.status !== "ok") return assert.fail("expected ok");
    assert.equal(opened.deck.closed, false);
    idle[event]?.({} as ErrorEvent & MessageEvent);
    // Nothing was pending to reject: `closed` is the only trace, which the viewer checks (pptx-viewer.tsx).
    assert.ok(idle.terminated && opened.deck.closed, event);
    await assert.rejects(
      opened.deck.render(1),
      (error) => error instanceof PptxWorkerError && error.reason === "closed",
    );
    assert.deepEqual(
      idle.requests.map((r) => r.type),
      ["open"],
      event,
    );
  }
});

await test("the default bounds", () => {
  assert.equal(PPTX_OPEN_TIMEOUT_MS, 20_000);
  assert.equal(PPTX_RENDER_TIMEOUT_MS, 10_000);
});
