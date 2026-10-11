// The only network boundary of the release tools. Commands receive an
// HttpClient; the CLI passes fetchClient, tests pass a fake or refusingClient.

export interface HttpRequest {
  url: string;
  method: "GET" | "PUT";
  headers: Record<string, string>;
  body?: Uint8Array;
}

export interface HttpResponse {
  status: number;
  /** Lowercase header names. */
  headers: Record<string, string>;
  body: Uint8Array;
}

export type HttpClient = (request: HttpRequest) => Promise<HttpResponse>;

/** Any status outside 2xx. */
export class HttpStatus extends Error {
  constructor(readonly response: HttpResponse) {
    super(`HTTP ${String(response.status)}`);
  }
}

const TIMEOUT_MS = 60_000;

/**
 * GET follows redirects; PUT does not, so a redirected push surfaces as a
 * non-2xx status instead of being replayed. Identity encoding keeps manifest
 * bytes exactly as the registry stores them (their sha256 is the digest).
 */
export const fetchClient: HttpClient = async (request) => {
  const response = await fetch(request.url, {
    method: request.method,
    headers: { "accept-encoding": "identity", ...request.headers },
    body: request.body,
    redirect: request.method === "GET" ? "follow" : "manual",
    signal: AbortSignal.timeout(TIMEOUT_MS),
  });
  const headers: Record<string, string> = {};
  response.headers.forEach((value, name) => {
    headers[name.toLowerCase()] = value;
  });
  return { status: response.status, headers, body: new Uint8Array(await response.arrayBuffer()) };
};

/** Refuses every request; the default client of the tests. */
export const refusingClient: HttpClient = (request) =>
  Promise.reject(new Error(`network refused in tests: ${request.method} ${request.url}`));

/** Sends a request and throws HttpStatus for any non-2xx answer. */
export async function send(client: HttpClient, request: HttpRequest): Promise<HttpResponse> {
  const response = await client(request);
  if (response.status < 200 || response.status > 299) throw new HttpStatus(response);
  return response;
}
