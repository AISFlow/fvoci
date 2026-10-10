import { describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  CheckFailed,
  checks,
  field,
  hasItem,
  objectVersions,
  oracle,
  phaseCommand,
  phaseCommands,
  truthy,
  zoteroKeyring,
} from "./smoke.ts";

const j = (value: unknown) => JSON.stringify(value);
const run =
  (name: string, ...args: string[]) =>
  () => {
    const check = checks[name];
    if (!check) throw new Error(`no check ${name}`);
    check(...args);
  };

describe("install smoke checks", () => {
  test("python truthiness", () => {
    for (const value of [null, undefined, false, 0, "", [], {}]) expect(truthy(value)).toBe(false);
    for (const value of [true, 1, "x", [0], { a: 0 }]) expect(truthy(value)).toBe(true);
  });

  test("field reads a path and refuses a missing or null value", () => {
    expect(field(j({ items: [{ id: "a" }] }), ["items", "0", "id"])).toBe("a");
    expect(field(j({ version: 3 }), ["version"])).toBe("3");
    expect(() => field(j({}), ["id"])).toThrow(CheckFailed);
    expect(() => field(j({ id: null }), ["id"])).toThrow(CheckFailed);
  });

  test("user-id, doctor and body equality", () => {
    expect(run("user-id", j({ userId: "u" }))).not.toThrow();
    expect(run("user-id", j({ userId: "" }))).toThrow(CheckFailed);
    expect(
      run("doctor-convert", j({ ok: true, checks: [{ name: "document_convert", ok: false }] })),
    ).toThrow(CheckFailed);
    expect(
      run("doctor-convert", j({ ok: true, checks: [{ name: "document_convert", ok: true }] })),
    ).not.toThrow();
    expect(
      run("same-body", j({ contentJson: { a: 1, b: [2] } }), j({ contentJson: { b: [2], a: 1 } })),
    ).not.toThrow();
    expect(run("same-body", j({ contentJson: { a: 1 } }), j({ contentJson: { a: 2 } }))).toThrow(
      CheckFailed,
    );
  });

  test("meili settings are exact", () => {
    const good = {
      searchableAttributes: [
        "title",
        "body",
        "chosung",
        "stem",
        "bibliographyBody",
        "bibliographyChosung",
        "bibliographyStem",
      ],
      displayedAttributes: [
        "id",
        "kind",
        "workspaceId",
        "projectId",
        "documentId",
        "taskId",
        "commentId",
        "attachmentId",
        "chunkNo",
        "updatedAt",
      ],
      filterableAttributes: ["resourceKey"],
    };
    expect(run("meili-settings", j(good))).not.toThrow();
    expect(
      run(
        "meili-settings",
        j({ ...good, displayedAttributes: [...good.displayedAttributes, "body"] }),
      ),
    ).toThrow(CheckFailed);
    expect(run("meili-settings", j({ ...good, filterableAttributes: [] }))).toThrow(CheckFailed);
  });

  test("body-extends needs a change that keeps every earlier node", () => {
    const before = j({ contentJson: { content: [{ p: 1 }] } });
    expect(
      run("body-extends", before, j({ contentJson: { content: [{ p: 1 }, { p: 2 }] } })),
    ).not.toThrow();
    expect(run("body-extends", before, before)).toThrow(CheckFailed);
    expect(run("body-extends", before, j({ contentJson: { content: [{ p: 2 }] } }))).toThrow(
      CheckFailed,
    );
  });

  test("reports: secrets, storage, MFA", () => {
    expect(run("secrets-verified", `log\n${j({ secretsVerified: true })}\n`)).not.toThrow();
    expect(run("secrets-verified", j({ secretsVerified: "true" }))).toThrow(CheckFailed);
    const report = {
      checked: 2,
      missing: ["b", "a"],
      sizeMismatch: [],
      previewMissing: [],
      previewSizeMismatch: [],
    };
    expect(run("storage-report", `x\n${j(report)}`, "2", "a,b", "")).not.toThrow();
    expect(run("storage-report", j(report), "2", "a", "")).toThrow(CheckFailed);
    expect(run("storage-report", `${j(report)}\n${j(report)}`, "2", "a,b", "")).toThrow(
      CheckFailed,
    );
    expect(run("storage-report", j({ ...report, previewMissing: ["p"] }), "2", "a,b", "")).toThrow(
      CheckFailed,
    );
    expect(
      run("mfa-invalid", j({ userMfa: { checked: 1, invalid: ["x"], keyUnavailable: [] } })),
    ).not.toThrow();
    expect(
      run("mfa-invalid", j({ userMfa: { checked: 1, invalid: ["x"], keyUnavailable: ["k"] } })),
    ).toThrow(CheckFailed);
  });

  test("counts and the backup manifest", () => {
    expect(
      run(
        "native-counts",
        "document_states=1;document_collab_updates=0;task_states=0;task_collab_updates=2",
      ),
    ).not.toThrow();
    expect(
      run(
        "native-counts",
        "document_states=0;document_collab_updates=0;task_states=1;task_collab_updates=0",
      ),
    ).toThrow(CheckFailed);
    const dir = mkdtempSync(join(tmpdir(), "smoke-manifest-"));
    try {
      for (const name of ["database.dump", "storage.tar"]) writeFileSync(join(dir, name), "");
      const manifest = {
        search: { included: false },
        encryptionKeys: { configured: true, keyFingerprints: { k1: "f" } },
      };
      writeFileSync(join(dir, "manifest.json"), j(manifest));
      process.env["ENC_ID"] = "k1";
      process.env["ENC_K1"] = "a".repeat(64);
      expect(run("backup-manifest", join(dir, "manifest.json"))).not.toThrow();
      writeFileSync(join(dir, "manifest.json"), j({ ...manifest, leaked: "a".repeat(64) }));
      expect(run("backup-manifest", join(dir, "manifest.json"))).toThrow(CheckFailed);
      writeFileSync(join(dir, "extra"), "");
      writeFileSync(join(dir, "manifest.json"), j(manifest));
      expect(run("backup-manifest", join(dir, "manifest.json"))).toThrow(CheckFailed);
    } finally {
      rmSync(dir, { recursive: true });
    }
  });

  test("search membership, versions, keyring", () => {
    expect(hasItem(j({ items: [{ id: "t" }] }), "t")).toBe(true);
    expect(hasItem(j({ items: [] }), "t")).toBe(false);
    const rows = [
      { status: "success", versionId: "v1", versionOrdinal: 2, size: 5, etag: "e" },
      { status: "success", versionId: "v2", versionOrdinal: 1, isDeleteMarker: true },
    ];
    expect(objectVersions(rows.map(j).join("\n"))).toBe("v1 5 false true e\nv2 0 true false -");
    expect(() => objectVersions([rows[0], rows[0]].map(j).join("\n"))).toThrow(CheckFailed);
    const fixture = `const PEPPER: &str =\n r#"{"zf":"${"0".repeat(64)}"}"#;`;
    const upstream = `pub const KEY: &str = "SYNTHETIC_ONLY_KEY";`;
    expect(zoteroKeyring(fixture, upstream)).toBe(`zf ${"0".repeat(64)} SYNTHETIC_ONLY_KEY`);
    expect(() => zoteroKeyring(fixture, `pub const KEY: &str = "REAL_KEY";`)).toThrow(CheckFailed);
  });

  test("oracle is canonical and drops serverNow", () => {
    const args = Array.from({ length: 26 }, () => j({ items: [] }));
    args[2] = "404";
    args[16] = "403";
    args[17] = j({ items: [], serverNow: "t1" });
    const a = oracle(args);
    args[17] = j({ serverNow: "t2", items: [] });
    expect(oracle(args)).toBe(a);
    expect(() => oracle(args.slice(1))).toThrow(CheckFailed);
  });
});

