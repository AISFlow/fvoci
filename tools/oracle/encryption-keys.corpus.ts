// Differential corpus for the encryption-keys oracle: fixed edge cases plus
// seeded generated keyrings. encryption-keys.expected.json holds what the v1
// Python oracle printed for each case; for a `differs` case (a documented
// contract difference) only the exit code is recorded.
import { manifestEntry, parseKeyring, pyJson } from "./encryption-keyring.ts";

export interface Case {
  name: string;
  argv: string[];
  env: { ENCRYPTION_KEYS?: string; ENCRYPTION_ACTIVE_KEY_ID?: string };
  /** Content of ENTRY_FILE for `check`; null means the file does not exist. */
  entry?: string | null;
  differs?: string;
}

export interface Outcome {
  exit: number;
  stdout: string;
  stderr: string;
}

const HEX = "0123456789abcdef";
const ID_CHARS = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";

function prng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const k = (byte: string) => byte.repeat(32);
const env = (keys: string, active: string) => ({
  ENCRYPTION_KEYS: keys,
  ENCRYPTION_ACTIVE_KEY_ID: active,
});
const ringJson = (ring: Record<string, string>) => JSON.stringify(ring);

function entryText(ring: Record<string, string>, active: string): string {
  return pyJson(manifestEntry(parseKeyring(ringJson(ring), active)));
}

function manifestCases(): Case[] {
  const m = (name: string, e: Case["env"], differs?: string): Case => ({
    name: `manifest/${name}`,
    argv: ["manifest"],
    env: e,
    ...(differs ? { differs } : {}),
  });
  const k1 = k("11");
  const many = Object.fromEntries(Array.from({ length: 33 }, (_, i) => [`id${String(i)}`, k1]));
  const thirtyTwo = Object.fromEntries(Object.entries(many).slice(0, 32));
  return [
    m("unset", {}),
    m("empty both", env("", "")),
    m("whitespace both", env(" \t\n", "\n ")),
    m("keys only", { ENCRYPTION_KEYS: ringJson({ k1 }) }),
    m("active only", { ENCRYPTION_ACTIVE_KEY_ID: "k1" }),
    m("one key", env(ringJson({ k1 }), "k1")),
    m("padded values", env(` ${ringJson({ k1 })}\n`, " k1\t")),
    m("python-only whitespace strip", env(ringJson({ k1 }), "\x1ck1\x85")),
    m("bom is not whitespace", env(ringJson({ k1 }), "﻿k1")),
    m("ideographic space strip", env(ringJson({ k1 }), "　k1")),
    m("upper hex", env(ringJson({ k1: k("AB") }), "k1")),
    m(
      "integer-like ids keep text order",
      env(ringJson({ "10": k1, "2": k("22"), "-a": k1, _b: k1, Z: k1 }), "2"),
    ),
    m("proto id", env('{"__proto__":"' + k1 + '"}', "__proto__")),
    m("duplicate id last wins", env(`{"k1":"${k("22")}","k1":"${k1}"}`, "k1")),
    m("id with final newline", env(ringJson({ "k1\n": k1, k2: k1 }), "k2")),
    m("hex with final newline", env(ringJson({ k1: `${k1}\n` }), "k1")),
    m("32 keys", env(ringJson(thirtyTwo), "id0")),
    m("33 keys", env(ringJson(many), "id0")),
    m("32-char id", env(ringJson({ ["a".repeat(32)]: k1 }), "a".repeat(32))),
    m("33-char id", env(ringJson({ ["a".repeat(33)]: k1 }), "k1")),
    m("33-char active", env(ringJson({ k1 }), "a".repeat(33))),
    m("bad active", env(ringJson({ k1 }), "bad id")),
    m("active missing", env(ringJson({ k1 }), "k2")),
    m("empty object", env("{}", "k1")),
    m("array", env(`["${k1}"]`, "k1")),
    m("string json", env('"k1"', "k1")),
    m("not json", env("not json", "k1")),
    m("short hex", env(ringJson({ k1: "zz" }), "k1")),
    m("65 hex", env(ringJson({ k1: `${k1}1` }), "k1")),
    m("non-string value", env(`{"k1":1}`, "k1")),
    m("null value", env(`{"k1":null}`, "k1")),
    m("bad id", env(ringJson({ "bad id": k1 }), "bad id")),
    m("unicode id", env(ringJson({ é: k1, k1 }), "k1")),
    m("bad id before bad hex", env(ringJson({ "bad id": k1, k1: "zz" }), "k1")),
    m("bad hex before bad id", env(ringJson({ k1: "zz", "bad id": k1 }), "k1")),
    m(
      "integer id bad hex after bad id",
      env(`{"bad id":"${k1}","7":"zz"}`, "7"),
      "JSON.parse lists integer-like ids first, so the first reported problem can differ (both exit 1)",
    ),
    m(
      "NaN json",
      env("NaN", "k1"),
      "Python json accepts NaN/Infinity; Bun and Rust refuse it as invalid json (both exit 1)",
    ),
  ];
}

