// Independent v1 ENCRYPTION_KEYS manifest oracle (pure logic, no I/O).
//
// The keyring seals TOTP secrets, workspace SSO client secrets and webhook
// signing secrets (`enc:v2:<kid>:...`). A restore needs every key id those
// ciphertexts name, so the backup manifest records, per key id,
// HMAC-SHA256(key, label || id): it identifies the key without revealing it,
// lets restore accept a superset keyring (rotation added keys) and refuses one
// that lacks or changed a backed-up key id. `fingerprint` is the SHA-256 of the
// canonical keyring and active id. Key material never appears in any returned
// string. This is written from the format, not from the Rust code
// (src/backup_manifest.rs), so the two can check each other.
import { createHash, createHmac, timingSafeEqual } from "node:crypto";

const LABEL = "fvoci:encryption-key-fingerprint:v1:";
// The v1 oracle used Python `re.match(...$)`, whose `$` also matches before
// one final "\n"; the optional "\n" keeps that accepted set exactly.
const KEY_ID_RE = /^[a-zA-Z0-9_-]{1,32}\n?$/;
const HEX_RE = /^[0-9a-fA-F]{64}\n?$/;
// Python str.strip() whitespace (str.isspace), which differs from
// String.prototype.trim (no U+FEFF; adds U+001C-U+001F and U+0085).
const PY_SPACE =
  "\t\n\v\f\r\x1c\x1d\x1e\x1f \x85\xa0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000";
const PY_STRIP_RE = new RegExp(`^[${PY_SPACE}]+|[${PY_SPACE}]+$`, "gu");

export const NO_KEYS_NOTE =
  "ENCRYPTION_KEYS was not set; nothing could be sealed with it. Restore runs fvoci-migrate --verify-secrets regardless.";
export const KEYS_NOTE =
  "Per key id HMAC-SHA256(key, label||id); keys are not stored. Restore needs every key id listed here with the same key (extra keys are fine) and then opens every sealed secret (fvoci-migrate --verify-secrets).";

export class KeyringError extends Error {}

export interface Keyring {
  keys: Map<string, Buffer>;
  active: string;
}

export type ManifestEntry =
  | { configured: false; note: string }
  | {
      configured: true;
      fingerprint: string;
      activeKeyId: string;
      // A Map keeps sorted id order; a plain object would hoist integer-like ids.
      keyFingerprints: Map<string, string>;
      note: string;
    };

/** Python `sorted()` order for str: by code point, not UTF-16 unit. */
export function byCodePoint(a: string, b: string): number {
  const x = Array.from(a);
  const y = Array.from(b);
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    const d = (x[i]?.codePointAt(0) ?? 0) - (y[i]?.codePointAt(0) ?? 0);
    if (d !== 0) return d;
  }
  return x.length - y.length;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The env keyring; null when neither variable is set (whitespace counts as unset). */
export function parseKeyring(raw: string | undefined, active: string | undefined): Keyring | null {
  const ring = (raw ?? "").replace(PY_STRIP_RE, "");
  const activeId = (active ?? "").replace(PY_STRIP_RE, "");
  if (!ring && !activeId) return null;
  if (!ring || !activeId) {
    throw new KeyringError("ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID must be set together");
  }
  if (!KEY_ID_RE.test(activeId)) throw new KeyringError("invalid ENCRYPTION_ACTIVE_KEY_ID");
  let parsed: unknown;
  try {
    parsed = JSON.parse(ring);
  } catch {
    throw new KeyringError("invalid ENCRYPTION_KEYS json");
  }
  if (!isPlainObject(parsed)) throw new KeyringError("ENCRYPTION_KEYS must have 1-32 keys");
  const entries = Object.entries(parsed);
  if (entries.length < 1 || entries.length > 32) {
    throw new KeyringError("ENCRYPTION_KEYS must have 1-32 keys");
  }
  const keys = new Map<string, Buffer>();
  for (const [id, value] of entries) {
    if (!KEY_ID_RE.test(id)) throw new KeyringError("invalid ENCRYPTION_KEYS key id");
    if (typeof value !== "string" || !HEX_RE.test(value)) {
      throw new KeyringError("ENCRYPTION_KEYS keys must be 64-char hex");
    }
    keys.set(id, Buffer.from(value.slice(0, 64), "hex"));
  }
  if (!keys.has(activeId))
    throw new KeyringError("ENCRYPTION_ACTIVE_KEY_ID is not in ENCRYPTION_KEYS");
  return { keys, active: activeId };
}

