import { PPTX_OPEN_TIMEOUT_MS, PPTX_RENDER_TIMEOUT_MS } from "./pptx-limits.ts";
import type { SlideImageSvg } from "./pptx-svg.ts";

export type PptxWorkerRequest =
  { type: "open"; bytes: Uint8Array } | { type: "render"; id: number; index: number };

export type PptxWorkerResponse =
  | { type: "opened"; status: "ok"; width: number; height: number; slideCount: number }
  | { type: "opened"; status: "tooLarge" | "invalid" }
  | { type: "rendered"; id: number; slide: SlideImageSvg }
  | { type: "failed" };

/** The part of a dedicated `Worker` the client uses. */
export type PptxWorkerPort = {
  onmessage: ((event: MessageEvent<PptxWorkerResponse>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  onmessageerror: ((event: MessageEvent) => void) | null;
  postMessage(message: PptxWorkerRequest, transfer: Transferable[]): void;
  terminate(): void;
};

export type RemotePptxDeck = {
  /** Slide size in CSS px at 100% zoom. */
  width: number;
  height: number;
  slideCount: number;
  /**
   * One slide as the outer image SVG of `pptx-svg.ts`. Rejects with
   * {@link PptxWorkerError} on timeout, worker failure or after `close`.
   */
  render(index: number): Promise<SlideImageSvg>;
  /** Terminates the worker and the deck it holds, even mid-layout. */
  close(): void;
  /** Whether the worker is gone (closed, aborted, timed out or failed). */
  readonly closed: boolean;
};

export type RemotePptxOpenResult =
  | { status: "ok"; deck: RemotePptxDeck }
  /** Over a load cap, or slower than the open timeout: the file stays download-only. */
  | { status: "tooLarge" }
  /** Not a deck the renderer can read. */
  | { status: "invalid" }
  /** The worker failed, or `signal` aborted. */
  | { status: "failed" };

export class PptxWorkerError extends Error {
  /** `closed`: `close()` or the signal ended the worker, not the request itself. */
  readonly reason: "timeout" | "failed" | "closed";

  constructor(reason: "timeout" | "failed" | "closed") {
    super(`pptx worker ${reason}`);
    this.name = "PptxWorkerError";
    this.reason = reason;
  }
}

export type PptxClientOptions = {
  /** Aborting terminates the worker at once, even mid-parse. */
  signal?: AbortSignal;
  openTimeoutMs?: number;
  renderTimeoutMs?: number;
  createWorker?: () => PptxWorkerPort;
};

export function createPptxWorker(): PptxWorkerPort {
  return new Worker(new URL("./pptx-worker.ts", import.meta.url), { type: "module", name: "pptx" });
}

/**
 * Opens a copy of `bytes` in a dedicated worker (`pptx-worker.ts`), which
 * checks and parses the package and keeps the deck; slides come back as the
 * finished outer image SVG. `bytes` itself is left intact, so the caller can
 * open it again after a worker has been terminated.
 *
 * Opening and each slide have a wall-clock bound; past it, the worker is
 * terminated, which stops any parse or layout still running in it. The worker
 * is also terminated when `signal` aborts (an already aborted signal starts
 * no worker), on `close`, and on any worker error. One slide is laid out at a
 * time, in order; once the worker is gone, every later request fails with
 * `closed`.
 */
export function openPptxInWorker(
  bytes: Uint8Array,
  options: PptxClientOptions = {},
): Promise<RemotePptxOpenResult> {
  const {
    signal,
    openTimeoutMs = PPTX_OPEN_TIMEOUT_MS,
    renderTimeoutMs = PPTX_RENDER_TIMEOUT_MS,
    createWorker = createPptxWorker,
  } = options;
  if (signal?.aborted) return Promise.resolve({ status: "failed" });
  const worker = createWorker();
  let dead = false;
  let pending: {
    resolve: (response: PptxWorkerResponse) => void;
    reject: (error: PptxWorkerError) => void;
  } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let queue: Promise<unknown> = Promise.resolve();
  let nextId = 0;

  const kill = (reason: "timeout" | "failed" | "closed") => {
    if (dead) return;
    dead = true;
    clearTimeout(timer);
    signal?.removeEventListener("abort", onAbort);
    worker.onmessage = worker.onerror = worker.onmessageerror = null;
    worker.terminate();
    const waiting = pending;
    pending = null;
    waiting?.reject(new PptxWorkerError(reason));
  };
  const onAbort = () => {
    kill("closed");
  };
  signal?.addEventListener("abort", onAbort);
  worker.onmessage = ({ data }) => {
    const waiting = pending;
    pending = null;
    clearTimeout(timer);
    if (!waiting) {
      kill("failed");
      return;
    }
    if (data.type === "failed") {
      waiting.reject(new PptxWorkerError("failed"));
      kill("failed");
    } else {
      waiting.resolve(data);
    }
  };
  worker.onerror = worker.onmessageerror = () => {
    kill("failed");
  };

  const request = (message: PptxWorkerRequest, timeoutMs: number, transfer: Transferable[] = []) =>
    new Promise<PptxWorkerResponse>((resolve, reject) => {
      if (dead) {
        reject(new PptxWorkerError("closed"));
        return;
      }
      pending = { resolve, reject };
      timer = setTimeout(() => {
        kill("timeout");
      }, timeoutMs);
      try {
        worker.postMessage(message, transfer);
      } catch {
        kill("failed");
      }
    });

  const owned = bytes.slice();
  return request({ type: "open", bytes: owned }, openTimeoutMs, [owned.buffer]).then(
    (response): RemotePptxOpenResult => {
      if (response.type !== "opened" || response.status !== "ok") {
        kill("failed");
        if (response.type !== "opened") return { status: "failed" };
        return { status: response.status };
      }
      const deck: RemotePptxDeck = {
        width: response.width,
        height: response.height,
        slideCount: response.slideCount,
        render(index) {
          const id = ++nextId;
          const result = queue.then(() => request({ type: "render", id, index }, renderTimeoutMs));
          queue = result.catch(() => undefined);
          return result.then((reply) => {
            if (reply.type !== "rendered" || reply.id !== id) {
              kill("failed");
              throw new PptxWorkerError("failed");
            }
            return reply.slide;
          });
        },
        close: () => {
          kill("closed");
        },
        get closed() {
          return dead;
        },
      };
      return { status: "ok", deck };
    },
    (error: unknown) => {
      if (!(error instanceof PptxWorkerError))
        throw new Error("unexpected worker rejection", { cause: error });
      return error.reason === "timeout" ? { status: "tooLarge" } : { status: "failed" };
    },
  );
}