function checkCases(): Case[] {
  const k1 = k("11");
  const k2 = k("22");
  const k3 = k("33");
  const backed = entryText({ k1, k2 }, "k2");
  const c = (name: string, entry: string | null, e: Case["env"], differs?: string): Case => ({
    name: `check/${name}`,
    argv: ["check", "ENTRY_FILE"],
    env: e,
    entry,
    ...(differs ? { differs } : {}),
  });
  const set = env(ringJson({ k1, k2 }), "k2");
  const fp = (value: string) => `{"configured": true, "keyFingerprints": {"k1": ${value}}}`;
  return [
    c("same ring", backed, set),
    c("same ring other order and case", backed, env(ringJson({ k2: k2.toUpperCase(), k1 }), "k2")),
    c("superset other active", backed, env(ringJson({ k1, k2, k3 }), "k3")),
    c("missing id", backed, env(ringJson({ k2, k3 }), "k3")),
    c("changed key", backed, env(ringJson({ k1: k3, k2 }), "k2")),
    c("missing and changed", backed, env(ringJson({ k2: k3 }), "k2")),
    c("renamed id", backed, env(ringJson({ x1: k1, k2 }), "k2")),
    c("unset env", backed, {}),
    c("bad env", backed, env(ringJson({ k1 }), "")),
    c("not configured, unset env", '{"configured": false}', {}),
    c("not configured, set env", '{"configured": false, "note": "x"}', set),
    c("configured missing", '{"keyFingerprints": {}}', set),
    c("configured not bool", '{"configured": 1, "keyFingerprints": {"k1": "x"}}', set),
    c("not an object", '"x"', set),
    c("array", "[]", set),
    c("fingerprints missing", '{"configured": true}', set),
    c("fingerprints empty", '{"configured": true, "keyFingerprints": {}}', set),
    c("fingerprints array", '{"configured": true, "keyFingerprints": ["k1"]}', set),
    c("fingerprint number", fp("1"), set),
    c("fingerprint null", fp("null"), set),
    c("fingerprint uppercase", fp(JSON.stringify("A".repeat(64))), set),
    c(
      "id order by code point",
      '{"configured": true, "keyFingerprints": {"\\uff5e": "x", "\\ud83d\\ude00": "x", "10": "x", "2": "x", "-a": "x"}}',
      set,
    ),
    c(
      "unset env lists ids",
      '{"configured": true, "keyFingerprints": {"b": "x", "a": "x", "10": "x", "9": "x"}}',
      {},
    ),
    c(
      "file missing",
      null,
      set,
      "Python raised FileNotFoundError (traceback, exit 1); Bun prints one line, exit 1",
    ),
    c(
      "file not json",
      "{",
      set,
      "Python raised JSONDecodeError (traceback, exit 1); Bun prints one line, exit 1",
    ),
    c(
      "file not json and bad env",
      "{",
      env("not json", "k1"),
      "Python raised JSONDecodeError (traceback, exit 1); Bun prints one line, exit 1",
    ),
    c(
      "lone surrogate id with unset env",
      '{"configured": true, "keyFingerprints": {"\\ud800": "x"}}',
      {},
      "Python stderr writes the id as a \\ud800 escape, Bun as U+FFFD (both exit 1)",
    ),
    c(
      "non-ascii fingerprint",
      fp('"\\u00e9"'),
      set,
      "Python hmac.compare_digest raised TypeError (traceback, exit 1); Bun reports the id as changed, exit 1",
    ),
  ];
}

