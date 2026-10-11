// The v1 self-test vectors and negative checks, plus the fixed vectors that
// src/backup_manifest.rs (python_v1_fingerprint_vectors) asserts for Rust.
import {
  checkEntry,
  type Keyring,
  KeyringError,
  type ManifestEntry,
  manifestEntry,
  parseKeyring,
  pyJson,
} from "./encryption-keyring.ts";

function assert(condition: boolean, message: string): asserts condition {
  if (!condition) throw new Error(`encryption_keys self-test failed: ${message}`);
}

function sameList(actual: string[], expected: string[], message: string) {
  assert(
    JSON.stringify(actual) === JSON.stringify(expected),
    `${message}: ${JSON.stringify(actual)}`,
  );
}

function ring(keys: Record<string, string>, active: string): Keyring {
  const parsed = parseKeyring(JSON.stringify(keys), active);
  assert(parsed !== null, "test keyring parsed as unset");
  return parsed;
}

function configured(entry: ManifestEntry) {
  assert(entry.configured, "entry is not configured");
  return entry;
}

export function selfTest(): void {
  const k1 = "11".repeat(32);
  const k2 = "22".repeat(32);
  const k3 = "33".repeat(32);
  const backed = ring({ k1, k2 }, "k2");
  const entry = configured(manifestEntry(backed));
  const blob = pyJson(entry);
  assert(!blob.includes(k1) && !blob.includes(k2), "keys leaked into the manifest");
  assert(
    entry.keyFingerprints.get("k1") !== entry.keyFingerprints.get("k2"),
    "fingerprints collide",
  );
  // Restore checks the entry as the manifest file stores it.
  const stored: unknown = JSON.parse(blob);
  // Same keyring, hex case and key order do not matter.
  sameList(checkEntry(stored, ring({ k2: k2.toUpperCase(), k1 }, "k2")), [], "same keyring");
  // Rotation: a superset with another active key is accepted.
  sameList(checkEntry(stored, ring({ k1, k2, k3 }, "k3")), [], "superset keyring");
  // A missing or changed backed-up key id is refused, naming ids only.
  sameList(
    checkEntry(stored, ring({ k2, k3 }, "k3")),
    ["ENCRYPTION_KEYS lacks backed-up key id(s): k1"],
    "missing key id",
  );
  const changed = checkEntry(stored, ring({ k1: k3, k2 }, "k2"));
  sameList(changed, ["ENCRYPTION_KEYS has a different key for id(s): k1"], "changed key");
  assert(
    changed.every((p) => !p.includes(k3)),
    "key material in a problem",
  );
  // Same key under another id is not the same key id.
  assert(checkEntry(stored, ring({ x1: k1, k2 }, "k2")).length > 0, "renamed id accepted");
  // Unset in the restore env while the backup had keys: refused.
  assert(checkEntry(stored, null).length > 0, "unset keyring accepted");
  // Backup without keys: anything is accepted here (the decrypt probe decides).
  const noneEntry = manifestEntry(null);
  assert(!noneEntry.configured, "unset keyring is configured");
  const storedNone: unknown = JSON.parse(pyJson(noneEntry));
  sameList(checkEntry(storedNone, null), [], "unset backup, unset env");
  sameList(checkEntry(storedNone, backed), [], "unset backup, set env");
  // Malformed entries and keyrings.
  assert(
    checkEntry({ configured: true }, backed).length > 0,
    "entry without fingerprints accepted",
  );
  assert(checkEntry("x", backed).length > 0, "non-object entry accepted");
  const refused: [string, string][] = [
    ["{}", "k1"],
    [JSON.stringify({ k1: "zz" }), "k1"],
    [JSON.stringify({ k1 }), "k2"],
    [JSON.stringify({ "bad id": k1 }), "bad id"],
    ["not json", "k1"],
    [JSON.stringify({ k1 }), ""],
    ["", "k1"],
  ];
  for (const [raw, active] of refused) {
    try {
      parseKeyring(raw, active);
    } catch (error) {
      assert(error instanceof KeyringError, `unexpected error for ${raw}`);
      assert(!error.message.includes(k1), "key material in a keyring error");
      continue;
    }
    throw new Error(`encryption_keys self-test failed: accepted ${raw}`);
  }
  assert(
    parseKeyring("", "") === null && parseKeyring(undefined, undefined) === null,
    "unset is not null",
  );
  // The whole-ring fingerprint changes with any key or the active id.
  assert(
    entry.fingerprint !== configured(manifestEntry(ring({ k1, k2 }, "k1"))).fingerprint,
    "active id does not change the fingerprint",
  );
  // Fixed vectors shared with the Rust implementation.
  const rust = configured(manifestEntry(ring({ z: "AB".repeat(32), a: "cd".repeat(32) }, "z")));
  assert(
    rust.keyFingerprints.get("z") ===
      "bdd607ec8e30e34b96f730a8c46895ee598221896fb9165ab7f717b4052308df",
    "Rust vector z",
  );
  assert(
    rust.keyFingerprints.get("a") ===
      "2a8926208ff0f91ed933e8c13774b91384086680177711303a7676ede2a45358",
    "Rust vector a",
  );
  // Rust asserts this for the pepper ring with the same lowercase keys and
  // active id; both rings share the canonical form, so the hash is the same.
  assert(
    rust.fingerprint === "ad719e6cf13e153c2e3607564d89dfd07a63400ed0aa6045f18a78a89f6ebcef",
    "Rust whole-ring vector",
  );
}
