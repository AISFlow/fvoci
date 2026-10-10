import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  byCodePoint,
  checkEntry,
  manifestEntry,
  parseKeyring,
  pyJson,
} from "./encryption-keyring.ts";
import { type Case, corpus, type Outcome } from "./encryption-keys.corpus.ts";
import expectedJson from "./encryption-keys.expected.json";
import { main } from "./encryption-keys.ts";

const expected = expectedJson as Record<string, Partial<Outcome> & { exit: number }>;
const CLI = join(import.meta.dir, "encryption-keys.ts");

function runInProcess(c: Case): Outcome {
  let stdout = "";
  let stderr = "";
  const exit = main(
    c.argv.map((arg) => (arg === "ENTRY_FILE" ? "entry.json" : arg)),
    {
      env: c.env,
      out: (text) => (stdout += text),
      err: (text) => (stderr += text),
      readBytes: () => {
        if (typeof c.entry !== "string")
          throw Object.assign(new Error("missing"), { code: "ENOENT" });
        return new TextEncoder().encode(c.entry);
      },
    },
  );
  return { exit, stdout, stderr };
}

function keyMaterial(c: Case): string[] {
  try {
    const ring = JSON.parse(c.env.ENCRYPTION_KEYS ?? "") as unknown;
    if (typeof ring !== "object" || ring === null) return [];
    return Object.values(ring).filter((v): v is string => typeof v === "string" && v.length >= 64);
  } catch {
    return [];
  }
}

describe("differential corpus recorded from the v1 Python oracle", () => {
  const cases = corpus();

  test("every case has exactly one recorded outcome", () => {
    const names = cases.map((c) => c.name);
    expect(new Set(names).size).toBe(names.length);
    expect(names.sort()).toEqual(Object.keys(expected).sort());
    expect(cases.length).toBeGreaterThan(300);
  });

  for (const c of cases) {
    test(c.name, () => {
      const want = expected[c.name];
      const got = runInProcess(c);
      expect(want).toBeDefined();
      if (c.differs) {
        expect(got.exit).toBe(want?.exit ?? -1);
      } else {
        expect(got).toEqual(want as Outcome);
      }
      for (const key of keyMaterial(c)) {
        expect(got.stdout.toLowerCase()).not.toContain(key.slice(0, 64).toLowerCase());
        expect(got.stderr.toLowerCase()).not.toContain(key.slice(0, 64).toLowerCase());
      }
    });
  }
});

describe("format details", () => {
  const k1 = "11".repeat(32);

  test("manifest and canonical ring keep code-point id order, not integer-first", () => {
    const ring = parseKeyring(JSON.stringify({ "10": k1, "2": k1, "-a": k1 }), "2");
    const entry = manifestEntry(ring);
    expect(entry.configured && [...entry.keyFingerprints.keys()]).toEqual(["-a", "10", "2"]);
    expect(pyJson(entry)).toContain('"keyFingerprints": {"-a": ');
  });

  test("code-point order puts astral characters after the BMP", () => {
    expect(["\u{1f600}", "～", "a"].sort(byCodePoint)).toEqual(["a", "～", "\u{1f600}"]);
  });

  test("pyJson escapes like Python ensure_ascii", () => {
    expect(pyJson({ a: "é\u007f\n" })).toBe('{"a": "\\u00e9\\u007f\\n"}');
  });

  test("a key never reaches a refusal message", () => {
    const entry: unknown = JSON.parse(
      pyJson(manifestEntry(parseKeyring(JSON.stringify({ k1 }), "k1"))),
    );
    const other = "ab".repeat(32);
    const problems = checkEntry(entry, parseKeyring(JSON.stringify({ k1: other }), "k1"));
    expect(problems).toEqual(["ENCRYPTION_KEYS has a different key for id(s): k1"]);
  });
});

describe("process contract", () => {
  const run = (args: string[], env: Record<string, string> = {}) => {
    const proc = Bun.spawnSync([process.execPath, CLI, ...args], {
      env: { PATH: process.env.PATH ?? "", ...env },
    });
    return { exit: proc.exitCode, stdout: proc.stdout.toString(), stderr: proc.stderr.toString() };
  };

  test("self-test prints the v1 line and exits 0", () => {
    expect(run(["self-test"])).toEqual({
      exit: 0,
      stdout: "encryption_keys self-test ok\n",
      stderr: "",
    });
  });

  test("usage exits 2 on stderr only", () => {
    const got = run([]);
    expect(got.exit).toBe(2);
    expect(got.stdout).toBe("");
    expect(got.stderr).toContain("check ENTRY_FILE");
  });

  test("check reads the entry file and exits 1 on refusal, 0 on a superset", () => {
    const dir = mkdtempSync(join(tmpdir(), "encryption-keys-"));
    try {
      const ring = { k1: "11".repeat(32) };
      const env = { ENCRYPTION_KEYS: JSON.stringify(ring), ENCRYPTION_ACTIVE_KEY_ID: "k1" };
      const manifest = run(["manifest"], env);
      expect(manifest.exit).toBe(0);
      const file = join(dir, "entry.json");
      writeFileSync(file, manifest.stdout);
      const superset = { ...ring, k2: "22".repeat(32) };
      expect(
        run(["check", file], {
          ENCRYPTION_KEYS: JSON.stringify(superset),
          ENCRYPTION_ACTIVE_KEY_ID: "k2",
        }),
      ).toEqual({ exit: 0, stdout: "", stderr: "" });
      expect(
        run(["check", file], {
          ENCRYPTION_KEYS: JSON.stringify({ k2: superset.k2 }),
          ENCRYPTION_ACTIVE_KEY_ID: "k2",
        }),
      ).toEqual({
        exit: 1,
        stdout: "",
        stderr: "ENCRYPTION_KEYS lacks backed-up key id(s): k1\n",
      });
      expect(run(["check", join(dir, "absent.json")], env).exit).toBe(1);
      const latin1 = join(dir, "latin1.json");
      writeFileSync(latin1, Buffer.from('{"configured": false, "note": "\xe9"}', "latin1"));
      expect(run(["check", latin1], env)).toEqual({
        exit: 1,
        stdout: "",
        stderr: `${latin1} is not valid UTF-8\n`,
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
