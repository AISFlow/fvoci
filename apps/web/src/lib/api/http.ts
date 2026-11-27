/**
 * Requests to the server's API (`contracts/openapi.json`): its paths, the
 * header a changing request needs, and its errors as `BackendError`.
 *
 * The session is the cookie the server set at login. The browser sends it by
 * itself on a request to the page's own origin; a script never sees it. A
 * changing request goes in `fetch`'s default mode, `cors`: the server sends
 * its pages with `Referrer-Policy: no-referrer`, and under that policy any
 * other mode writes `Origin: null`, which the server refuses (the contract's
 * preamble, "Requests that change something").
 */
import {
  BackendError,
  ERROR_STATUSES,
  sessionOver,
  type ErrorStatus,
  type InvalidInput,
  type Problem,
} from '../state/backend.ts';

import type { paths } from './contract.ts';

/** The paths of the contract, as they are written there. */
export type ApiPath = keyof paths;

/** The media type of the server's error answers (RFC 9457). */
const PROBLEM = 'application/problem+json';

/** The status an error answer is told by. One the boundary does not know is read as the server's own failure. */
function errorStatus(status: number): ErrorStatus {
  return (ERROR_STATUSES as readonly number[]).includes(status) ? (status as ErrorStatus) : 500;
}

const isText = (value: unknown): value is string => typeof value === 'string';
const isCount = (value: unknown): value is number => typeof value === 'number';

/** The entries of `errors` that say where they are and why; anything else is left out. */
function invalidInputs(value: unknown): InvalidInput[] {
  if (!Array.isArray(value)) return [];
  const entries: InvalidInput[] = [];
  for (const item of value as unknown[]) {
    if (typeof item !== 'object' || item === null) continue;
    const { pointer, parameter, header, detail, reason } = item as Record<string, unknown>;
    if (!isText(reason)) continue;
    entries.push({
      ...(isText(pointer) ? { pointer } : {}),
      ...(isText(parameter) ? { parameter } : {}),
      ...(isText(header) ? { header } : {}),
      detail: isText(detail) ? detail : '',
      reason,
    });
  }
  return entries;
}

/**
 * The server's problem details of an error answer. An answer that is not one
 * (a proxy's page in the server's place, say) has only its status to go by.
 */
async function problemOf(response: Response): Promise<Partial<Problem> | undefined> {
  if (!(response.headers.get('Content-Type') ?? '').startsWith(PROBLEM)) return undefined;
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    return undefined;
  }
  if (typeof body !== 'object' || body === null) return undefined;
  const { type, title, detail, reason, errors, limit, retryAfter, requestId, orgId } =
    body as Record<string, unknown>;
  return {
    ...(isText(type) ? { type } : {}),
    ...(isText(title) ? { title } : {}),
    ...(isText(detail) ? { detail } : {}),
    ...(isText(reason) ? { reason } : {}),
    errors: invalidInputs(errors),
    ...(isCount(limit) ? { limit } : {}),
    ...(isCount(retryAfter) ? { retryAfter } : {}),
    ...(isText(requestId) ? { requestId } : {}),
    ...(isText(orgId) ? { orgId } : {}),
  };
}

/**
 * The refusal an answer that is no success stands for, read from its body.
 * Every request reads its errors here; so does a caller that made its own
 * request (a download read as it arrives, `download.ts`).
 */
export async function errorOf(response: Response): Promise<BackendError> {
  return new BackendError(errorStatus(response.status), await problemOf(response));
}

export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE';
  /** Query parameters; an undefined one is left out. */
  query?: Record<string, string | number | undefined>;
  /** Sent as JSON. */
  json?: unknown;
  /** Sent as they are (`application/octet-stream`), in place of `json`. */
  bytes?: Blob;
  /** More headers. */
  headers?: Record<string, string>;
  /** Gives the request up; it then rejects with the signal's reason. */
  signal?: AbortSignal;
  /** Sent even as the page goes (a small request: the browser takes 64 KiB of them at once). */
  keepalive?: boolean;
  /**
   * Success replaces the session cookie (`Set-Cookie`): concurrent requests
   * refused as signed out under the previous cookie wait for this to settle
   * and retry under the new cookie.
   */
  rotates?: boolean;
}

/**
 * Maximum retries a request may make for back-to-back session-cookie rotations
 * (e.g. setting a first password immediately followed by confirming it).
 */
const MAX_ROTATION_RETRIES = 2;

function abortError(signal: AbortSignal): Error {
  return signal.reason instanceof Error ? signal.reason : new DOMException('Aborted', 'AbortError');
}

