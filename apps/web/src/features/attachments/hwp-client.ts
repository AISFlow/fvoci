import type { HwpExportFormat } from "./hwp-edit.ts";
import type { HwpFailure, HwpReplaceOutcome, HwpRequest, HwpResponse } from "./hwp-worker-core.ts";

/** A parse (package check + rhwp) that runs longer is terminated. */
export const HWP_OPEN_TIMEOUT_MS = 60_000;

/**
 * Page text, page render, replace and export requests are terminated after
 * this long; a revert re-parses the original and gets the open deadline.
 */
export const HWP_REQUEST_TIMEOUT_MS = 30_000;

/** What the client needs from a `Worker` (a fake in tests). */
export type HwpWorkerPort = {
  postMessage(message: HwpRequest, transfer?: Transferable[]): void;
  terminate(): void;
  onmessage: ((event: MessageEvent<HwpResponse>) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
  onmessageerror: ((event: MessageEvent) => void) | null;
};

/** `timeout` and `closed` also mean the worker is gone. */
export class HwpClientError extends Error {
  readonly reason: HwpFailure | "timeout" | "closed";

  constructor(reason: HwpFailure | "timeout" | "closed") {
    super(`hwp worker: ${reason}`);
    this.reason = reason;
  }
}

export type HwpClientOptions = {
  createWorker?: () => HwpWorkerPort;
  openTimeoutMs?: number;
  requestTimeoutMs?: number;
  /** Aborting terminates the worker at once, even mid-parse; `open` fails with `closed`. */
  signal?: AbortSignal;
};

type Pending = {
  resolve: (response: HwpResponse & { ok: true }) => void;
  reject: (error: HwpClientError) => void;
  timer: ReturnType<typeof setTimeout>;
};

function createHwpWorker(): HwpWorkerPort {
  return new Worker(new URL("./hwp-worker.ts", import.meta.url), { type: "module", name: "hwp" });
}

type RequestBody =
  | { op: "open"; bytes: Uint8Array; module: WebAssembly.Module }
  | { op: "startPage"; chunk: number }
  | { op: "render"; page: number }
  | { op: "replace"; find: string; replacement: string; all: boolean }
  | { op: "revert" }
  | { op: "export"; format: HwpExportFormat };

/**
 * One HWP/HWPX document in its own module worker. rhwp's wasm memory only
 * grows, and a parse cannot be interrupted, so the worker — not the document
 * — is the unit of cleanup: aborting the open's signal or `close()`
 * terminates it (unmount, attachment switch, retry), and a request past its
 * deadline terminates it too. After that every request fails with `closed`.
 */
export class HwpDocumentClient {
  readonly #worker: HwpWorkerPort;
  readonly #requestTimeoutMs: number;
  readonly #openTimeoutMs: number;
  readonly #pending = new Map<number, Pending>();
  #nextId = 0;
  #closed = false;

  private constructor(worker: HwpWorkerPort, requestTimeoutMs: number, openTimeoutMs: number) {
    this.#worker = worker;
    this.#requestTimeoutMs = requestTimeoutMs;
    this.#openTimeoutMs = openTimeoutMs;
    worker.onmessage = (event) => this.#receive(event.data);
    worker.onerror = () => this.#fail("failed");
    worker.onmessageerror = () => this.#fail("failed");
  }

  /**
   * Starts a worker and opens `bytes` (transferred to it) there. A failure
   * or an abort of `options.signal` terminates the worker before rejecting;
   * an already aborted signal starts no worker.
   */
  static async open(
    bytes: Uint8Array,
    module: WebAssembly.Module,
    options: HwpClientOptions = {},
  ): Promise<{ client: HwpDocumentClient; pageCount: number }> {
    const { signal } = options;
    if (signal?.aborted) throw new HwpClientError("closed");
    const openTimeoutMs = options.openTimeoutMs ?? HWP_OPEN_TIMEOUT_MS;
    const client = new HwpDocumentClient(
      (options.createWorker ?? createHwpWorker)(),
      options.requestTimeoutMs ?? HWP_REQUEST_TIMEOUT_MS,
      openTimeoutMs,
    );
    const abort = () => client.close();
    signal?.addEventListener("abort", abort, { once: true });
    try {
      const opened = await client.#request(
        { op: "open", bytes, module },
        openTimeoutMs,
        [bytes.buffer as ArrayBuffer],
      );
      if (opened.op !== "open") throw new HwpClientError("failed");
      return { client, pageCount: opened.pageCount };
    } catch (error) {
      client.close();
      throw error instanceof HwpClientError ? error : new HwpClientError("failed");
    } finally {
      // Past the open the caller owns the client and closes it.
      signal?.removeEventListener("abort", abort);
    }
  }

  get closed(): boolean {
    return this.#closed;
  }

  /** The 0-based page holding search chunk `chunk`. */
  async startPage(chunk: number): Promise<number> {
    const response = await this.#request({ op: "startPage", chunk }, this.#requestTimeoutMs);
    if (response.op !== "startPage") throw new HwpClientError("failed");
    return response.page;
  }

  /** Page `page` as an SVG blob (shown only through `<img>`). */
  async renderPage(page: number): Promise<Blob> {
    const response = await this.#request({ op: "render", page }, this.#requestTimeoutMs);
    if (response.op !== "render") throw new HwpClientError("failed");
    return response.svg;
  }

  /** Replaces the first (`all` false) or every case-sensitive match of `find`. */
  async replace(
    find: string,
    replacement: string,
    all: boolean,
  ): Promise<{ outcome: HwpReplaceOutcome; pageCount: number }> {
    const response = await this.#request({ op: "replace", find, replacement, all }, this.#requestTimeoutMs);
    if (response.op !== "replace") throw new HwpClientError("failed");
    return { outcome: response.outcome, pageCount: response.pageCount };
  }

  /** Discards every edit by re-parsing the original bytes; resolves to the page count. */
  async revert(): Promise<number> {
    const response = await this.#request({ op: "revert" }, this.#openTimeoutMs);
    if (response.op !== "revert") throw new HwpClientError("failed");
    return response.pageCount;
  }

  /** The current (edited) document written as `format`. */
  async exportDocument(format: HwpExportFormat): Promise<Uint8Array> {
    const response = await this.#request({ op: "export", format }, this.#requestTimeoutMs);
    if (response.op !== "export") throw new HwpClientError("failed");
    return response.bytes;
  }

  /** Terminates the worker, releasing the document; pending requests fail. */
  close(): void {
    this.#fail("closed");
  }

  #request(
    body: RequestBody,
    timeoutMs: number,
    transfer: Transferable[] = [],
  ): Promise<HwpResponse & { ok: true }> {
    if (this.#closed) return Promise.reject(new HwpClientError("closed"));
    const id = (this.#nextId += 1);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.#fail("timeout"), timeoutMs);
      this.#pending.set(id, { resolve, reject, timer });
      try {
        this.#worker.postMessage({ id, ...body } as HwpRequest, transfer);
      } catch {
        this.#fail("failed");
      }
    });
  }

  #receive(response: HwpResponse): void {
    const pending = this.#pending.get(response.id);
    if (!pending) return;
    this.#pending.delete(response.id);
    clearTimeout(pending.timer);
    if (response.ok) pending.resolve(response);
    else pending.reject(new HwpClientError(response.error));
  }

  #fail(reason: "failed" | "timeout" | "closed"): void {
    if (!this.#closed) {
      this.#closed = true;
      this.#worker.onmessage = null;
      this.#worker.onerror = null;
      this.#worker.onmessageerror = null;
      this.#worker.terminate();
    }
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new HwpClientError(reason));
    }
    this.#pending.clear();
  }
}
