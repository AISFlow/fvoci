import { afterAll, describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { GATES, verdict } from "./check-ci.ts";

const ROOT = resolve(import.meta.dir, "../..");
const SHA = "a".repeat(40);
const T1 = "2026-10-10T01:00:00Z";
const T2 = "2026-10-10T02:00:00Z";
const row = (name: string, status: string, conclusion: string, at: string) =>
  [name, status, conclusion, at].join("\t");
const allGreen = GATES.map((gate) => row(gate, "completed", "success", T1));

describe("release gate verdict", () => {
  test("gates are the five workflow gates in registry order", () => {
    expect(GATES).toEqual([
      "web-ci-gate",
      "rust-ci-gate",
      "documents-ci-gate",
      "collab-engine-ci-gate",
      "install-ci-gate",
    ]);
  });

  test("every gate green, other check runs ignored", () => {
    const runs = [row("lint", "completed", "failure", T2), ...allGreen].join("\n") + "\n";
    expect(verdict(GATES, runs)).toEqual({
      lines: GATES.map((gate) => `${gate}: completed success ${T1}`),
      bad: [],
    });
  });

  test("a missing, failed or unfinished gate is not green", () => {
    const runs = [
      row("web-ci-gate", "completed", "failure", T1),
      row("rust-ci-gate", "in_progress", "none", T1),
      row("documents-ci-gate", "completed", "success", T1),
      row("collab-engine-ci-gate", "completed", "skipped", T1),
    ].join("\n");
    const result = verdict(GATES, runs);
    expect(result.bad).toEqual([
      "web-ci-gate",
      "rust-ci-gate",
      "collab-engine-ci-gate",
      "install-ci-gate",
    ]);
    expect(result.lines.at(-1)).toBe("install-ci-gate: missing none ");
    expect(verdict(GATES, "").bad).toEqual([...GATES]);
  });

  test("the latest run decides, in either order", () => {
    const rerun = [row("web-ci-gate", "completed", "failure", T1), ...allGreen.slice(1)];
    const fixed = row("web-ci-gate", "completed", "success", T2);
    expect(verdict(GATES, [...rerun, fixed].join("\n")).bad).toEqual([]);
    expect(verdict(GATES, [fixed, ...rerun].join("\n")).bad).toEqual([]);
    const broken = row("web-ci-gate", "completed", "failure", T2);
    expect(verdict(GATES, [broken, ...allGreen].join("\n")).bad).toEqual(["web-ci-gate"]);
  });

  test("a run that has not started is newer than any finished run", () => {
    const queued = row("web-ci-gate", "queued", "none", "");
    expect(verdict(GATES, [...allGreen, queued].join("\n")).bad).toEqual(["web-ci-gate"]);
    expect(verdict(GATES, [queued, ...allGreen].join("\n")).bad).toEqual(["web-ci-gate"]);
  });

  test("a tie between green and not green is not green", () => {
    const tie = row("web-ci-gate", "completed", "failure", T1);
    expect(verdict(GATES, [...allGreen, tie].join("\n")).bad).toEqual(["web-ci-gate"]);
    expect(verdict(GATES, [tie, ...allGreen].join("\n")).bad).toEqual(["web-ci-gate"]);
  });

  test("a line that is not four fields is refused", () => {
    for (const bad of ["web-ci-gate\tcompleted\tsuccess", `${allGreen[0] ?? ""}\textra`, ""]) {
      expect(() => verdict(GATES, [...allGreen, bad, ...allGreen].join("\n"))).toThrow(
        "is not 4 tab-separated fields",
      );
    }
  });
});

describe("scripts/release-check-ci.sh", () => {
  const scratch = mkdtempSync(join(tmpdir(), "fvoci-check-ci-"));
  afterAll(() => {
    rmSync(scratch, { recursive: true, force: true });
  });
  const bin = join(scratch, "bin");
  // A gh stand-in that prints the TSV the real --jq filter would produce.
  mkdirSync(bin);
  writeFileSync(
    join(bin, "gh"),
    `#!/usr/bin/env bash\nprintf '%s\\n' "$*" >"${scratch}/gh.args"\ncat "${scratch}/runs.tsv"\nexit "\${FAKE_GH_EXIT:-0}"\n`,
  );
  chmodSync(join(bin, "gh"), 0o755);

  const run = (runs: string, args = [SHA], env: Record<string, string> = {}) => {
    writeFileSync(join(scratch, "runs.tsv"), runs);
    const result = Bun.spawnSync(["bash", join(ROOT, "scripts/release-check-ci.sh"), ...args], {
      env: {
        PATH: `${bin}:${dirname(process.execPath)}:/usr/bin:/bin`,
        GITHUB_REPOSITORY: "AISFlow/fvoci",
        ...env,
      },
    });
    return {
      code: result.exitCode,
      stdout: result.stdout.toString(),
      stderr: result.stderr.toString(),
    };
  };

  test("green commit passes and asks for that commit's check runs", () => {
    const result = run(allGreen.join("\n") + "\n");
    expect(result).toEqual({
      code: 0,
      stdout: GATES.map((gate) => `${gate}: completed success ${T1}\n`).join(""),
      stderr: "",
    });
    expect(readFileSync(join(scratch, "gh.args"), "utf8")).toStartWith(
      `api --paginate repos/AISFlow/fvoci/commits/${SHA}/check-runs?per_page=100 --jq `,
    );
  });

  test("a gate that is not green fails with its name", () => {
    const result = run(allGreen.slice(1).join("\n") + "\n");
    expect(result.code).toBe(1);
    expect(result.stderr).toBe("release-check-ci: release commit is not green on: web-ci-gate\n");
  });

  test("gh failure, short SHA and missing repository stop before any verdict", () => {
    const failed = run(allGreen.join("\n"), [SHA], { FAKE_GH_EXIT: "1" });
    expect([failed.code, failed.stdout]).toEqual([1, ""]);
    expect(run("", ["abc"]).code).toBe(2);
    const noRepo = run("", [SHA], { GITHUB_REPOSITORY: "" });
    expect([noRepo.code, noRepo.stdout]).toEqual([1, ""]);
  });
});
