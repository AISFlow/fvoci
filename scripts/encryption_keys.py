#!/usr/bin/env python3
"""Independent v1 ENCRYPTION_KEYS manifest compatibility oracle.

Operational backup and restore use the shared Rust fvoci-migrate modes. Keep
this Python fixture for differential vectors and negative checks.

The keyring seals TOTP secrets, workspace SSO client secrets and webhook
signing secrets in the database (`enc:v2:<kid>:...`). A restore needs every
key id those ciphertexts name. The backup manifest therefore records, per key
id, HMAC-SHA256(key, label || id): it identifies the key without revealing it
and lets restore accept a superset keyring (rotation added keys) while
refusing one that lacks or changed a backed-up key id. `fingerprint` is the
SHA-256 of the canonical keyring and active id, computed like the pepper
fingerprint. Keys never leave the env file and are never printed.

  encryption_keys.py manifest            print the manifest entry (JSON)
  encryption_keys.py check ENTRY_FILE    exit 1 if the env keyring cannot open
                                         what the backed-up keyring sealed
  encryption_keys.py self-test

The keyring is read from the ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID
environment variables (both or neither, as the server requires).
"""

import hashlib
import hmac
import json
import os
import re
import sys

LABEL = b"fvoci:encryption-key-fingerprint:v1:"
KEY_ID_RE = re.compile(r"^[a-zA-Z0-9_-]{1,32}$")
HEX_RE = re.compile(r"^[0-9a-fA-F]{64}$")


class KeyringError(Exception):
    pass


def parse_keyring(raw, active):
    """Mirror of Keyring::parse_named. None when neither variable is set."""
    raw = (raw or "").strip()
    active = (active or "").strip()
    if not raw and not active:
        return None
    if not raw or not active:
        raise KeyringError("ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID must be set together")
    if not KEY_ID_RE.match(active):
        raise KeyringError("invalid ENCRYPTION_ACTIVE_KEY_ID")
    try:
        ring = json.loads(raw)
    except ValueError:
        raise KeyringError("invalid ENCRYPTION_KEYS json") from None
    if not isinstance(ring, dict) or not 1 <= len(ring) <= 32:
        raise KeyringError("ENCRYPTION_KEYS must have 1-32 keys")
    keys = {}
    for key_id, value in ring.items():
        if not KEY_ID_RE.match(key_id):
            raise KeyringError("invalid ENCRYPTION_KEYS key id")
        if not isinstance(value, str) or not HEX_RE.match(value):
            raise KeyringError("ENCRYPTION_KEYS keys must be 64-char hex")
        keys[key_id] = bytes.fromhex(value)
    if active not in keys:
        raise KeyringError("ENCRYPTION_ACTIVE_KEY_ID is not in ENCRYPTION_KEYS")
    return keys, active


def key_fingerprint(key_id, key):
    return hmac.new(key, LABEL + key_id.encode(), hashlib.sha256).hexdigest()


def manifest_entry(keyring):
    if keyring is None:
        return {
            "configured": False,
            "note": "ENCRYPTION_KEYS was not set; nothing could be sealed with it. Restore runs fvoci-migrate --verify-secrets regardless.",
        }
    keys, active = keyring
    canon = json.dumps(
        {"keys": {k: keys[k].hex() for k in sorted(keys)}, "active": active},
        separators=(",", ":"),
    )
    return {
        "configured": True,
        "fingerprint": hashlib.sha256(canon.encode()).hexdigest(),
        "activeKeyId": active,
        "keyFingerprints": {k: key_fingerprint(k, keys[k]) for k in sorted(keys)},
        "note": "Per key id HMAC-SHA256(key, label||id); keys are not stored. Restore needs every key id listed here with the same key (extra keys are fine) and then opens every sealed secret (fvoci-migrate --verify-secrets).",
    }


