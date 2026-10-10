import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { main, parseArgs, runPlan, type PlanArgs } from "../plan.ts";
import { WORKFLOW_JOBS, WORKFLOWS } from "./registry.ts";
import { cleanEnv, git, PrCheckout, prEvent, tempDir } from "./test-support.ts";

// The ci-plan step as the workflows run it: real git checkouts, the event
// file, GITHUB_EVENT_NAME / GITHUB_SHA, and the files it writes.

const TIMEOUT = 60_000;
const PLAN_TS = resolve(import.meta.dir, "../plan.ts");

type CliRun = {
  code: number;
  stdout: string;
  stderr: string;
  plan: string | null;
  output: string | null;
};

async function cli(
  repo: string,
  workflow: string,
  eventName: string | undefined,
  event: unknown,
  tested: string | undefined,
  extra: string[] = [],
): Promise<CliRun> {
  const dir = tempDir("cli");
  const eventPath = join(dir, "event.json");
  writeFileSync(eventPath, typeof event === "string" ? event : JSON.stringify(event));
  const proc = Bun.spawn(
    [
      process.execPath,
      PLAN_TS,
      "--workflow",
      workflow,
      "--repo-root",
      repo,
      "--event-json",
      eventPath,
      "--output-plan",
      join(dir, "plan.json"),
      "--github-output",
      join(dir, "out.txt"),
      ...extra,
    ],
    {
      cwd: repo,
      env: cleanEnv({ GITHUB_EVENT_NAME: eventName, GITHUB_SHA: tested }),
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  const read = (name: string) =>
    existsSync(join(dir, name)) ? readFileSync(join(dir, name), "utf8") : null;
  return { code, stdout, stderr, plan: read("plan.json"), output: read("out.txt") };
}

function outputs(text: string): Map<string, string> {
  const out = new Map<string, string>();
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] as string;
    if (!line) continue;
    const heredoc = /^([a-z_]+)<<([A-Z_]+)$/.exec(line);
    if (heredoc) {
      const end = lines.indexOf(heredoc[2] as string, i + 1);
      expect(end).toBe(i + 2);
      out.set(heredoc[1] as string, lines[i + 1] as string);
      i = end;
      continue;
    }
    const eq = line.indexOf("=");
    out.set(line.slice(0, eq), line.slice(eq + 1));
  }
  return out;
}

const pgChecks = (output: string) =>
  (
    JSON.parse(outputs(output).get("postgres_matrix") as string) as { include: { check: string }[] }
  ).include.map((r) => r.check);

