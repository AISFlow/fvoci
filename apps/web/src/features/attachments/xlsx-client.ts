import { XLSX_OPEN_TIMEOUT_MS, XLSX_PAGE_TIMEOUT_MS } from "./xlsx-limits.ts";
import type { XlsxPage, XlsxSheet } from "./xlsx-workbook.ts";

export type XlsxWorkerRequest =
  | { type: "open"; bytes: Uint8Array }
  | { type: "page"; id: number; index: number; rowPage: number; colPage: number };

export type XlsxWorkerResponse =
  | { type: "opened"; status: "ok"; sheets: XlsxSheet[] }
  | { type: "opened"; status: "tooLarge" | "invalid" }
  | { type: "page"; id: number; page: XlsxPage | null }
  | { type: "failed" };

/** The part of a dedicated `Worker` the client uses. */
export type XlsxWorkerPort = {
  onmessage: ((event: MessageEvent<XlsxWorkerResponse>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  onmessageerror: ((event: MessageEvent) => void) | null;
  postMessage(message: XlsxWorkerRequest, transfer: Transferable[]): void;
  terminate(): void;
};

export type RemoteXlsxBook = {
  sheets: XlsxSheet[];
  /** Rejects with {@link XlsxWorkerError} on timeout, worker failure or after `close`. */
  page(index: number, rowPage: number, colPage: number): Promise<XlsxPage | null>;
  /** Terminates the worker and the workbook it holds. */
  close(): void;
};

export type RemoteXlsxOpenResult =
  | { status: "ok"; book: RemoteXlsxBook }
  /** Over a load cap, or slower than the open timeout: the file stays download-only. */
  | { status: "tooLarge" }
  /** Not a readable XLSX package, the worker failed, or `signal` aborted. */
  | { status: "invalid" };

export class XlsxWorkerError extends Error {
  readonly reason: "timeout" | "failed";

  constructor(reason: "timeout" | "failed") {
    super(`xlsx worker ${reason}`);
    this.name = "XlsxWorkerError";
    this.reason = reason;
  }
}

export type XlsxClientOptions = {
  signal?: AbortSignal;
  openTimeoutMs?: number;
  pageTimeoutMs?: number;
  createWorker?: () => XlsxWorkerPort;
};

export function createXlsxWorker(): XlsxWorkerPort {
  return new Worker(new URL("./xlsx-worker.ts", import.meta.url), { type: "module", name: "xlsx" });
}

/**
 * Opens `bytes` in a dedicated worker (`xlsx-worker.ts`), which runs
 * `openXlsx` and keeps the workbook; pages come back as plain display text.
 * Opening and each page request have a wall-clock bound; past it, the worker
 * is terminated, which stops any inflate or parse still running in it. The
 * worker is also terminated when `signal` aborts, on `close`, and on any
 * worker error. One page request is answered at a time, in order.
 */
export function openXlsxInWorker(bytes: Uint8Array, options: XlsxClientOptions = {}): Promise<RemoteXlsxOpenResult> {
  const {
    signal,
    openTimeoutMs = XLSX_OPEN_TIMEOUT_MS,
    pageTimeoutMs = XLSX_PAGE_TIMEOUT_MS,
    createWorker = createXlsxWorker,
  } = options;
  if (signal?.aborted) return Promise.resolve({ status: "invalid" });
  const worker = createWorker();
  let dead = false;
  let pending: { resolve: (response: XlsxWorkerResponse) => void; reject: (error: XlsxWorkerError) => void } | null =
    null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let queue: Promise<unknown> = Promise.resolve();
  let nextId = 0;

  const kill = (reason: "timeout" | "failed") => {
    if (dead) return;
    dead = true;
    clearTimeout(timer);
    signal?.removeEventListener("abort", onAbort);
    worker.onmessage = worker.onerror = worker.onmessageerror = null;
    worker.terminate();
    const waiting = pending;
    pending = null;
    waiting?.reject(new XlsxWorkerError(reason));
  };
  const onAbort = () => kill("failed");
  signal?.addEventListener("abort", onAbort);
  worker.onmessage = ({ data }) => {
    const waiting = pending;
    pending = null;
    clearTimeout(timer);
    if (!waiting) return kill("failed");
    if (data.type === "failed") {
      waiting.reject(new XlsxWorkerError("failed"));
      kill("failed");
    } else {
      waiting.resolve(data);
    }
  };
  worker.onerror = worker.onmessageerror = () => kill("failed");

  const request = (message: XlsxWorkerRequest, timeoutMs: number, transfer: Transferable[] = []) =>
    new Promise<XlsxWorkerResponse>((resolve, reject) => {
      if (dead) return reject(new XlsxWorkerError("failed"));
      pending = { resolve, reject };
      timer = setTimeout(() => kill("timeout"), timeoutMs);
      worker.postMessage(message, transfer);
    });

  // Transfer, not copy, when `bytes` is its whole buffer.
  const owned = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength ? bytes : bytes.slice();
  return request({ type: "open", bytes: owned }, openTimeoutMs, [owned.buffer as ArrayBuffer]).then(
    (response) => {
      if (response.type !== "opened" || response.status !== "ok") {
        kill("failed");
        return { status: response.type === "opened" && response.status === "tooLarge" ? "tooLarge" : "invalid" };
      }
      const book: RemoteXlsxBook = {
        sheets: response.sheets,
        page(index, rowPage, colPage) {
          const id = ++nextId;
          const result = queue.then(() => request({ type: "page", id, index, rowPage, colPage }, pageTimeoutMs));
          queue = result.catch(() => undefined);
          return result.then((reply) => {
            if (reply.type !== "page" || reply.id !== id) {
              kill("failed");
              throw new XlsxWorkerError("failed");
            }
            return reply.page;
          });
        },
        close: () => kill("failed"),
      };
      return { status: "ok", book };
    },
    (error: XlsxWorkerError) => ({ status: error.reason === "timeout" ? "tooLarge" : "invalid" }),
  );
}