function waitWithAbort(promise: Promise<unknown>, signal?: AbortSignal): Promise<void> {
  if (!signal) return promise.then(() => undefined);
  if (signal.aborted) return Promise.reject(abortError(signal));
  return new Promise((resolve, reject) => {
    const onAbort = (): void => {
      reject(abortError(signal));
    };
    const cleanup = (): void => {
      signal.removeEventListener('abort', onAbort);
    };
    signal.addEventListener('abort', onAbort, { once: true });
    promise.then(
      () => {
        cleanup();
        resolve();
      },
      (error: unknown) => {
        cleanup();
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });
}

/** Talks to one server: the one the page came from, under the path the app is served at. */
export class Http {
  /** The path the app is served under, without a trailing slash: `''` at the root. */
  readonly basePath: string;
  /** Whether the session's cookie goes along: not for a reader by a board's link. */
  readonly #credentials: RequestCredentials;
  /** Count of session-cookie rotations that have succeeded on this client. */
  #rotation = 0;
  /** In-flight requests whose success replaces the session cookie. */
  readonly #rotations = new Set<Promise<void>>();

  constructor(basePath: string, credentials: RequestCredentials = 'same-origin') {
    this.basePath = basePath.replace(/\/$/, '');
    this.#credentials = credentials;
  }

  /** The address of an API path, its `{name}` parts filled in from `params`. */
  url(path: ApiPath, params: Record<string, string> = {}, query: RequestOptions['query'] = {}) {
    const filled = path.replace(/\{(\w+)\}/g, (_, name: string) => {
      const value = params[name];
      if (value === undefined) throw new Error(`no value for {${name}} in ${path}`);
      return encodeURIComponent(value);
    });
    const search = new URLSearchParams();
    for (const [name, value] of Object.entries(query)) {
      if (value !== undefined) search.set(name, String(value));
    }
    const text = search.toString();
    return `${this.basePath}${filled}${text ? `?${text}` : ''}`;
  }

  async #trackRotation<T>(run: () => Promise<T>): Promise<T> {
    let settled = (): void => undefined;
    const done = new Promise<void>((resolve) => {
      settled = resolve;
    });
    this.#rotations.add(done);
    try {
      const result = await run();
      this.#rotation += 1;
      return result;
    } finally {
      this.#rotations.delete(done);
      settled();
    }
  }

  async #rotatedSince(
    epoch: number,
    pending: readonly Promise<void>[],
    noAnswer: string,
    signal: AbortSignal | undefined,
    rethrowAbort: boolean,
  ): Promise<boolean> {
    const waiting = new Set([...pending, ...this.#rotations]);
    if (waiting.size > 0) {
      try {
        await waitWithAbort(Promise.all(waiting), signal);
      } catch (error) {
        if (rethrowAbort) throw error;
        throw new BackendError(0, { detail: noAnswer });
      }
    }
    return this.#rotation !== epoch;
  }

  async #fetch(
    send: () => Promise<Response>,
    noAnswer: string,
    signal: AbortSignal | undefined,
    rethrowAbort: boolean,
  ): Promise<Response> {
    try {
      return await send();
    } catch (error) {
      if (rethrowAbort && signal?.aborted) throw error;
      throw new BackendError(0, { detail: noAnswer });
    }
  }

  /**
   * Sends one request, retrying when a concurrent session-cookie rotation
   * (`rotates`) succeeded while this request was sent under the previous cookie.
   * Genuine session loss (no rotation, or still 401 after rotation) throws as is.
   */
  async #send(
    send: () => Promise<Response>,
    noAnswer: string,
    options: { rotates?: boolean; signal?: AbortSignal; rethrowAbort: boolean },
  ): Promise<Response> {
    if (options.rotates) {
      return this.#trackRotation(async () => {
        const response = await this.#fetch(send, noAnswer, options.signal, options.rethrowAbort);
        if (!response.ok) throw await errorOf(response);
        return response;
      });
    }
    for (let attempt = 0; ; attempt += 1) {
      const epoch = this.#rotation;
      const pending = [...this.#rotations];
      const response = await this.#fetch(send, noAnswer, options.signal, options.rethrowAbort);
      if (response.ok) return response;
      const error = await errorOf(response);
      if (
        attempt < MAX_ROTATION_RETRIES &&
        sessionOver(error) === 'signedOut' &&
        (await this.#rotatedSince(epoch, pending, noAnswer, options.signal, options.rethrowAbort))
      ) {
        continue;
      }
      throw error;
    }
  }

  /** Reads the bytes at a path (a file), until `signal` gives up. Rejects as `request` does. */
  async bytes(
    path: ApiPath,
    params: Record<string, string> = {},
    query: RequestOptions['query'] = {},
    signal?: AbortSignal,
  ): Promise<Blob> {
    const address = this.url(path, params, query);
    const response = await this.#send(
      () => fetch(address, { credentials: this.#credentials, signal }),
      `GET ${path}: no answer`,
      { signal, rethrowAbort: false },
    );
    return response.blob();
  }

  /**
   * Sends a request and reads the answer: its JSON, or undefined for 204.
   * Rejects with a `BackendError`: the server's problem details, or status 0
   * when no answer came.
   */
  async request<T>(
    path: ApiPath,
    params: Record<string, string> = {},
    options: RequestOptions = {},
  ): Promise<T> {
    const method = options.method ?? 'GET';
    const headers: Record<string, string> = { Accept: `application/json, ${PROBLEM}` };
    // Every request but a reading one needs it; with it, a page of another
    // origin cannot make the browser send one (contract, "Requests that change something").
    if (method !== 'GET') headers['X-Requested-With'] = 'XMLHttpRequest';
    Object.assign(headers, options.headers);
    let body: string | Blob | undefined;
    if (options.bytes !== undefined) {
      headers['Content-Type'] = 'application/octet-stream';
      body = options.bytes;
    } else if (options.json !== undefined) {
      headers['Content-Type'] = 'application/json';
      body = JSON.stringify(options.json);
    }
    const address = this.url(path, params, options.query);
    const response = await this.#send(
      () =>
        fetch(address, {
          method,
          headers,
          body,
          credentials: this.#credentials,
          signal: options.signal,
          keepalive: options.keepalive,
        }),
      `${method} ${path}: no answer`,
      { rotates: options.rotates, signal: options.signal, rethrowAbort: true },
    );
    // 204 says nothing, and 202 that the work waits for something (an address to
    // be confirmed): neither has a body.
    if (response.status === 204 || response.status === 202) return undefined as T;
    try {
      return (await response.json()) as T;
    } catch {
      // Something between answered in the server's place (a proxy's page, say).
      throw new BackendError(0, { detail: `${method} ${path}: not the server's answer` });
    }
  }
}