describe("plan CLI", () => {
  test(
    "a docs-only pull request writes the exact plan, outputs and summary",
    async () => {
      const fx = new PrCheckout();
      const head = fx.branch({ "README.md": "docs only\n" });
      const m = fx.merge(fx.base, head);
      const r = await cli(m.work, "install", "pull_request", prEvent(fx.base, head), m.tested);
      expect(r.stderr).toBe("");
      expect(r.code).toBe(0);
      expect(r.stdout).toBe('{"mode": "narrow", "reason_code": "NARROW_DOCS"}\n');
      expect(r.plan).toBe(
        [
          "{",
          '  "version": 3,',
          '  "workflow": "install",',
          '  "mode": "narrow",',
          '  "reason_code": "NARROW_DOCS",',
          '  "plan_ok": true,',
          `  "base_sha": "${fx.base}",`,
          `  "head_sha": "${head}",`,
          `  "merge_base_sha": "${fx.base}",`,
          `  "tested_sha": "${m.tested}",`,
          '  "path_count": 1,',
          '  "jobs": {',
          '    "install-smoke": {',
          '      "selected": false',
          "    },",
          '    "backup-restore-smoke": {',
          '      "selected": false',
          "    },",
          '    "upgrade-smoke-arm64": {',
          '      "selected": false',
          "    }",
          "  }",
          "}",
          "",
        ].join("\n"),
      );
      expect(r.output).toBe(
        [
          "mode=narrow",
          "reason_code=NARROW_DOCS",
          "plan_ok=true",
          "select_install_smoke=false",
          "select_backup_restore_smoke=false",
          "select_upgrade_smoke_arm64=false",
          "plan_json<<PLAN_EOF",
          `{"base_sha":"${fx.base}","head_sha":"${head}","jobs":{"backup-restore-smoke":{"selected":false},"install-smoke":{"selected":false},"upgrade-smoke-arm64":{"selected":false}},"merge_base_sha":"${fx.base}","mode":"narrow","path_count":1,"plan_ok":true,"reason_code":"NARROW_DOCS","tested_sha":"${m.tested}","version":3,"workflow":"install"}`,
          "PLAN_EOF",
          "",
        ].join("\n"),
      );
    },
    TIMEOUT,
  );

  test(
    "every workflow emits a select output per job; only rust emits the postgres matrix",
    async () => {
      const fx = new PrCheckout();
      const head = fx.branch({ "src/lib.rs": "fn x() {}\n" });
      const m = fx.merge(fx.base, head);
      for (const workflow of WORKFLOWS) {
        const r = await cli(m.work, workflow, "pull_request", prEvent(fx.base, head), m.tested);
        expect(r.code, r.stderr).toBe(0);
        const out = outputs(r.output ?? "");
        const jobs: readonly string[] = WORKFLOW_JOBS[workflow];
        expect([...out.keys()]).toEqual([
          "mode",
          "reason_code",
          "plan_ok",
          ...jobs.map((job) => `select_${job.replaceAll("-", "_")}`),
          "plan_json",
          ...(workflow === "rust" ? ["postgres_matrix"] : []),
        ]);
        expect(JSON.parse(out.get("plan_json") as string)).toEqual(JSON.parse(r.plan ?? ""));
      }
    },
    TIMEOUT,
  );

  test(
    "postgres rows: a pull request runs PG18 x64 shards even when full; other events run all twelve",
    async () => {
      const fx = new PrCheckout();
      const docs = fx.branch({ "README.md": "docs\n" });
      const m = fx.merge(fx.base, docs);
      const pr = await cli(m.work, "rust", "pull_request", prEvent(fx.base, docs), m.tested);
      expect(pgChecks(pr.output ?? "")).toEqual(["postgres", "postgres-c", "postgres-b"]);
      const code = fx.branch({ "src/lib.rs": "fn x() {}\n" });
      const c = fx.merge(fx.base, code);
      const full = await cli(c.work, "rust", "pull_request", prEvent(fx.base, code), c.tested);
      expect(outputs(full.output ?? "").get("select_postgres")).toBe("true");
      expect(pgChecks(full.output ?? "")).toEqual(["postgres", "postgres-c", "postgres-b"]);
      const group = { merge_group: { base_sha: fx.base, head_sha: m.tested } };
      for (const [eventName, event] of [
        ["merge_group", group],
        ["push", { before: fx.base, after: m.tested }],
        ["workflow_dispatch", {}],
        ["schedule", {}],
      ] as const) {
        const r = await cli(m.work, "rust", eventName, event, m.tested);
        expect(r.code, r.stderr).toBe(0);
        const checks = pgChecks(r.output ?? "");
        expect(checks).toHaveLength(12);
        expect(new Set(checks).size).toBe(12);
      }
    },
    TIMEOUT,
  );

  test(
    "workflow_dispatch opt-in outputs",
    async () => {
      const fx = new PrCheckout();
      const work = fx.clone();
      for (const [inputs, selected, ok] of [
        [{ run_upgrade_smoke_arm: "true" }, "true", "true"],
        [{ run_upgrade_smoke_arm: "false" }, "false", "true"],
        [{ run_upgrade_smoke_arm: "maybe" }, "false", "false"],
      ] as const) {
        const r = await cli(work, "install", "workflow_dispatch", { inputs }, fx.base);
        expect(r.code, r.stderr).toBe(0);
        const out = outputs(r.output ?? "");
        expect(out.get("select_upgrade_smoke_arm64")).toBe(selected);
        expect(out.get("plan_ok")).toBe(ok);
        expect(out.get("select_install_smoke")).toBe("true");
      }
    },
    TIMEOUT,
  );

  test(
    "unknown events and unbound checkouts are recorded, not narrowed",
    async () => {
      const fx = new PrCheckout();
      const work = fx.clone();
      const unknown = await cli(work, "web", "schedule", {}, fx.base);
      expect(unknown.stdout).toBe('{"mode": "full", "reason_code": "EVENT_UNKNOWN"}\n');
      expect(outputs(unknown.output ?? "").get("plan_ok")).toBe("false");
      const mismatch = await cli(work, "web", "schedule", {}, "f".repeat(40));
      expect(mismatch.stdout).toBe('{"mode": "full", "reason_code": "TESTED_SHA_MISMATCH"}\n');
      const docs = fx.branch({ "README.md": "x\n" });
      git(work, "fetch", "-q", "origin", docs);
      git(work, "checkout", "-q", "--detach", docs);
      const direct = await cli(work, "web", "pull_request", prEvent(fx.base, docs), docs);
      expect(direct.stdout).toBe('{"mode": "full", "reason_code": "FULL_PR_CHECKOUT_NOT_MERGE"}\n');
      expect(outputs(direct.output ?? "").get("plan_ok")).toBe("true");
    },
    TIMEOUT,
  );

  test(
    "refusals exit 1 with the reason and write nothing",
    async () => {
      const fx = new PrCheckout();
      const work = fx.clone();
      const cases: [string, string | undefined, unknown, string][] = [
        ["web", undefined, {}, "plan: GITHUB_EVENT_NAME required\n"],
        ["web", "  ", {}, "plan: GITHUB_EVENT_NAME required\n"],
        [
          "web",
          "push",
          "{nope",
          "plan: event JSON unreadable: Expecting property name enclosed in double quotes at char 1\n",
        ],
        ["web", "push", "[]", "plan: event payload invalid: event is not an object\n"],
        [
          "web",
          "pull_request",
          prEvent(5, fx.base),
          "plan: event payload invalid: pull_request sha is not a string\n",
        ],
        [
          "web",
          "workflow_dispatch",
          "\u{feff}{}",
          "plan: event JSON unreadable: Expecting value at char 0\n",
        ],
      ];
      for (const [workflow, eventName, event, stderr] of cases) {
        const r = await cli(work, workflow, eventName, event, fx.base);
        expect({
          code: r.code,
          stderr: r.stderr,
          stdout: r.stdout,
          plan: r.plan,
          output: r.output,
        }).toEqual({
          code: 1,
          stderr,
          stdout: "",
          plan: null,
          output: null,
        });
      }

      const yml = join(work, ".github/workflows/web.yml");
      writeFileSync(yml, readFileSync(yml, "utf8").replace(/\n {6}select_web_static: [^\n]*/, ""));
      const registry = await cli(work, "rust", "push", {}, fx.base);
      expect(registry).toMatchObject({
        code: 1,
        stdout: "",
        stderr:
          "plan: workflow registry validation failed\nweb: missing selector output select_web_static\n",
        plan: null,
        output: null,
      });
      git(work, "checkout", "-q", "--", ".github/workflows/web.yml");
    },
    TIMEOUT,
  );

  test("usage errors exit 2", () => {
    const lines: string[] = [];
    const io = {
      env: {},
      stdout: (t: string) => lines.push(t),
      stderr: (t: string) => lines.push(t),
    };
    for (const argv of [
      [],
      ["--workflow", "nope", "--event-json", "e", "--output-plan", "p"],
      ["--workflow"],
      ["--workflow", "web", "--event-json", "e", "--output-plan", "p", "extra"],
      ["--bogus", "x"],
    ]) {
      lines.length = 0;
      expect(main(argv, io), argv.join(" ")).toBe(2);
      expect(lines.join("")).toStartWith(
        "usage: plan.ts [-h] --workflow {collab-engine,documents,install,rust,web}",
      );
    }
    lines.length = 0;
    expect(main(["--help"], io)).toBe(0);
    expect(main(["--he"], io)).toBe(0);
  });

  test("option values follow argparse", () => {
    const base = ["--workflow", "web", "--output-plan", "p"];
    for (const value of ["-", "-.5", "-1", "-x y"]) {
      expect(parseArgs([...base, "--event-json", value]), value).toMatchObject({
        eventJson: value,
      });
    }
    expect(parseArgs(["--work=rust", "--event", "e", "--output-plan", "p"])).toMatchObject({
      workflow: "rust",
      eventJson: "e",
      githubOutput: null,
    });
    expect(() => parseArgs([...base, "--event-json", "-x"])).toThrow("expected one argument");
  });

  test(
    "the registry step is injected; its errors stop the plan before any output",
    () => {
      const fx = new PrCheckout();
      const work = fx.clone();
      const dir = tempDir("inject");
      const args: PlanArgs = {
        workflow: "web",
        repoRoot: work,
        eventJson: join(dir, "event.json"),
        outputPlan: join(dir, "plan.json"),
        githubOutput: join(dir, "out.txt"),
      };
      writeFileSync(args.eventJson, "{}");
      let seen: string[] = [];
      const err: string[] = [];
      const code = runPlan(
        args,
        {
          env: { GITHUB_EVENT_NAME: "push", GITHUB_SHA: fx.base },
          stdout: () => {},
          stderr: (t) => err.push(t),
        },
        (ctx) => {
          seen = [...(ctx.workflows?.keys() ?? [])];
          return ["web: injected failure"];
        },
      );
      expect(code).toBe(1);
      expect(err.join("")).toBe(
        "plan: workflow registry validation failed\nweb: injected failure\n",
      );
      expect(seen).toContain("rust.yml");
      expect(existsSync(args.outputPlan)).toBe(false);
    },
    TIMEOUT,
  );
});
