/**
 * A renderer's worker as the frame talks to it: one request at a time, each
 * with a time to be answered in.
 *
 * The worker answers each request with one message, in order. A request is
 * sent once the one before it is answered, so its time counts its own work
 * only; one given up (`signal`) before it is sent is not sent. A request not
 * answered in time stops the worker (`terminate()`) and fails as
 * `SlowRenderer`, with every request after it. A client may be made with no
 * time, and given one later (`limitEach`): until then it waits as long as it
 * takes, and whoever made it keeps the time. So does closing the client,
 * as `WorkerClosed`, and the worker failing on its own (an uncaught error, a
 * message that cannot be read), as `WorkerFailed`. A stopped worker takes its
 * memory with it: the WebAssembly the renderer grew and the document it read.
 */
import { SlowRenderer } from './renderer.ts';

/** The part of a `Worker` the client uses; tests give it another. */
export interface WorkerLike {
  postMessage(message: unknown, transfer: Transferable[]): void;
  terminate(): void;
  addEventListener(type: 'message', listener: (event: MessageEvent) => void): void;
  addEventListener(type: 'error' | 'messageerror', listener: (event: Event) => void): void;
}

/** The worker was closed while the request waited. */
export class WorkerClosed extends Error {}

/** The worker failed on its own: an uncaught error, or a message it could not read. */
export class WorkerFailed extends Error {}

interface Pending {
  message: unknown;
  transfer: Transferable[];
  signal: AbortSignal | undefined;
  resolve: (answer: unknown) => void;
  reject: (error: Error) => void;
}

export class WorkerClient {
  readonly #worker: WorkerLike;
  #limit: number | undefined;
  readonly #queue: Pending[] = [];
  #current: Pending | undefined;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #closed: Error | undefined;

  /** `limit`: how long each request may take, in milliseconds; undefined for as long as it takes. */
  constructor(worker: WorkerLike, limit: number | undefined) {
    this.#worker = worker;
    this.#limit = limit;
    worker.addEventListener('message', (event) => {
      this.#answer(event.data);
    });
    const failed = (): void => {
      this.close(new WorkerFailed());
    };
    worker.addEventListener('error', failed);
    worker.addEventListener('messageerror', failed);
  }

  /** Whether the worker was stopped, and why. */
  get closed(): Error | undefined {
    return this.#closed;
  }

  /** Sends `message` once the requests before it are answered; resolves with the answer. */
  request(message: unknown, transfer: Transferable[] = [], signal?: AbortSignal): Promise<unknown> {
    if (this.#closed) return Promise.reject(this.#closed);
    return new Promise((resolve, reject) => {
      this.#queue.push({ message, transfer, signal, resolve, reject });
      this.#next();
    });
  }

  #next(): void {
    while (!this.#current && !this.#closed) {
      const pending = this.#queue.shift();
      if (!pending) return;
      if (pending.signal?.aborted) {
        pending.reject(new DOMException('Given up', 'AbortError'));
        continue;
      }
      this.#current = pending;
      this.#time();
      this.#worker.postMessage(pending.message, pending.transfer);
    }
  }

  /** Gives the request under way its time, from now. */
  #time(): void {
    clearTimeout(this.#timer);
    if (this.#limit === undefined) return;
    this.#timer = setTimeout(() => {
      this.close(new SlowRenderer());
    }, this.#limit);
  }

  /**
   * From now on each request may take `limit`, in milliseconds: the one under
   * way counted from now, and each after it from when it is sent.
   */
  limitEach(limit: number): void {
    this.#limit = limit;
    if (this.#current && !this.#closed) this.#time();
  }

  #answer(data: unknown): void {
    const pending = this.#current;
    if (!pending) return;
    clearTimeout(this.#timer);
    this.#current = undefined;
    pending.resolve(data);
    this.#next();
  }

  /** Stops the worker; what waits fails with `reason`. Closing again does nothing. */
  close(reason: Error = new WorkerClosed()): void {
    if (this.#closed) return;
    this.#closed = reason;
    clearTimeout(this.#timer);
    this.#worker.terminate();
    const waiting = [this.#current, ...this.#queue.splice(0)];
    this.#current = undefined;
    for (const pending of waiting) pending?.reject(reason);
  }
}