export function keyFingerprint(id: string, key: Buffer): string {
  return createHmac("sha256", key)
    .update(LABEL + id, "utf8")
    .digest("hex");
}

function sortedIds(keys: Map<string, Buffer>): string[] {
  return [...keys.keys()].sort(byCodePoint);
}

export function manifestEntry(ring: Keyring | null): ManifestEntry {
  if (ring === null) return { configured: false, note: NO_KEYS_NOTE };
  const ids = sortedIds(ring.keys);
  const hexKeys = new Map<string, string>();
  const fingerprints = new Map<string, string>();
  for (const id of ids) {
    const key = ring.keys.get(id) ?? Buffer.alloc(0);
    hexKeys.set(id, key.toString("hex"));
    fingerprints.set(id, keyFingerprint(id, key));
  }
  // Canonical form: keys before active, ids sorted, lowercase hex, compact.
  const canonical = `{"keys":${pyJson(hexKeys, ",", ":")},"active":${pyJson(ring.active, ",", ":")}}`;
  return {
    configured: true,
    fingerprint: createHash("sha256").update(canonical, "utf8").digest("hex"),
    activeKeyId: ring.active,
    keyFingerprints: fingerprints,
    note: KEYS_NOTE,
  };
}

function digestEqual(expected: unknown, actual: string): boolean {
  // A non-string or wrongly sized value can never equal a hex digest.
  if (typeof expected !== "string") return false;
  const a = Buffer.from(expected, "utf8");
  const b = Buffer.from(actual, "utf8");
  return a.length === b.length && timingSafeEqual(a, b);
}

/** Problems that stop a restore; they name key ids only, never key material. */
export function checkEntry(entry: unknown, ring: Keyring | null): string[] {
  if (!isPlainObject(entry) || typeof entry.configured !== "boolean") {
    return ["backup manifest encryptionKeys entry is malformed"];
  }
  if (!entry.configured) return [];
  const expected = entry.keyFingerprints;
  if (!isPlainObject(expected) || Object.keys(expected).length === 0) {
    return ["backup manifest encryptionKeys.keyFingerprints is missing"];
  }
  const ids = Object.keys(expected).sort(byCodePoint);
  if (ring === null) {
    return [
      `the backed-up install had ENCRYPTION_KEYS (key ids: ${ids.join(", ")}); set ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID`,
    ];
  }
  const missing = ids.filter((id) => !ring.keys.has(id));
  const changed = ids.filter((id) => {
    const key = ring.keys.get(id);
    return key !== undefined && !digestEqual(expected[id], keyFingerprint(id, key));
  });
  const problems: string[] = [];
  if (missing.length)
    problems.push(`ENCRYPTION_KEYS lacks backed-up key id(s): ${missing.join(", ")}`);
  if (changed.length)
    problems.push(`ENCRYPTION_KEYS has a different key for id(s): ${changed.join(", ")}`);
  return problems;
}

/**
 * Python `json.dumps` text (ensure_ascii, the given separators) for the
 * string/boolean/Map/object values a manifest entry holds, in insertion order.
 */
export function pyJson(value: unknown, item = ", ", key = ": "): string {
  if (typeof value === "string") {
    return JSON.stringify(value).replace(
      /[^\x20-\x7e]/g,
      (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`,
    );
  }
  if (typeof value === "boolean") return value ? "true" : "false";
  if (value instanceof Map || isPlainObject(value)) {
    const entries: [unknown, unknown][] = value instanceof Map ? [...value] : Object.entries(value);
    const parts = entries.map(([k, v]) => `${pyJson(k)}${key}${pyJson(v, item, key)}`);
    return `{${parts.join(item)}}`;
  }
  throw new TypeError("pyJson: unsupported value");
}
