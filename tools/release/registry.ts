// Pure OCI distribution rules: reference syntax, auth challenge, index bytes
// and the two-platform shape. No I/O.
import { createHash } from "node:crypto";
import { Fail, repr, str } from "./fail.ts";

export const DIGEST = /^sha256:[0-9a-f]{64}$/;
export const TAG = /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$/;
/** Lowercase registry/repository, no tag, no port. */
export const IMAGE = /^[a-z0-9.-]+(\/[a-z0-9._-]+)+$/;

export const DOCKER_MANIFEST = "application/vnd.docker.distribution.manifest.v2+json";
export const DOCKER_LIST = "application/vnd.docker.distribution.manifest.list.v2+json";
export const OCI_MANIFEST = "application/vnd.oci.image.manifest.v1+json";
export const OCI_INDEX = "application/vnd.oci.image.index.v1+json";
export const ACCEPT_IMAGE = [OCI_MANIFEST, DOCKER_MANIFEST].join(", ");
export const ACCEPT_INDEX = [OCI_INDEX, DOCKER_LIST].join(", ");

export function sha256Digest(bytes: Uint8Array): string {
  return "sha256:" + createHash("sha256").update(bytes).digest("hex");
}

/** realm and service of a `Bearer realm="...",service="..."` challenge. */
export function parseChallenge(challenge: string): { realm: string; service?: string } {
  const params = new Map<string, string>();
  for (const match of challenge.matchAll(/(\w+)="([^"]*)"/g))
    params.set(match[1] ?? "", match[2] ?? "");
  const realm = params.get("realm");
  if (!challenge.toLowerCase().startsWith("bearer ") || realm === undefined) {
    throw new Fail(`unsupported registry challenge: ${repr(challenge)}`);
  }
  const service = params.get("service");
  return service === undefined ? { realm } : { realm, service };
}

/** application/x-www-form-urlencoded with only alphanumerics and `_.-~` left as is. */
function quotePlus(value: string): string {
  return encodeURIComponent(value)
    .replace(/[!'()*]/g, (ch) => `%${ch.charCodeAt(0).toString(16).toUpperCase()}`)
    .replace(/%20/g, "+");
}

/** Token endpoint URL: the realm plus scope (and service when announced). */
export function tokenUrl(realm: string, name: string, actions: string, service?: string): string {
  const query: [string, string][] = [["scope", `repository:${name}:${actions}`]];
  if (service !== undefined) query.push(["service", service]);
  return (
    realm + "?" + query.map(([key, value]) => `${quotePlus(key)}=${quotePlus(value)}`).join("&")
  );
}

export interface PlatformManifest {
  mediaType: string;
  digest: string;
  size: number;
}

/**
 * The two-platform index body. Key order and compact separators are part of
 * the digest; size is the byte length of each platform manifest.
 */
export function buildIndex(
  amd64: PlatformManifest,
  arm64: PlatformManifest,
): { mediaType: string; body: Uint8Array } {
  const manifests = (
    [
      ["amd64", amd64],
      ["arm64", arm64],
    ] as const
  ).map(([architecture, m]) => ({
    mediaType: m.mediaType,
    digest: m.digest,
    size: m.size,
    platform: { architecture, os: "linux" },
  }));
  const docker = manifests.every((m) => m.mediaType === DOCKER_MANIFEST);
  const mediaType = docker ? DOCKER_LIST : OCI_INDEX;
  const body = new TextEncoder().encode(JSON.stringify({ schemaVersion: 2, mediaType, manifests }));
  return { mediaType, body };
}

/** Media type of a manifest: its mediaType field, else the Content-Type header. */
export function manifestMediaType(document: unknown, contentType: string): string {
  if (typeof document !== "object" || document === null || Array.isArray(document)) {
    throw new Fail("manifest is not a JSON object");
  }
  const field = (document as Record<string, unknown>).mediaType;
  if (field && typeof field !== "string")
    throw new Fail(`manifest mediaType ${repr(field)} is not a string`);
  return ((field as string) || contentType).split(";")[0]?.trim() ?? "";
}

/** linux/amd64 and linux/arm64 digests of an index, refusing any other shape. */
export function indexPlatforms(
  index: Record<string, unknown>,
  where: string,
): { amd64: string; arm64: string } {
  const entries = index.manifests === undefined ? [] : index.manifests;
  if (!Array.isArray(entries)) throw new Fail(`${where}: manifests is not a list`);
  const platforms = new Map<string, string>();
  for (const entry of entries as unknown[]) {
    if (typeof entry !== "object" || entry === null)
      throw new Fail(`${where}: manifest entry is not an object`);
    const record = entry as Record<string, unknown>;
    const platform = (record.platform ?? {}) as Record<string, unknown>;
    if (typeof platform !== "object") throw new Fail(`${where}: platform is not an object`);
    const key = `${str(platform.os)}/${str(platform.architecture)}`;
    if (platforms.has(key) || (key !== "linux/amd64" && key !== "linux/arm64")) {
      throw new Fail(`${where}: unexpected or repeated platform ${key}`);
    }
    if (typeof record.digest !== "string" || !DIGEST.test(record.digest)) {
      // Printed into $GITHUB_OUTPUT by the workflow: only a well-formed digest may pass.
      throw new Fail(`${where}: ${key} digest ${repr(record.digest)} is not sha256:<64 hex>`);
    }
    platforms.set(key, record.digest);
  }
  const amd64 = platforms.get("linux/amd64");
  const arm64 = platforms.get("linux/arm64");
  if (amd64 === undefined || arm64 === undefined) {
    throw new Fail(
      `${where}: platforms ${repr([...platforms.keys()].sort())} are not linux/amd64 and linux/arm64`,
    );
  }
  return { amd64, arm64 };
}