describe("phases and the first error", () => {
  test("the CLI dispatches every phase command the smokes call", () => {
    for (const name of [
      "init",
      "phase",
      "fail",
      "error",
      "report",
      "finish",
      "group",
      "endgroup",
      "quote",
    ]) {
      expect(phaseCommands.has(name)).toBe(true);
    }
  });

  const run = (command: string, ...args: string[]) =>
    (phaseCommand(command, args, () => Promise.resolve("")) as { out: string }).out;
  const withActions = (value: string | undefined, body: (dir: string) => void) => {
    const dir = mkdtempSync(join(tmpdir(), "smoke-phase-"));
    const previous = process.env["GITHUB_ACTIONS"];
    if (value === undefined) delete process.env["GITHUB_ACTIONS"];
    else process.env["GITHUB_ACTIONS"] = value;
    try {
      body(dir);
    } finally {
      if (previous === undefined) delete process.env["GITHUB_ACTIONS"];
      else process.env["GITHUB_ACTIONS"] = previous;
      rmSync(dir, { recursive: true });
    }
  };

  test("a fail inside a phase is the reported first error, also in Actions", () => {
    withActions("true", (dir) => {
      const state = join(dir, "state");
      const assertLog = join(dir, "assert.log");
      run("init", state, "unit", assertLog);
      expect(run("phase", state, "one")).toBe("::group::phase one\n");
      expect(run("phase", state, "two")).toBe("::endgroup::\n::group::phase two\n");
      run("error", state, "1", "12", "curl", "-f");
      run("fail", state, "body differs");
      expect(readFileSync(assertLog, "utf8")).toBe("FAIL: body differs\n");
      expect(run("report", state, "1")).toBe(
        "::endgroup::\n::error title=unit::phase two failed (exit 1): body differs\n",
      );
      expect(run("finish", state, "1")).toBe("");
      expect(existsSync(state)).toBe(false);
    });
  });

  test("the failing command, an interrupt and a teardown-only failure", () => {
    withActions(undefined, (dir) => {
      const state = join(dir, "state");
      const report = () => (JSON.parse(readFileSync(state, "utf8")) as { report: string }).report;
      run("init", state, "unit");
      expect(run("phase", state, "test")).toBe("== phase test\n");
      run("error", state, "7", "40", "curl", "-fsS", "x");
      run("report", state, "7");
      expect(report()).toBe("phase test failed (exit 7): line 40: curl -fsS x (exit 7)");
      run("report", state, "143");
      expect(report()).toBe("phase test failed (exit 143): interrupted (SIGTERM)");
      run("report", state, "0");
      expect(report()).toBe("");
      run("finish", state, "1");
      expect(existsSync(state)).toBe(false);
    });
  });
});

describe("has-item CLI", () => {
  const cli = (body: string): number | null =>
    Bun.spawnSync([process.execPath, join(import.meta.dir, "smoke.ts"), "has-item", body, "t"], {
      stdout: "ignore",
      stderr: "ignore",
    }).exitCode;

  test("only a valid search listing proves absence", () => {
    expect(cli(j({ items: [{ id: "t" }], next_cursor: null }))).toBe(0);
    expect(cli(j({ items: [{ id: "u" }], next_cursor: null }))).toBe(1);
    expect(cli(j({ items: [], next_cursor: null }))).toBe(1);
  });

  test("a malformed search reply is unreadable, never absent", () => {
    for (const body of [
      "{",
      "{}",
      "null",
      "[]",
      '"items"',
      j({ items: null }),
      j({ items: "bad" }),
      j({ items: {} }),
      j({ items: [null] }),
      j({ items: ["t"] }),
      j({ items: [{}] }),
      j({ items: [{ id: 7 }] }),
      j({ items: [{ id: "u" }, { id: null }] }),
    ])
      expect({ body, status: cli(body) }).toEqual({ body, status: 2 });
  });
});
