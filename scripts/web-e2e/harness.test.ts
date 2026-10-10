import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { evidenceDirectory, timerInvocations } from "./labels.ts";
import { redactServerLog, redactStartupLog } from "./redact.ts";
import { retainFailureArtifacts } from "./run-group.ts";

const titles = [
  ...readFileSync(
    join(import.meta.dir, "../../apps/web/e2e/v050-task-timer.spec.ts"),
    "utf8",
  ).matchAll(/^test\("([^"\n]+)"/gm),
].map((match) => match[1] ?? "");

describe("web e2e harness contracts", () => {
  test("timer split covers every current title once", () => {
    expect(titles).toHaveLength(22);
    const rows = timerInvocations([
      "--workers=1",
      "e2e/v050-task-timer.spec.ts",
      "--retries=0",
      "--trace=on",
    ]);
    expect(rows).toHaveLength(4);
    expect(rows?.map((row) => row[0])).toEqual(["--grep-invert", "--grep", "--grep", "--grep"]);
    const groups =
      rows?.map(
        (row) =>
          new Set(
            titles.filter(
              (title) => new RegExp(row[1] ?? "").test(title) === (row[0] === "--grep"),
            ),
          ),
      ) ?? [];
    expect(groups.map((group) => group.size)).toEqual([9, 7, 1, 5]);
    const union = new Set(groups.flatMap((group) => [...group]));
    expect(union.size).toBe(titles.length);
    expect(groups[2]).toEqual(
      new Set([
        "native same-database restart preserves paused and running anchors for genuine new clients",
      ]),
    );
  });

  test("explicit timer filters and pending mode stay one invocation", () => {
    const cases = [
      ["v050-task-timer.spec.ts", "--grep", "literal owner title"],
      ["--grep=literal owner title", "e2e/v050-task-timer.spec.ts"],
      ["v050-task-timer.spec.ts", "--grep-invert", "literal owner title"],
      ["v050-task-timer.spec.ts", "-g", "literal owner title"],
      ["v050-task-timer.spec.ts", "--shard", "1/2"],
      ["v050-task-timer.spec.ts", "--list"],
      ["v050-task-timer.spec.ts", "--", "literal owner title"],
      ["v050-task-timer.spec.ts", "other-flow.spec.ts"],
      ["other-flow.spec.ts"],
      [],
    ];
    for (const args of cases) expect(timerInvocations(args)).toBeNull();
    expect(timerInvocations(["v050-task-timer.spec.ts"], "1")).toBeNull();
  });

  test("evidence directory defaults per run and keeps an explicit path", () => {
    expect(evidenceDirectory("/tmp/run-first", undefined)).toBe(
      "/tmp/run-first/playwright-output/w5-evidence",
    );
    expect(evidenceDirectory("/tmp/run-empty", "")).toBe(
      "/tmp/run-empty/playwright-output/w5-evidence",
    );
    expect(evidenceDirectory("/tmp/run-third", "/tmp/caller-owned evidence")).toBe(
      "/tmp/caller-owned evidence",
    );
  });

  test("server log redaction removes database and libsql secrets", () => {
    const raw = [
      "probe DATABASE_APP_URL=postgres://app:fixture-secret@127.0.0.1:5432/db admin unset",
      "probe FVOCI_LIBSQL_URL=libsql://libsql-url-secret.example.test FVOCI_LIBSQL_AUTH_TOKEN=libsql-auth-secret FVOCI_TEST_TURSO_DATABASE_URL=https://turso-url-secret.example.test FVOCI_TEST_TURSO_AUTH_TOKEN=turso-auth-secret",
      "bare libsql://libsql-url-secret.example.test",
      "bare https://libsql-url-secret.aws-ap-northeast-1.turso.io",
      "probe ENCRYPTION_KEYS=super-secret",
    ].join("\n");
    const retained = redactServerLog(raw);
    expect(retained).toContain("probe DATABASE_APP_URL=redacted admin unset");
    expect(retained).toContain("FVOCI_LIBSQL_URL=redacted");
    expect(retained).toContain("FVOCI_LIBSQL_AUTH_TOKEN=redacted");
    expect(retained).toContain("FVOCI_TEST_TURSO_DATABASE_URL=redacted");
    expect(retained).toContain("FVOCI_TEST_TURSO_AUTH_TOKEN=redacted");
    expect(retained).toContain("libsql://redacted");
    expect(retained).toContain("https://redacted");
    expect(retained).not.toContain("fixture-secret");
    expect(retained).not.toContain("libsql-url-secret");
    expect(retained).not.toContain("turso-auth-secret");
    const startup = redactStartupLog(`${raw}\nprobe MEILI_MASTER_KEY=master-secret`);
    expect(startup).toContain("MEILI_MASTER_KEY=redacted");
    expect(startup).toContain("ENCRYPTION_KEYS=redacted");
    expect(startup).not.toContain("master-secret");
    expect(startup).not.toContain("super-secret");
  });

  test("failure retention copies proof files and drops the run directory", async () => {
    const directory = mkdtempSync(join(tmpdir(), "fvoci-retain-"));
    const run = join(directory, "run");
    const evidence = join(run, "playwright-output", "w5-evidence");
    require("node:fs").mkdirSync(evidence, { recursive: true });
    writeFileSync(join(evidence, "native-proof.json"), '{"fixture":true}');
    writeFileSync(join(evidence, "zoom200.png"), "fixture screenshot bytes");
    const output = join(directory, "github-output");
    const previous = process.env.GITHUB_OUTPUT;
    process.env.GITHUB_OUTPUT = output;
    const retained = await retainFailureArtifacts({
      runDir: run,
      serverLog: join(run, "missing-server.log"),
      netMonitorLog: join(run, "missing-net.log"),
      netMarksLog: join(run, "missing-marks.log"),
      label: "timer-evidence-fixture",
      tempDir: directory,
    });
    rmSync(run, { recursive: true, force: true });
    expect(readFileSync(output, "utf8")).toContain(`failure-artifacts=${retained}`);
    expect(
      readFileSync(join(retained, "playwright-output/w5-evidence/native-proof.json"), "utf8"),
    ).toBe('{"fixture":true}');
    expect(readFileSync(join(retained, "playwright-output/w5-evidence/zoom200.png"), "utf8")).toBe(
      "fixture screenshot bytes",
    );
    if (previous === undefined) delete process.env.GITHUB_OUTPUT;
    else process.env.GITHUB_OUTPUT = previous;
    rmSync(directory, { recursive: true, force: true });
  });

  test("entrypoint rejects dry-run and shard-count override before any build", async () => {
    const cases: [Record<string, string>, string[], string][] = [
      [{ FVOCI_WEB_E2E_DRY_RUN: "1" }, [], "FVOCI_WEB_E2E_DRY_RUN is not supported"],
      [{}, ["--ci-shard-count", "4"], "--ci-shard-count is not supported"],
      [
        {},
        ["--ci-use-committed-api", "--ci-use-committed-api"],
        "--ci-use-committed-api may be supplied only once",
      ],
    ];
    for (const [env, args, message] of cases) {
      const child = Bun.spawn(["bun", join(import.meta.dir, "run.ts"), ...args], {
        env: { ...process.env, ...env },
        stdout: "pipe",
        stderr: "pipe",
      });
      const stderr = await new Response(child.stderr).text();
      expect(await child.exited).toBe(1);
      expect(stderr).toContain(message);
      expect(stderr).not.toContain("fvoci-web-e2e-fake");
    }
  });
});
