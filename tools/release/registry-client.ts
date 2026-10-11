// Registry calls over an injected HttpClient: bearer token, manifest read with
// digest verification, manifest push. Decisions use status codes only.
import { EXIT_UNREADABLE, Fail, parseJson, repr, str } from "./fail.ts";
import { HttpStatus, send, type HttpClient } from "./http.ts";
import {
  DIGEST,
  IMAGE,
  manifestMediaType,
  parseChallenge,
  sha256Digest,
  tokenUrl,
} from "./registry.ts";

export interface Manifest {
  digest: string;
  mediaType: string;
  body: Uint8Array;
}

export interface Credentials {
  user?: string;
  password?: string;
}

export class Registry {
  private constructor(
    private readonly http: HttpClient,
    readonly name: string,
    private readonly base: string,
    private readonly token: string | null,
  ) {}

  /** Validates the image and fetches a token scoped to `actions` (pull or pull,push). */
  static async open(
    http: HttpClient,
    image: string,
    actions: string,
    credentials: Credentials,
  ): Promise<Registry> {
    if (!IMAGE.test(image))
      throw new Fail(`--image ${repr(image)} must be a lowercase registry/repository without tag`);
    const slash = image.indexOf("/");
    const host = image.slice(0, slash);
    const name = image.slice(slash + 1);
    const token = await fetchToken(http, host, name, actions, credentials);
    return new Registry(http, name, `https://${host}/v2/${name}`, token);
  }

  private headers(extra: Record<string, string>): Record<string, string> {
    return this.token ? { ...extra, Authorization: `Bearer ${this.token}` } : extra;
  }

  /** The manifest a tag or digest names, null on 404. */
  async manifest(reference: string, accept: string): Promise<Manifest | null> {
    let response;
    try {
      response = await send(this.http, {
        url: `${this.base}/manifests/${reference}`,
        method: "GET",
        headers: this.headers({ Accept: accept }),
      });
    } catch (error) {
      if (!(error instanceof HttpStatus)) throw error;
      const status = error.response.status;
      if (status === 404) return null;
      if (status === 401 || status === 403) {
        throw new Fail(
          `${this.name}:${reference} not readable (HTTP ${String(status)})`,
          EXIT_UNREADABLE,
        );
      }
      throw new Fail(`${this.name}:${reference}: HTTP ${String(status)}`);
    }
    const digest = sha256Digest(response.body);
    const announced = response.headers["docker-content-digest"];
    if (announced && announced !== digest) {
      throw new Fail(
        `${this.name}:${reference}: registry digest ${announced} != content digest ${digest}`,
      );
    }
    if (DIGEST.test(reference) && reference !== digest) {
      throw new Fail(`${this.name}@${reference}: content digest is ${digest}`);
    }
    const mediaType = manifestMediaType(
      parseJson(response.body),
      response.headers["content-type"] ?? "",
    );
    return { digest, mediaType, body: response.body };
  }

  /** Pushes manifest bytes under a tag or digest; requires 201 and a matching digest. */
  async put(reference: string, mediaType: string, body: Uint8Array): Promise<string> {
    const digest = sha256Digest(body);
    let response;
    try {
      response = await send(this.http, {
        url: `${this.base}/manifests/${reference}`,
        method: "PUT",
        headers: this.headers({ "Content-Type": mediaType }),
        body,
      });
    } catch (error) {
      if (!(error instanceof HttpStatus)) throw error;
      const detail = new TextDecoder().decode(error.response.body.slice(0, 300));
      throw new Fail(
        `push ${this.name}:${reference}: HTTP ${String(error.response.status)} ${repr(detail)}`,
      );
    }
    const announced = response.headers["docker-content-digest"];
    if (response.status !== 201 || (announced && announced !== digest)) {
      throw new Fail(
        `push ${this.name}:${reference}: HTTP ${String(response.status)}, digest ${str(announced)} != ${digest}`,
      );
    }
    return digest;
  }
}

/** Bearer token from the realm the registry announces, null for an open registry. */
async function fetchToken(
  http: HttpClient,
  host: string,
  name: string,
  actions: string,
  credentials: Credentials,
): Promise<string | null> {
  let challenge: string;
  try {
    await send(http, { url: `https://${host}/v2/`, method: "GET", headers: {} });
    return null;
  } catch (error) {
    if (!(error instanceof HttpStatus)) throw error;
    if (error.response.status !== 401)
      throw new Fail(`https://${host}/v2/ answered HTTP ${String(error.response.status)}`);
    challenge = error.response.headers["www-authenticate"] ?? "";
  }
  const { realm, service } = parseChallenge(challenge);
  const headers: Record<string, string> = {};
  if (credentials.user && credentials.password) {
    headers.Authorization =
      "Basic " + Buffer.from(`${credentials.user}:${credentials.password}`).toString("base64");
  }
  let body: Uint8Array;
  try {
    body = (
      await send(http, { url: tokenUrl(realm, name, actions, service), method: "GET", headers })
    ).body;
  } catch (error) {
    if (error instanceof HttpStatus) {
      const status = error.response.status;
      if (status === 401 || status === 403) {
        throw new Fail(
          `registry token for ${name} refused (HTTP ${String(status)})`,
          EXIT_UNREADABLE,
        );
      }
      throw new Fail(`registry token for ${name}: HTTP ${String(status)}`);
    }
    throw error;
  }
  const payload = parseJson(body);
  if (typeof payload !== "object" || payload === null || Array.isArray(payload)) {
    throw new Fail(`registry token response for ${name} is not a JSON object`);
  }
  const fields = payload as Record<string, unknown>;
  const token = fields.token || fields.access_token;
  if (!token) return null;
  if (typeof token !== "string") throw new Fail(`registry token for ${name} is not a string`);
  return token;
}