function randomRing(next: () => number, size: number): Record<string, string> {
  const ring: Record<string, string> = {};
  while (Object.keys(ring).length < size) {
    const length = 1 + Math.floor(next() ** 2 * 32);
    let id = "";
    for (let i = 0; i < length; i++) id += ID_CHARS.charAt(Math.floor(next() * ID_CHARS.length));
    let hex = "";
    for (let i = 0; i < 64; i++) {
      const digit = HEX.charAt(Math.floor(next() * 16));
      hex += next() < 0.3 ? digit.toUpperCase() : digit;
    }
    ring[id] = hex;
  }
  return ring;
}

function shuffled<T>(items: T[], next: () => number): T[] {
  const out = [...items];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(next() * (i + 1));
    [out[i], out[j]] = [out[j] as T, out[i] as T];
  }
  return out;
}

function generatedCases(count: number, seed: number): Case[] {
  const next = prng(seed);
  const cases: Case[] = [];
  for (let n = 0; n < count; n++) {
    const size = n % 10 === 9 ? 32 : 1 + Math.floor(next() * 6);
    const ring = randomRing(next, size);
    const ids = Object.keys(ring);
    const active = ids[Math.floor(next() * ids.length)] ?? "";
    const name = (what: string) => `generated/${String(n)}/${what}`;
    cases.push({ name: name("manifest"), argv: ["manifest"], env: env(ringJson(ring), active) });
    const entry = entryText(ring, active);
    const check = (what: string, restore: Record<string, string>, restoreActive: string): Case => ({
      name: name(what),
      argv: ["check", "ENTRY_FILE"],
      env: env(ringJson(restore), restoreActive),
      entry,
    });
    // Same keys, other order and hex case.
    const same = Object.fromEntries(
      shuffled(Object.entries(ring), next).map(([id, hex]) => [
        id,
        next() < 0.5 ? hex.toUpperCase() : hex.toLowerCase(),
      ]),
    );
    cases.push(check("same", same, active));
    // Superset with a new active key.
    const extra = randomRing(next, 1);
    const extraId = Object.keys(extra)[0] ?? "";
    if (!(extraId in ring) && size < 32) {
      cases.push(check("superset", { ...ring, ...extra }, extraId));
    }
    const victim = ids[Math.floor(next() * ids.length)] ?? "";
    const others = Object.fromEntries(Object.entries(ring).filter(([id]) => id !== victim));
    if (ids.length > 1) {
      const keep = Object.keys(others)[0] ?? "";
      cases.push(check("missing", others, keep));
    }
    const flip = (hex: string) => (hex[0] === "0" ? "1" : "0") + hex.slice(1);
    cases.push(check("changed", { ...ring, [victim]: flip(ring[victim] ?? "") }, active));
    const renamed = `${victim.slice(0, 31)}${victim.endsWith("x") ? "y" : "x"}`;
    if (!(renamed in ring)) {
      cases.push(
        check(
          "renamed",
          { ...others, [renamed]: ring[victim] ?? "" },
          active === victim ? renamed : active,
        ),
      );
    }
    cases.push({ name: name("unset env"), argv: ["check", "ENTRY_FILE"], env: {}, entry });
  }
  return cases;
}

function usageCases(): Case[] {
  const why = "usage text is reworded for the Bun entry point; no caller parses it (both exit 2)";
  return [
    [],
    ["help"],
    ["manifest", "extra"],
    ["check"],
    ["check", "a", "b"],
    ["self-test", "x"],
    ["--self-test"],
  ].map((argv) => ({ name: `usage/${argv.join(" ") || "none"}`, argv, env: {}, differs: why }));
}

export function corpus(): Case[] {
  return [...usageCases(), ...manifestCases(), ...checkCases(), ...generatedCases(40, 0x5eed_e1)];
}