def check(entry, keyring):
    """Returns a list of problems (key ids only, never key material)."""
    if not isinstance(entry, dict) or not isinstance(entry.get("configured"), bool):
        return ["backup manifest encryptionKeys entry is malformed"]
    if not entry["configured"]:
        return []
    expected = entry.get("keyFingerprints")
    if not isinstance(expected, dict) or not expected:
        return ["backup manifest encryptionKeys.keyFingerprints is missing"]
    if keyring is None:
        return [
            "the backed-up install had ENCRYPTION_KEYS (key ids: %s); set ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID"
            % ", ".join(sorted(expected))
        ]
    keys, _active = keyring
    missing = sorted(k for k in expected if k not in keys)
    changed = sorted(
        k
        for k in expected
        if k in keys
        and not hmac.compare_digest(str(expected[k]), key_fingerprint(k, keys[k]))
    )
    problems = []
    if missing:
        problems.append("ENCRYPTION_KEYS lacks backed-up key id(s): %s" % ", ".join(missing))
    if changed:
        problems.append("ENCRYPTION_KEYS has a different key for id(s): %s" % ", ".join(changed))
    return problems


def self_test():
    k1, k2, k3 = "11" * 32, "22" * 32, "33" * 32
    backed = parse_keyring(json.dumps({"k1": k1, "k2": k2}), "k2")
    entry = manifest_entry(backed)
    blob = json.dumps(entry)
    assert k1 not in blob and k2 not in blob, "keys leaked into the manifest"
    assert entry["keyFingerprints"]["k1"] != entry["keyFingerprints"]["k2"]
    # Same keyring, hex case and key order do not matter.
    assert check(entry, parse_keyring(json.dumps({"k2": k2.upper(), "k1": k1}), "k2")) == []
    # Rotation: a superset with another active key is accepted.
    assert check(entry, parse_keyring(json.dumps({"k1": k1, "k2": k2, "k3": k3}), "k3")) == []
    # A missing or changed backed-up key id is refused, naming ids only.
    problems = check(entry, parse_keyring(json.dumps({"k2": k2, "k3": k3}), "k3"))
    assert problems == ["ENCRYPTION_KEYS lacks backed-up key id(s): k1"], problems
    problems = check(entry, parse_keyring(json.dumps({"k1": k3, "k2": k2}), "k2"))
    assert problems == ["ENCRYPTION_KEYS has a different key for id(s): k1"], problems
    assert all(k3 not in p for p in problems)
    # Same key under another id is not the same key id.
    assert check(entry, parse_keyring(json.dumps({"x1": k1, "k2": k2}), "k2")) != []
    # Unset in the restore env while the backup had keys: refused.
    assert check(entry, None) != []
    # Backup without keys: anything is accepted here (the decrypt probe decides).
    none_entry = manifest_entry(None)
    assert none_entry["configured"] is False
    assert check(none_entry, None) == []
    assert check(none_entry, backed) == []
    # Malformed entries and keyrings.
    assert check({"configured": True}, backed) != []
    assert check("x", backed) != []
    for raw, active in [
        ("{}", "k1"),
        (json.dumps({"k1": "zz"}), "k1"),
        (json.dumps({"k1": k1}), "k2"),
        (json.dumps({"bad id": k1}), "bad id"),
        ("not json", "k1"),
        (json.dumps({"k1": k1}), ""),
        ("", "k1"),
    ]:
        try:
            parse_keyring(raw, active)
        except KeyringError as err:
            assert k1 not in str(err)
        else:
            raise AssertionError("accepted %r" % raw)
    assert parse_keyring("", "") is None and parse_keyring(None, None) is None
    # The whole-ring fingerprint changes with any key or the active id.
    assert entry["fingerprint"] != manifest_entry(parse_keyring(json.dumps({"k1": k1, "k2": k2}), "k1"))["fingerprint"]
    print("encryption_keys self-test ok")


def env_keyring():
    return parse_keyring(
        os.environ.get("ENCRYPTION_KEYS"), os.environ.get("ENCRYPTION_ACTIVE_KEY_ID")
    )


def main(argv):
    try:
        if argv == ["manifest"]:
            json.dump(manifest_entry(env_keyring()), sys.stdout)
            sys.stdout.write("\n")
            return 0
        if len(argv) == 2 and argv[0] == "check":
            with open(argv[1], encoding="utf-8") as fh:
                entry = json.load(fh)
            problems = check(entry, env_keyring())
            for problem in problems:
                print(problem, file=sys.stderr)
            return 1 if problems else 0
        if argv == ["self-test"]:
            self_test()
            return 0
    except KeyringError as err:
        print(str(err), file=sys.stderr)
        return 1
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
