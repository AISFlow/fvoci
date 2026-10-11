// A stateful in-memory registry + GitHub releases API behind the HttpClient
// interface, for tests and offline differential runs. It records every request
// and stores pushed manifests, so a test can tell what a failed run left behind.
import { sha256Digest as sha256 } from "./registry.ts";
import {
  FAKE_GH_TOKEN,
  GITHUB_API,
  REGISTRY_BASE,
  REGISTRY_TOKEN,
  REPOSITORY_NAME,
} from "./fixtures.ts";
import type { HttpClient, HttpRequest, HttpResponse } from "./http.ts";

export interface RecordedRequest {
  method: string;
  url: string;
  accept?: string;
  authorization?: string;
  contentType?: string;
  bodySha256?: string;
}

interface Stored {
  mediaType: string;
  body: Uint8Array;
}

export interface FakeOptions {
  /** "bearer" answers /v2/ with a token challenge, "open" with 200. */
  auth?: "bearer" | "open";
  registryBase?: string;
  githubBase?: string;
  /** Realm announced in the challenge; defaults to `${registryBase}/token`. */
  realm?: string;
  /** Pages of the GitHub releases list. */
  releasePages?: unknown[][];
  /** Consulted first; a returned response short-circuits the fake. */
  intercept?: (request: HttpRequest, path: string) => HttpResponse | undefined;
}

const encoder = new TextEncoder();
const reply = (
  status: number,
  body: string | Uint8Array = "",
  headers: Record<string, string> = {},
): HttpResponse => ({
  status,
  headers,
  body: typeof body === "string" ? encoder.encode(body) : body,
});
const errors = (code: string) =>
  JSON.stringify({ errors: [{ code, message: code.toLowerCase() }] });

export class FakeRegistry {
  readonly requests: RecordedRequest[] = [];
  readonly manifests = new Map<string, Stored>();
  readonly tags = new Map<string, string>();
  private readonly auth: "bearer" | "open";
  private readonly registryBase: string;
  private readonly githubBase: string;
  private readonly realm: string;

  constructor(private readonly options: FakeOptions = {}) {
    this.auth = options.auth ?? "bearer";
    this.registryBase = options.registryBase ?? REGISTRY_BASE;
    this.githubBase = options.githubBase ?? GITHUB_API;
    this.realm = options.realm ?? `${this.registryBase}/token`;
  }

  /** Seeds a manifest (by its digest) and optionally tags it. */
  add(mediaType: string, text: string, tag?: string): string {
    const body = encoder.encode(text);
    const digest = sha256(body);
    this.manifests.set(digest, { mediaType, body });
    if (tag !== undefined) this.tags.set(tag, digest);
    return digest;
  }

  readonly client: HttpClient = (request) => Promise.resolve(this.handle(request));

  handle(request: HttpRequest): HttpResponse {
    const headers = Object.fromEntries(
      Object.entries(request.headers).map(([k, v]) => [k.toLowerCase(), v]),
    );
    this.requests.push({
      method: request.method,
      url: request.url,
      ...(headers.accept === undefined ? {} : { accept: headers.accept }),
      ...(headers.authorization === undefined ? {} : { authorization: headers.authorization }),
      ...(headers["content-type"] === undefined ? {} : { contentType: headers["content-type"] }),
      ...(request.body === undefined ? {} : { bodySha256: sha256(request.body) }),
    });
    const url = new URL(request.url);
    const path = url.pathname + url.search;
    const intercepted = this.options.intercept?.(request, path);
    if (intercepted) return intercepted;
    if (request.url.startsWith(this.githubBase + "/")) return this.github(url, headers);
    if (request.url.startsWith(this.realm + "?")) return this.tokenEndpoint();
    if (!request.url.startsWith(this.registryBase + "/")) return reply(599, "unexpected host");
    if (url.pathname === "/v2/") {
      if (this.auth === "open") return reply(200, "{}");
      return reply(401, errors("UNAUTHORIZED"), {
        "www-authenticate": `Bearer realm="${this.realm}",service="ghcr.io",scope="repository:${REPOSITORY_NAME}:pull"`,
      });
    }
    const prefix = `/v2/${REPOSITORY_NAME}/manifests/`;
    if (!url.pathname.startsWith(prefix)) return reply(404, errors("NAME_UNKNOWN"));
    if (this.auth === "bearer" && headers.authorization !== `Bearer ${REGISTRY_TOKEN}`) {
      return reply(401, errors("UNAUTHORIZED"));
    }
    const reference = url.pathname.slice(prefix.length);
    return request.method === "GET"
      ? this.getManifest(reference)
      : this.putManifest(reference, headers, request.body);
  }

  private tokenEndpoint(): HttpResponse {
    return reply(200, JSON.stringify({ token: REGISTRY_TOKEN }), {
      "content-type": "application/json",
    });
  }

  private getManifest(reference: string): HttpResponse {
    const digest = reference.startsWith("sha256:") ? reference : this.tags.get(reference);
    const stored = digest === undefined ? undefined : this.manifests.get(digest);
    if (digest === undefined || stored === undefined) return reply(404, errors("MANIFEST_UNKNOWN"));
    return reply(200, stored.body, {
      "content-type": stored.mediaType,
      "docker-content-digest": digest,
    });
  }

  private putManifest(
    reference: string,
    headers: Record<string, string>,
    body: Uint8Array | undefined,
  ): HttpResponse {
    if (body === undefined) return reply(400, errors("MANIFEST_INVALID"));
    const digest = sha256(body);
    if (reference.startsWith("sha256:") && reference !== digest)
      return reply(400, errors("DIGEST_INVALID"));
    const document = JSON.parse(new TextDecoder().decode(body)) as {
      manifests?: { digest: string }[];
    };
    for (const child of document.manifests ?? []) {
      if (!this.manifests.has(child.digest)) return reply(400, errors("MANIFEST_BLOB_UNKNOWN"));
    }
    this.manifests.set(digest, { mediaType: headers["content-type"] ?? "", body });
    if (!reference.startsWith("sha256:")) this.tags.set(reference, digest);
    return reply(201, "", {
      "docker-content-digest": digest,
      location: `/v2/${REPOSITORY_NAME}/manifests/${digest}`,
    });
  }

  private github(url: URL, headers: Record<string, string>): HttpResponse {
    if (headers.authorization !== `Bearer ${FAKE_GH_TOKEN}`)
      return reply(401, JSON.stringify({ message: "Bad credentials" }));
    if (!url.pathname.endsWith("/releases"))
      return reply(404, JSON.stringify({ message: "Not Found" }));
    const page = Number(url.searchParams.get("page") ?? "1");
    const pages = this.options.releasePages ?? [];
    return reply(200, JSON.stringify(pages[page - 1] ?? []), {
      "content-type": "application/json",
    });
  }
}
