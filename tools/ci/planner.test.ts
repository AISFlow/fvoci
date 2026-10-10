import { YAML } from "bun";
import { afterAll, describe, expect, test } from "bun:test";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

// Drives the approved planner (scripts/ci_selection.py) only through its
// `plan`, `gate` and `verify-workflows` CLI, with event JSON files, synthetic
// needs JSON and throwaway git repositories. The planner stays the single
// policy implementation; these tests hold the observable contract.

const root = resolve(import.meta.dir, "../..");
// The same per-test budget web-checks passes to bun test.
const TIMEOUT = 60_000;
const SHA_A = "a".repeat(40);
const SHA_B = "b".repeat(40);
const GATE_WORKFLOWS = ["rust", "web", "install", "documents", "collab-engine"] as const;
// install/upgrade-smoke-arm64 is a manual opt-in: no event or path policy
// selects it, not even full mode. Only workflow_dispatch with
// inputs.run_upgrade_smoke_arm == "true" does. It is the only such job.
const OPT_IN = { workflow: "install", job: "upgrade-smoke-arm64", input: "run_upgrade_smoke_arm" };
const POSTGRES_MATRIX_EXPR = "${{ fromJSON(needs.ci-plan.outputs.postgres_matrix) }}";
const POSTGRES_MATRIX_LINE = `      matrix: ${POSTGRES_MATRIX_EXPR}\n`;

type Plan = {
  version: number;
  workflow: string;
  mode: "full" | "narrow";
  reason_code: string;
  plan_ok: boolean;
  base_sha: string | null;
  head_sha: string | null;
  tested_sha: string | null;
  jobs: Record<string, { selected: boolean }>;
};
type Row = Record<string, string>;
type Run = { code: number; stdout: string; stderr: string };
type PlanRun = Run & { plan: Plan | null; outputs: Record<string, string>; text: string };

const scratch: string[] = [];
afterAll(() => {
  for (const dir of scratch) rmSync(dir, { recursive: true, force: true });
});

function tempDir(label: string): string {
  const dir = mkdtempSync(join(tmpdir(), `fvoci-planner-${label}-`));
  scratch.push(dir);
  return dir;
}

function cleanEnv(extra: Record<string, string | undefined> = {}): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    // A CI job exports its own GITHUB_* event; never let it leak into a case.
    if (value !== undefined && !key.startsWith("GITHUB_")) env[key] = value;
  }
  for (const [key, value] of Object.entries(extra)) {
    if (value !== undefined) env[key] = value;
  }
  return env;
}

async function run(cmd: string[], cwd: string, env: Record<string, string>): Promise<Run> {
  const proc = Bun.spawn(cmd, { cwd, env, stdout: "pipe", stderr: "pipe", stdin: "ignore" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { code, stdout, stderr };
}

function git(cwd: string, ...args: string[]): string {
  const proc = Bun.spawnSync(["git", ...args], {
    cwd,
    env: cleanEnv(),
    stdout: "pipe",
    stderr: "pipe",
  });
  if (proc.exitCode !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${proc.stderr.toString()}`);
  }
  return proc.stdout.toString().trim();
}

async function pool<T, R>(
  items: readonly T[],
  fn: (item: T) => Promise<R>,
  limit = 8,
): Promise<R[]> {
  const results = new Array<R>(items.length);
  let next = 0;
  const workers = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length) {
      const index = next++;
      results[index] = await fn(items[index] as T);
    }
  });
  await Promise.all(workers);
  return results;
}

function parseGithubOutput(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (!line) continue;
    const heredoc = /^([A-Za-z0-9_]+)<<(.+)$/.exec(line);
    if (heredoc) {
      const [, key, delimiter] = heredoc;
      const body: string[] = [];
      i++;
      while (i < lines.length && lines[i] !== delimiter) body.push(lines[i++] ?? "");
      if (i >= lines.length) throw new Error(`unterminated ${String(key)} output`);
      out[key as string] = body.join("\n");
      continue;
    }
    const eq = line.indexOf("=");
    if (eq < 0) throw new Error(`malformed output line ${line}`);
    out[line.slice(0, eq)] = line.slice(eq + 1);
  }
  return out;
}

function planner(plannerRoot: string): string {
  return join(plannerRoot, "scripts", "ci_selection.py");
}

let planCounter = 0;
async function runPlan(
  work: string,
  workflow: string,
  eventName: string | null,
  payload: unknown,
  tested: string,
  plannerRoot = root,
): Promise<PlanRun> {
  const id = `${workflow}-${String(planCounter++)}`;
  const dir = tempDir("plan");
  const event = join(dir, `${id}-event.json`);
  const output = join(dir, `${id}-plan.json`);
  const githubOutput = join(dir, `${id}-github.txt`);
  writeFileSync(event, JSON.stringify(payload));
  const result = await run(
    [
      "python3",
      "-B",
      planner(plannerRoot),
      "plan",
      "--workflow",
      workflow,
      "--repo-root",
      work,
      "--event-json",
      event,
      "--output-plan",
      output,
      "--github-output",
      githubOutput,
    ],
    work,
    cleanEnv({ GITHUB_EVENT_NAME: eventName ?? undefined, GITHUB_SHA: tested }),
  );
  const plan = existsSync(output) ? (JSON.parse(readFileSync(output, "utf8")) as Plan) : null;
  const text = existsSync(githubOutput) ? readFileSync(githubOutput, "utf8") : "";
  return { ...result, plan, text, outputs: text ? parseGithubOutput(text) : {} };
}

async function runGate(
  workflow: string,
  needs: string,
  tested: string,
  eventName: string | null,
  eventPayload: unknown = {},
): Promise<Run> {
  const dir = tempDir("gate");
  const event = join(dir, "event.json");
  writeFileSync(
    event,
    typeof eventPayload === "string" ? eventPayload : JSON.stringify(eventPayload),
  );
  return run(
    [
      "python3",
      "-B",
      planner(root),
      "gate",
      "--workflow",
      workflow,
      "--needs-json",
      needs,
      "--tested-sha",
      tested,
    ],
    root,
    cleanEnv({ GITHUB_EVENT_NAME: eventName ?? undefined, GITHUB_EVENT_PATH: event }),
  );
}

function honestResults(plan: Plan): Record<string, string> {
  return Object.fromEntries(
    Object.entries(plan.jobs).map(([job, meta]) => [job, meta.selected ? "success" : "skipped"]),
  );
}

function needsJson(
  plan: Plan,
  outputs: Record<string, string> | null,
  results: Record<string, string>,
  planResult = "success",
): string {
  const needs: Record<string, unknown> = {
    "ci-plan": outputs === null ? { result: planResult } : { result: planResult, outputs },
  };
  for (const job of Object.keys(plan.jobs))
    needs[job] = { result: results[job] ?? "skipped", outputs: {} };
  return JSON.stringify(needs);
}

function expectPlan(r: PlanRun, context: string): Plan {
  expect(r.code, `${context}: ${r.stderr}`).toBe(0);
  expect(r.plan, context).not.toBeNull();
  return r.plan as Plan;
}

function expectFullSelection(workflow: string, plan: Plan, context: string): void {
  for (const [job, meta] of Object.entries(plan.jobs)) {
    const optIn = workflow === OPT_IN.workflow && job === OPT_IN.job;
    expect(meta.selected, `${context} ${workflow}/${job}`).toBe(!optIn);
  }
}

// ---- Repository fixtures -------------------------------------------------

const RUST_REGISTRY_SCRIPTS = [
  "scripts/run-rust-collaboration-ci-tests.sh",
  "scripts/collab-capacity-probe.sh",
];

function writeFile(repo: string, rel: string, content: string): void {
  const path = join(repo, rel);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}

function copyWorkflows(dst: string): void {
  const src = join(root, ".github", "workflows");
  mkdirSync(join(dst, ".github", "workflows"), { recursive: true });
  for (const name of readdirSync(src)) {
    if (name.endsWith(".yml") || name.endsWith(".yaml")) {
      copyFileSync(join(src, name), join(dst, ".github", "workflows", name));
    }
  }
}

/** Real workflows plus the trimmed Cargo [[test]] registry the planner accepts. */
function writeRegistryTree(dst: string): void {
  copyWorkflows(dst);
  for (const rel of RUST_REGISTRY_SCRIPTS) {
    mkdirSync(dirname(join(dst, rel)), { recursive: true });
    copyFileSync(join(root, rel), join(dst, rel));
  }
  writeFile(
    dst,
    "Cargo.toml",
    [
      "[features]",
      "db-tests = []",
      "",
      "[[test]]",
      'name = "db_integration"',
      'path = "tests/db_integration.rs"',
      'required-features = ["db-tests"]',
      "",
    ].join("\n"),
  );
}

function configure(repo: string): void {
  git(repo, "config", "user.email", "ci@test");
  git(repo, "config", "user.name", "ci");
  git(repo, "config", "commit.gpgsign", "false");
}

/** Origin repo with GitHub-like PR merge checkouts (mirrors PrCheckoutFixture). */
class Origin {
  readonly dir = tempDir("origin");
  readonly base: string;
  private branches = 0;

  constructor() {
    git(this.dir, "init", "-q", "-b", "main");
    configure(this.dir);
    git(this.dir, "config", "uploadpack.allowReachableSHA1InWant", "true");
    writeRegistryTree(this.dir);
    writeFile(this.dir, "README.md", "base docs\n");
    git(this.dir, "add", ".");
    git(this.dir, "commit", "-q", "-m", "base");
    this.base = git(this.dir, "rev-parse", "HEAD");
  }

  branch(files: Record<string, string>): string {
    const name = `pr-${String(this.branches++)}`;
    git(this.dir, "checkout", "-q", "-B", name, "main");
    for (const [rel, content] of Object.entries(files)) writeFile(this.dir, rel, content);
    git(this.dir, "add", "-A");
    git(this.dir, "commit", "-q", "-m", name);
    const sha = git(this.dir, "rev-parse", "HEAD");
    git(this.dir, "checkout", "-q", "main");
    return sha;
  }

  /** A clone checked out at the merge GitHub tests for a PR (first parent = base). */
  mergeClone(head: string): { work: string; tested: string } {
    const work = this.clone();
    git(work, "checkout", "-q", "-B", "tested", this.base);
    git(work, "merge", "-q", "--no-ff", "-m", "github merge", head);
    return { work, tested: git(work, "rev-parse", "HEAD") };
  }

  clone(): string {
    const work = join(tempDir("work"), "repo");
    git(this.dir, "clone", "-q", this.dir, work);
    configure(work);
    return work;
  }
}

let sharedOrigin: Origin | null = null;
function origin(): Origin {
  sharedOrigin ??= new Origin();
  return sharedOrigin;
}

function prPayload(base: string, head: string): Record<string, unknown> {
  return { pull_request: { base: { sha: base }, head: { sha: head } } };
}

/** Plan a pull_request whose diff is exactly `files`. */
async function planPr(
  files: Record<string, string>,
  workflows: readonly string[],
  plannerRoot = root,
): Promise<Record<string, PlanRun>> {
  const o = origin();
  const head = o.branch(files);
  const { work, tested } = o.mergeClone(head);
  const out: Record<string, PlanRun> = {};
  for (const workflow of workflows) {
    out[workflow] = await runPlan(
      work,
      workflow,
      "pull_request",
      prPayload(o.base, head),
      tested,
      plannerRoot,
    );
  }
  return out;
}

async function planPaths(
  paths: readonly string[],
  workflows: readonly string[],
  plannerRoot = root,
): Promise<Record<string, Record<string, PlanRun>>> {
  const files = paths.map((path) => ({ [path]: `change ${path}\n` }));
  // Branch creation shares the origin index; create them in order.
  const prepared = files.map((f) => {
    const o = origin();
    const head = o.branch(f);
    return { head, ...o.mergeClone(head) };
  });
  const runs = await pool(prepared, async ({ head, work, tested }) => {
    const out: Record<string, PlanRun> = {};
    for (const workflow of workflows) {
      out[workflow] = await runPlan(
        work,
        workflow,
        "pull_request",
        prPayload(origin().base, head),
        tested,
        plannerRoot,
      );
    }
    return out;
  });
  return Object.fromEntries(paths.map((path, i) => [path, runs[i] as Record<string, PlanRun>]));
}

/** A copy of the planner with its ROOT-read inputs, optionally mutated. */
function plannerCopy(
  mutate: { source?: (s: string) => string; formatWeb?: (s: string) => string } = {},
): string {
  const dir = tempDir("planner");
  const source = readFileSync(planner(root), "utf8");
  const mutated = mutate.source ? mutate.source(source) : source;
  writeFile(dir, "scripts/ci_selection.py", mutated);
  const formatWeb = readFileSync(join(root, "scripts/format-web.sh"), "utf8");
  writeFile(
    dir,
    "scripts/format-web.sh",
    mutate.formatWeb ? mutate.formatWeb(formatWeb) : formatWeb,
  );
  copyFileSync(join(root, ".prettierignore"), join(dir, ".prettierignore"));
  copyFileSync(join(root, "package.json"), join(dir, "package.json"));
  copyWorkflows(dir);
  return dir;
}

function replaceOnce(text: string, needle: string, replacement: string): string {
  const at = text.indexOf(needle);
  if (at < 0) throw new Error(`needle not found: ${needle}`);
  if (text.indexOf(needle, at + needle.length) >= 0)
    throw new Error(`needle not unique: ${needle}`);
  return text.slice(0, at) + replacement + text.slice(at + needle.length);
}

// ---- Postgres matrix oracle ---------------------------------------------

type WorkflowDoc = {
  on: Record<string, unknown>;
  jobs: Record<string, Record<string, unknown>>;
};

function loadWorkflow(name: string, base = root): WorkflowDoc {
  return YAML.parse(
    readFileSync(join(base, ".github/workflows", `${name}.yml`), "utf8"),
  ) as WorkflowDoc;
}

function postgresCatalog(): Row[] {
  const job = loadWorkflow("rust").jobs.postgres as { env: Record<string, string> };
  const rows = JSON.parse(job.env.FVOCI_POSTGRES_MATRIX_CATALOG ?? "null") as unknown;
  if (!Array.isArray(rows)) throw new Error("postgres catalog must be a list");
  return rows as Row[];
}

const catalog = postgresCatalog();
// Policy: pull_request runs PG 18 on x64 only; every other known event runs all.
const prRows = catalog.filter((row) => row.runner === "ubuntu-26.04" && row.pg_major === "18");
const FULL_MATRIX = JSON.stringify({ include: catalog });
const PR_MATRIX = JSON.stringify({ include: prRows });

function emittedMatrix(r: PlanRun): { include: Row[] } {
  const raw = r.outputs.postgres_matrix;
  if (raw === undefined) throw new Error(`postgres_matrix missing: ${r.text}`);
  expect(raw).not.toContain("\n");
  return JSON.parse(raw) as { include: Row[] };
}

// =========================================================================

describe("postgres matrix catalog", () => {
  test("catalog shape and the pull_request subset", () => {
    expect(catalog).toHaveLength(12);
    expect(new Set(catalog.map((row) => row.pg_major))).toEqual(new Set(["16", "17", "18"]));
    expect(new Set(catalog.map((row) => row.runner))).toEqual(
      new Set(["ubuntu-26.04", "ubuntu-26.04-arm"]),
    );
    expect(prRows.map((row) => row.check)).toEqual(["postgres", "postgres-c", "postgres-b"]);
    expect(catalog.length - prRows.length).toBe(9);
  });

  // RustSuiteRegistryTest.test_postgres_workflow_matrix_is_plan_fromjson_not_static_include
  test("rust.yml postgres runs the plan matrix, not a static include", () => {
    const jobs = loadWorkflow("rust").jobs;
    const plan = jobs["ci-plan"] as { outputs: Record<string, string> };
    expect(plan.outputs.postgres_matrix).toBe("${{ steps.plan.outputs.postgres_matrix }}");
    const strategy = (job: string) =>
      (jobs[job] as { strategy: { matrix: unknown } }).strategy.matrix;
    expect(strategy("postgres")).toBe(POSTGRES_MATRIX_EXPR);
    expect(strategy("postgres-build")).not.toEqual(POSTGRES_MATRIX_EXPR);
    expect(strategy("collaboration")).not.toEqual(POSTGRES_MATRIX_EXPR);
  });

  // RustSuiteRegistryTest.test_postgres_matrix_follows_event_and_rejects_check_mutations
  test(
    "verify-workflows rejects static, exclude and fallback matrices",
    async () => {
      const source = readFileSync(join(root, ".github/workflows/rust.yml"), "utf8");
      expect(source).toContain(POSTGRES_MATRIX_LINE);
      const flow = JSON.stringify(catalog);
      const mutations: Record<string, { text: string; needle?: string }> = {
        "static-include": { text: `      matrix: {"include": ${flow}, "exclude": []}\n` },
        "plain-static-include": { text: `      matrix: {"include": ${flow}}\n` },
        "exclude-object": {
          text: `      matrix: {"include": ${flow}, "exclude": "\${{ fromJSON(needs.ci-plan.outputs.postgres_exclude) }}"}\n`,
        },
        "exclude-output": {
          text: "      matrix: ${{ fromJSON(needs.ci-plan.outputs.postgres_exclude) }}\n",
        },
        "array-fallback": {
          text: "      matrix: ${{ fromJSON(needs.ci-plan.outputs.postgres_matrix || '[]') }}\n",
          needle: "without an empty-array fallback",
        },
        "include-fallback": {
          text: `      matrix: \${{ fromJSON(needs.ci-plan.outputs.postgres_matrix || '{"include":[]}') }}\n`,
          needle: "without an empty-array fallback",
        },
      };
      const baseline = tempDir("verify");
      writeRegistryTree(baseline);
      const ok = await run(
        ["python3", "-B", planner(root), "verify-workflows", "--repo-root", baseline],
        root,
        cleanEnv(),
      );
      expect(ok.code, ok.stderr).toBe(0);
      await pool(Object.entries(mutations), async ([label, { text, needle }]) => {
        const dir = tempDir("verify");
        writeRegistryTree(dir);
        const path = join(dir, ".github/workflows/rust.yml");
        writeFileSync(path, replaceOnce(source, POSTGRES_MATRIX_LINE, text));
        const r = await run(
          ["python3", "-B", planner(root), "verify-workflows", "--repo-root", dir],
          root,
          cleanEnv(),
        );
        expect(r.code, label).not.toBe(0);
        if (needle) expect(r.stderr, label).toContain(needle);
      });
    },
    TIMEOUT,
  );

  // Source-shape part of test_postgres_matrix_follows_event_and_rejects_check_mutations.
  test("matrix policy is one event comparison and reads no event payload", () => {
    const source = readFileSync(planner(root), "utf8");
    expect(source.split('event_name != "pull_request"')).toHaveLength(2);
    const body = (name: string): string => {
      const start = source.indexOf(`\ndef ${name}(`);
      expect(start, name).toBeGreaterThan(0);
      const end = source.indexOf("\ndef ", start + 1);
      return source.slice(start, end);
    };
    for (const [name, params] of [
      ["postgres_matrix_row_runs", "event_name: str, row: dict"],
      ["postgres_matrix_include", "event_name: str, rows: list[dict]"],
      ["postgres_matrix_json", "event_name: str, rows: list[dict]"],
    ] as const) {
      const text = body(name);
      expect(text, name).toContain(`def ${name}(${params})`);
      const code = text.replace(/"""[\s\S]*?"""/g, "");
      for (const forbidden of [
        "labels",
        "body",
        "payload",
        "github",
        '"head"',
        '"ref"',
        "pull_request.head",
        ".ref",
      ]) {
        expect(code.includes(forbidden), `${name} must not read ${forbidden}`).toBe(false);
      }
    }
    const plan = body("cmd_plan");
    expect(plan.split("postgres_matrix_json(")).toHaveLength(2);
    expect(plan).toContain("postgres_matrix_json(event_name, rows)");
  });
});

// =========================================================================
// Event x change-kind matrix through the real plan and gate.

const CHANGE_KINDS = {
  docs: { "docs/rewrite.md": "docs only lane\n" },
  "fixture-md": { "compat/fixtures/markdown-oracle/x.md": "oracle\n" },
  code: { "src/lib.rs": "// code\n" },
} as const;

describe("event x change kind", () => {
  test(
    "plans and honest gates for every workflow",
    async () => {
      const o = origin();
      const prs = Object.entries(CHANGE_KINDS).map(([kind, files]) => {
        const head = o.branch(files);
        return { kind, head, ...o.mergeClone(head) };
      });
      const atBase = o.clone();
      const baseSha = git(atBase, "rev-parse", "HEAD");
      type Case = {
        label: string;
        event: string;
        payload: unknown;
        work: string;
        tested: string;
        mode: Plan["mode"];
        reason: string;
        ok: boolean;
        selection: "none" | "full";
      };
      const cases: Case[] = [];
      for (const pr of prs) {
        const narrow = pr.kind === "docs";
        cases.push({
          label: `pull_request/${pr.kind}`,
          event: "pull_request",
          payload: prPayload(o.base, pr.head),
          work: pr.work,
          tested: pr.tested,
          mode: narrow ? "narrow" : "full",
          reason: narrow ? "NARROW_DOCS" : "FULL_PATH_BROADEN",
          ok: true,
          selection: narrow ? "none" : "full",
        });
      }
      const fullEvents: [string, unknown, string, boolean][] = [
        [
          "merge_group",
          { merge_group: { base_sha: SHA_A, head_sha: SHA_B } },
          "FULL_EVENT_MERGE_GROUP",
          true,
        ],
        [
          "push",
          { before: SHA_A, after: baseSha, ref: "refs/heads/main" },
          "FULL_EVENT_PUSH",
          true,
        ],
        [
          "workflow_dispatch",
          { ref: "refs/heads/main", inputs: {} },
          "FULL_EVENT_WORKFLOW_DISPATCH",
          true,
        ],
        ["schedule", { schedule: "0 0 * * *" }, "EVENT_UNKNOWN", false],
      ];
      for (const [event, payload, reason, ok] of fullEvents) {
        cases.push({
          label: event,
          event,
          payload,
          work: atBase,
          tested: baseSha,
          mode: "full",
          reason,
          ok,
          selection: "full",
        });
      }
      const jobs = cases.flatMap((c) => GATE_WORKFLOWS.map((workflow) => ({ c, workflow })));
      await pool(jobs, async ({ c, workflow }) => {
        const context = `${c.label} ${workflow}`;
        // The work tree is shared by the workflows of one case; plans only read it.
        const r = await runPlan(c.work, workflow, c.event, c.payload, c.tested);
        const plan = expectPlan(r, context);
        expect(plan.mode, context).toBe(c.mode);
        expect(plan.reason_code, context).toBe(c.reason);
        expect(plan.plan_ok, context).toBe(c.ok);
        expect(r.outputs.plan_ok, context).toBe(c.ok ? "true" : "false");
        if (c.selection === "none") {
          expect(
            Object.values(plan.jobs).some((meta) => meta.selected),
            context,
          ).toBe(false);
        } else {
          expectFullSelection(workflow, plan, context);
        }
        if (workflow === "rust") {
          // A pull request runs the reduced matrix even when its paths make the
          // plan full; every other event runs the full catalog.
          const expected = c.event === "pull_request" ? prRows : catalog;
          expect(emittedMatrix(r).include, context).toEqual(expected);
        } else {
          expect(r.outputs.postgres_matrix, context).toBeUndefined();
        }
        const gate = await runGate(
          workflow,
          needsJson(plan, r.outputs, honestResults(plan)),
          c.tested,
          c.event,
          c.payload,
        );
        if (c.ok) {
          expect(gate.code, `${context}: ${gate.stderr}`).toBe(0);
        } else {
          expect(gate.code, context).toBe(1);
          expect(gate.stderr, context).toContain("PLAN_NOT_OK");
        }
        // A selected lane reported as skipped (or cancelled/failed) never passes.
        const selected = Object.entries(plan.jobs).find(([, meta]) => meta.selected)?.[0];
        if (selected && c.ok) {
          for (const result of workflow === "rust"
            ? ["skipped", "cancelled", "failure"]
            : ["skipped"]) {
            const bad = await runGate(
              workflow,
              needsJson(plan, r.outputs, { ...honestResults(plan), [selected]: result }),
              c.tested,
              c.event,
              c.payload,
            );
            expect(bad.code, `${context} ${selected}=${result}`).toBe(1);
            expect(bad.stderr).toContain(`selected job ${selected} must succeed, got ${result}`);
          }
        }
        // An unselected lane that ran never passes.
        const unselected = Object.entries(plan.jobs).find(([, meta]) => !meta.selected)?.[0];
        if (unselected && c.ok) {
          const bad = await runGate(
            workflow,
            needsJson(plan, r.outputs, { ...honestResults(plan), [unselected]: "success" }),
            c.tested,
            c.event,
            c.payload,
          );
          expect(bad.code, `${context} ${unselected}=success`).toBe(1);
          expect(bad.stderr).toContain(`unselected job ${unselected} must be skipped`);
        }
      });
    },
    TIMEOUT,
  );

  test(
    "workflow_dispatch opt-in selects only the manual job and the gate re-derives it",
    async () => {
      const o = origin();
      const work = o.clone();
      const tested = git(work, "rev-parse", "HEAD");
      const chosen = { inputs: { [OPT_IN.input]: "true" } };
      const r = await runPlan(work, OPT_IN.workflow, "workflow_dispatch", chosen, tested);
      const plan = expectPlan(r, "dispatch opt-in");
      expect(Object.values(plan.jobs).every((meta) => meta.selected)).toBe(true);
      const ok = await runGate(
        OPT_IN.workflow,
        needsJson(plan, r.outputs, honestResults(plan)),
        tested,
        "workflow_dispatch",
        chosen,
      );
      expect(ok.code, ok.stderr).toBe(0);
      // The same plan under an event that did not choose the input fails.
      const replay = await runGate(
        OPT_IN.workflow,
        needsJson(plan, r.outputs, honestResults(plan)),
        tested,
        "workflow_dispatch",
        { inputs: {} },
      );
      expect(replay.code).toBe(1);
      expect(replay.stderr).toContain(`OPT_IN_MISMATCH ${OPT_IN.job}`);
    },
    TIMEOUT,
  );
});

// =========================================================================
// Gate: plan result, plan output and postgres matrix content.

async function rustFullPlan(): Promise<{
  plan: Plan;
  outputs: Record<string, string>;
  tested: string;
}> {
  const o = origin();
  const head = o.branch({ "src/lib.rs": "// gate fixture\n" });
  const { work, tested } = o.mergeClone(head);
  const r = await runPlan(work, "rust", "pull_request", prPayload(o.base, head), tested);
  const plan = expectPlan(r, "rust full plan");
  expect(plan.jobs.postgres?.selected).toBe(true);
  return { plan, outputs: r.outputs, tested };
}

describe("gate", () => {
  // GateSchemaTest.test_plan_failure_or_skip_rejects_empty_postgres_matrix
  test(
    "a plan job that did not succeed fails the gate whatever its outputs",
    async () => {
      const { plan, outputs, tested } = await rustFullPlan();
      const results = Object.fromEntries(Object.keys(plan.jobs).map((job) => [job, "skipped"]));
      const matrices = [null, "", "[]", "{}", '{"include":[]}', PR_MATRIX];
      const cases = ["failure", "skipped", "cancelled"].flatMap((planResult) =>
        matrices.map((matrix) => ({ planResult, matrix })),
      );
      await pool(cases, async ({ planResult, matrix }) => {
        const out: Record<string, string> = { plan_json: outputs.plan_json ?? "" };
        if (matrix !== null) out.postgres_matrix = matrix;
        const r = await runGate(
          "rust",
          needsJson(plan, out, results, planResult),
          tested,
          "pull_request",
        );
        expect(r.code, `${planResult} ${String(matrix)}`).toBe(1);
        expect(r.stderr).toContain("PLAN_RESULT");
      });
    },
    TIMEOUT,
  );

  test(
    "missing or malformed plan output fails the gate",
    async () => {
      const { plan, outputs, tested } = await rustFullPlan();
      const results = honestResults(plan);
      const planJson = outputs.plan_json ?? "";
      const cases: [string, Record<string, string> | null, string][] = [
        ["no-outputs", null, "PLAN_JSON_MISSING"],
        ["no-plan-json", { postgres_matrix: PR_MATRIX }, "PLAN_JSON_MISSING"],
        ["blank-plan-json", { plan_json: " ", postgres_matrix: PR_MATRIX }, "PLAN_JSON_MISSING"],
        [
          "malformed-plan-json",
          { plan_json: "{", postgres_matrix: PR_MATRIX },
          "PLAN_JSON_MALFORMED",
        ],
        [
          "plan-not-ok",
          { plan_json: JSON.stringify({ ...plan, plan_ok: false }), postgres_matrix: PR_MATRIX },
          "PLAN_NOT_OK",
        ],
        [
          "wrong-tested-sha",
          { plan_json: planJson.replace(tested, SHA_A), postgres_matrix: PR_MATRIX },
          "tested_sha",
        ],
      ];
      await pool(cases, async ([label, out, needle]) => {
        const r = await runGate("rust", needsJson(plan, out, results), tested, "pull_request");
        expect(r.code, label).toBe(1);
        expect(r.stderr, label).toContain(needle);
      });
      for (const raw of ["{not-json", "[]", ""]) {
        const r = await runGate("rust", raw, tested, "pull_request");
        expect(r.code, raw).toBe(1);
      }
    },
    TIMEOUT,
  );

  // GateSchemaTest.test_selected_postgres_rejects_empty_or_missing_matrix_output
  test(
    "selected postgres rejects a missing, malformed or empty matrix",
    async () => {
      const { plan, outputs, tested } = await rustFullPlan();
      const results = honestResults(plan);
      const cases: Record<string, string | null> = {
        missing: null,
        blank: "",
        array: "[]",
        object: "{}",
        "empty-include": '{"include":[]}',
        "empty-row": '{"include":[{}]}',
        null: "null",
        malformed: "{",
        "extra-key": JSON.stringify({ include: prRows, exclude: [] }),
      };
      await pool(Object.entries(cases), async ([label, matrix]) => {
        const out: Record<string, string> = { plan_json: outputs.plan_json ?? "" };
        if (matrix !== null) out.postgres_matrix = matrix;
        const r = await runGate("rust", needsJson(plan, out, results), tested, "pull_request");
        expect(r.code, label).toBe(1);
        expect(r.stderr, label).toContain("postgres matrix error POSTGRES_MATRIX_");
      });
      const kept = await runGate("rust", needsJson(plan, outputs, results), tested, "pull_request");
      expect(outputs.postgres_matrix).toBe(PR_MATRIX);
      expect(kept.code, kept.stderr).toBe(0);
    },
    TIMEOUT,
  );

  test(
    "postgres matrix content must match the event",
    async () => {
      const { plan, outputs, tested } = await rustFullPlan();
      const results = honestResults(plan);
      const withMatrix = (matrix: string) =>
        needsJson(plan, { ...outputs, postgres_matrix: matrix }, results);
      const pg17x64 = catalog.find((row) => row.runner === "ubuntu-26.04" && row.pg_major === "17");
      const pg18arm = catalog.find(
        (row) => row.runner === "ubuntu-26.04-arm" && row.pg_major === "18",
      );
      expect(pg17x64 && pg18arm).toBeTruthy();
      const swapped = (row: Row) => JSON.stringify({ include: [row, ...prRows.slice(1)] });
      const cases: [string, string, string | null, string][] = [
        ["pull_request gets full", "pull_request", FULL_MATRIX, "POSTGRES_MATRIX_ROW_COUNT"],
        ["merge_group gets reduced", "merge_group", PR_MATRIX, "POSTGRES_MATRIX_ROW_COUNT"],
        ["push gets reduced", "push", PR_MATRIX, "POSTGRES_MATRIX_ROW_COUNT"],
        [
          "workflow_dispatch gets reduced",
          "workflow_dispatch",
          PR_MATRIX,
          "POSTGRES_MATRIX_ROW_COUNT",
        ],
        [
          "pull_request one row",
          "pull_request",
          JSON.stringify({ include: prRows.slice(0, 1) }),
          "POSTGRES_MATRIX_ROW_COUNT",
        ],
        [
          "pull_request pg17 swapped in",
          "pull_request",
          swapped(pg17x64 as Row),
          "POSTGRES_MATRIX_ROWS",
        ],
        [
          "pull_request arm64 swapped in",
          "pull_request",
          swapped(pg18arm as Row),
          "POSTGRES_MATRIX_ROWS",
        ],
        [
          "pull_request edited image",
          "pull_request",
          JSON.stringify({
            include: prRows.map((row, i) => (i ? row : { ...row, postgres_image: "postgres:18" })),
          }),
          "POSTGRES_MATRIX_ROWS",
        ],
        [
          "merge_group duplicated row",
          "merge_group",
          JSON.stringify({ include: [...catalog.slice(1), catalog[1]] }),
          "POSTGRES_MATRIX_ROWS",
        ],
        ["schedule", "schedule", FULL_MATRIX, "POSTGRES_MATRIX_EVENT"],
        ["no event name", "", PR_MATRIX, "POSTGRES_MATRIX_EVENT"],
        ["unset event name", null as unknown as string, PR_MATRIX, "POSTGRES_MATRIX_EVENT"],
      ];
      await pool(cases, async ([label, event, matrix, needle]) => {
        const r = await runGate("rust", withMatrix(matrix as string), tested, event);
        expect(r.code, label).toBe(1);
        expect(r.stderr, label).toContain(needle);
      });
      for (const [event, matrix] of [
        ["pull_request", PR_MATRIX],
        ["merge_group", FULL_MATRIX],
        ["push", FULL_MATRIX],
        ["workflow_dispatch", FULL_MATRIX],
      ] as const) {
        const r = await runGate("rust", withMatrix(matrix), tested, event);
        expect(r.code, `${event}: ${r.stderr}`).toBe(0);
      }
    },
    TIMEOUT,
  );

  // GateSchemaTest.test_unselected_postgres_does_not_require_matrix_output
  test(
    "unselected postgres does not require a matrix output",
    async () => {
      const runs = await planPr({ "docs/other.md": "docs\n" }, ["rust"]);
      const r = runs.rust as PlanRun;
      const plan = expectPlan(r, "rust docs");
      expect(plan.jobs.postgres?.selected).toBe(false);
      const results = honestResults(plan);
      const tested = plan.tested_sha as string;
      const withPlan = (extra: Record<string, string>) =>
        needsJson(plan, { plan_json: r.outputs.plan_json ?? "", ...extra }, results);
      const extras: Record<string, string>[] = [
        {},
        { postgres_matrix: "[]" },
        { postgres_matrix: PR_MATRIX },
      ];
      for (const extra of extras) {
        const gate = await runGate("rust", withPlan(extra), tested, "pull_request");
        expect(gate.code, `${JSON.stringify(extra)} ${gate.stderr}`).toBe(0);
      }
    },
    TIMEOUT,
  );
});

// =========================================================================
// MergeGroupPlanTest

describe("merge_group plan", () => {
  test(
    "copies merge_group SHAs and ignores the pull_request payload",
    async () => {
      const o = origin();
      const head = o.branch({ "docs/rewrite.md": "docs only lane\n" });
      const { work, tested } = o.mergeClone(head);
      const payload = {
        merge_group: { base_sha: SHA_A, head_sha: SHA_B },
        ...prPayload(o.base, head),
        before: "e".repeat(40),
        after: "f".repeat(40),
      };
      const r = await runPlan(work, "web", "merge_group", payload, tested);
      const plan = expectPlan(r, "merge_group");
      expect(plan.base_sha).toBe(SHA_A);
      expect(plan.head_sha).toBe(SHA_B);
      expect(plan.mode).toBe("full");
      expect(plan.reason_code).toBe("FULL_EVENT_MERGE_GROUP");
      expect(plan.plan_ok).toBe(true);
      expectFullSelection("web", plan, "merge_group");
      expect("postgres_matrix" in plan).toBe(false);
    },
    TIMEOUT,
  );

  test(
    "missing or invalid merge_group fields stay full",
    async () => {
      const o = origin();
      const head = o.branch({ "docs/rewrite.md": "docs only lane\n" });
      const { work, tested } = o.mergeClone(head);
      const pr = prPayload(o.base, head).pull_request;
      const cases: [string, unknown, string | null, string | null][] = [
        [
          "missing-group",
          { pull_request: pr, before: "e".repeat(40), after: "f".repeat(40) },
          null,
          null,
        ],
        ["non-object", { merge_group: "queued", pull_request: pr }, null, null],
        ["missing-head", { merge_group: { base_sha: SHA_A }, pull_request: pr }, SHA_A, null],
        [
          "invalid-base",
          { merge_group: { base_sha: "HEAD", head_sha: SHA_B }, pull_request: pr },
          null,
          SHA_B,
        ],
        ["uppercase", { merge_group: { base_sha: "A".repeat(40), head_sha: SHA_B } }, null, SHA_B],
      ];
      await pool(cases, async ([label, payload, base, headSha]) => {
        const plan = expectPlan(await runPlan(work, "rust", "merge_group", payload, tested), label);
        expect(plan.base_sha, label).toBe(base);
        expect(plan.head_sha, label).toBe(headSha);
        expect(plan.mode, label).toBe("full");
        expect(plan.reason_code, label).toBe("FULL_EVENT_MERGE_GROUP");
        expect(plan.plan_ok, label).toBe(true);
        expectFullSelection("rust", plan, label);
        expect([plan.base_sha, plan.head_sha]).not.toContain(o.base);
        expect([plan.base_sha, plan.head_sha]).not.toContain(head);
      });
    },
    TIMEOUT,
  );

  test(
    "a non-object event does not crash",
    async () => {
      const work = origin().clone();
      const tested = git(work, "rev-parse", "HEAD");
      const plan = expectPlan(await runPlan(work, "documents", "merge_group", [], tested), "list");
      expect(plan.base_sha).toBeNull();
      expect(plan.head_sha).toBeNull();
      expect(plan.mode).toBe("full");
      expect(plan.plan_ok).toBe(true);
      expectFullSelection("documents", plan, "list");
    },
    TIMEOUT,
  );

  // MergeGroupPlanTest.test_unknown_event_cli_does_not_narrow_a_docs_merge and
  // PlanSelectionTest.test_unknown_event_stays_full_and_selectable
  test(
    "unknown events stay full, select every lane and report EVENT_UNKNOWN",
    async () => {
      const o = origin();
      const head = o.branch({ "NOTES.md": "ordinary docs\n" });
      const { work, tested } = o.mergeClone(head);
      const cases = ["schedule", "deployment", "not-an-event", "pull_request_target"].flatMap(
        (event) => (["install", "web"] as const).map((workflow) => ({ event, workflow })),
      );
      await pool(cases, async ({ event, workflow }) => {
        const r = await runPlan(work, workflow, event, prPayload(o.base, head), tested);
        const plan = expectPlan(r, event);
        expect(plan.mode, event).toBe("full");
        expect(plan.reason_code, event).toBe("EVENT_UNKNOWN");
        expect(plan.plan_ok, event).toBe(false);
        expect(plan.base_sha, event).toBeNull();
        expect(plan.head_sha, event).toBeNull();
        expectFullSelection(workflow, plan, event);
        expect(r.text, event).not.toContain("postgres_matrix<<");
      });
      // An empty or missing event name produces no plan at all.
      for (const event of ["", null]) {
        const r = await runPlan(work, "web", event, prPayload(o.base, head), tested);
        expect(r.code).toBe(1);
        expect(r.stderr).toContain("GITHUB_EVENT_NAME required");
        expect(r.plan).toBeNull();
        expect(r.text).toBe("");
      }
    },
    TIMEOUT,
  );

  test(
    "the rust matrix is event scoped and outside plan_json",
    async () => {
      const work = origin().clone();
      const tested = git(work, "rev-parse", "HEAD");
      const base = origin().base;
      expect(PR_MATRIX).not.toBe(FULL_MATRIX);
      const pr = await runPlan(
        work,
        "rust",
        "pull_request",
        { ...prPayload(base, tested), merge_group: { base_sha: SHA_A, head_sha: SHA_B } },
        tested,
      );
      const prPlan = expectPlan(pr, "pull_request");
      expect(prPlan.plan_ok).toBe(true);
      expect("postgres_matrix" in prPlan).toBe(false);
      expect("postgres_exclude" in prPlan).toBe(false);
      expect(pr.outputs.plan_json).not.toContain("postgres_matrix");
      expect(pr.text).toContain(
        `postgres_matrix<<POSTGRES_MATRIX_EOF\n${PR_MATRIX}\nPOSTGRES_MATRIX_EOF\n`,
      );
      const mg = await runPlan(
        work,
        "rust",
        "merge_group",
        { merge_group: { base_sha: SHA_A, head_sha: SHA_B }, ...prPayload(base, tested) },
        tested,
      );
      const mgPlan = expectPlan(mg, "merge_group");
      expect([mgPlan.base_sha, mgPlan.head_sha]).toEqual([SHA_A, SHA_B]);
      expect(mgPlan.plan_ok).toBe(true);
      expect(mg.text).toContain(
        `postgres_matrix<<POSTGRES_MATRIX_EOF\n${FULL_MATRIX}\nPOSTGRES_MATRIX_EOF\n`,
      );
      const pushLike = { before: "e".repeat(40), after: tested, ref: "refs/heads/main" };
      for (const event of ["push", "workflow_dispatch"]) {
        const r = await runPlan(work, "rust", event, pushLike, tested);
        expect(expectPlan(r, event).plan_ok, event).toBe(true);
        expect(r.text, event).toContain(
          `postgres_matrix<<POSTGRES_MATRIX_EOF\n${FULL_MATRIX}\nPOSTGRES_MATRIX_EOF\n`,
        );
      }
      const schedule = await runPlan(work, "rust", "schedule", pushLike, tested);
      const schedulePlan = expectPlan(schedule, "schedule");
      expect(schedulePlan.plan_ok).toBe(false);
      expect(schedulePlan.reason_code).toBe("EVENT_UNKNOWN");
      expectFullSelection("rust", schedulePlan, "schedule");
      expect(schedule.text).toContain(
        `postgres_matrix<<POSTGRES_MATRIX_EOF\n${FULL_MATRIX}\nPOSTGRES_MATRIX_EOF\n`,
      );
    },
    TIMEOUT,
  );

  test(
    "the matrix ignores PR-controlled inputs and keeps every full leg",
    async () => {
      const work = origin().clone();
      const tested = git(work, "rev-parse", "HEAD");
      const base = origin().base;
      const prBase = { base: { ref: "main", sha: base }, head: { sha: tested } };
      const prPayloads = [
        {
          ref: "refs/heads/feature",
          pull_request: { ...prBase, head: { ref: "feature", sha: tested }, labels: [], body: "" },
        },
        {
          ref: "refs/heads/full-matrix",
          pull_request: {
            ...prBase,
            head: { ref: "run-all-legs", sha: tested },
            labels: [{ name: "full-ci" }, { name: "postgres-all" }],
            body: "run all 12 postgres legs including arm64",
          },
        },
        {
          ref: "refs/heads/pg18-only",
          pull_request: {
            ...prBase,
            head: { ref: "reduce-matrix", sha: tested },
            labels: [{ name: "pg18-only" }, { name: "skip-arm" }],
            body: "only ubuntu-26.04 pg 18",
          },
        },
      ];
      const stuffed = {
        ref: "refs/heads/feature",
        before: "e".repeat(40),
        after: tested,
        pull_request: {
          base: { ref: "main", sha: base },
          head: { ref: "pull_request", sha: tested },
          labels: [{ name: "pg18-only" }, { name: "reduce-matrix" }],
          body: "only ubuntu-26.04 pg 18",
        },
        merge_group: { base_sha: SHA_A, head_sha: SHA_B },
      };
      const cases = [
        ...prPayloads.map((payload) => ({ event: "pull_request", payload, expected: prRows })),
        ...["merge_group", "push", "workflow_dispatch", "schedule", "pull_request_target"].map(
          (event) => ({
            event,
            payload: stuffed,
            expected: catalog,
          }),
        ),
      ];
      await pool(cases, async ({ event, payload, expected }) => {
        const r = await runPlan(work, "rust", event, payload, tested);
        expect(r.code, `${event}: ${r.stderr}`).toBe(0);
        expect(emittedMatrix(r).include, `${event} ${payload.ref}`).toEqual(expected);
      });
    },
    TIMEOUT,
  );

  test("web-checks runs this file after installing the pinned planner parser", () => {
    const steps = (loadWorkflow("web").jobs["web-checks"] as { steps: Record<string, unknown>[] })
      .steps;
    const runs = steps.map((step) => (typeof step.run === "string" ? step.run : ""));
    const install = runs.findIndex((text) =>
      text.includes("-r scripts/ci_selection_requirements.txt"),
    );
    const self = steps.findIndex(
      (step) => step.run === "bun test --timeout=60000 tools/ci/planner.test.ts",
    );
    expect(install).toBeGreaterThanOrEqual(0);
    expect(self).toBeGreaterThan(install);
    const step = steps[self] as Record<string, unknown>;
    expect("if" in step || "continue-on-error" in step).toBe(false);
  });

  test("required gate names, triggers and no paths filters on the five gate workflows", () => {
    const rust = readFileSync(join(root, ".github/workflows/rust.yml"), "utf8");
    expect(
      rust.split("rust-postgres-${{ runner.arch }}-${{ github.sha }}-${{ github.run_attempt }}"),
    ).toHaveLength(3);
    expect(
      rust.split("rust-helper-${{ runner.arch }}-${{ github.sha }}-${{ github.run_attempt }}"),
    ).toHaveLength(3);
    for (const workflow of GATE_WORKFLOWS) {
      const doc = loadWorkflow(workflow);
      const id = `${workflow}-ci-gate`;
      const gate = doc.jobs[id] as { name?: string; if?: string; needs?: string[] };
      expect(gate.name, workflow).toBe(id);
      expect(gate.if, workflow).toBe("always()");
      expect(doc.on.merge_group, workflow).toEqual({ types: ["checks_requested"] });
      expect(doc.on.pull_request, workflow).toBeNull();
      expect(doc.on.push, workflow).toEqual({ branches: ["main"] });
      for (const [trigger, spec] of Object.entries(doc.on)) {
        if (spec && typeof spec === "object") {
          for (const key of ["paths", "paths-ignore"]) {
            expect(key in spec, `${workflow} on.${trigger}.${key}`).toBe(false);
          }
        }
      }
      expect(new Set(gate.needs), workflow).toEqual(
        new Set(
          Object.keys(doc.jobs)
            .filter((job) => job !== id && job !== "ci-plan")
            .concat("ci-plan"),
        ),
      );
    }
  });

  test(
    "verify-workflows passes and rejects event-specific gate names or filtered merge_group",
    async () => {
      const baseline = tempDir("verify");
      writeRegistryTree(baseline);
      const ok = await run(
        ["python3", "-B", planner(root), "verify-workflows", "--repo-root", baseline],
        root,
        cleanEnv(),
      );
      expect(ok.code, ok.stderr).toBe(0);
      const cases = GATE_WORKFLOWS.flatMap((workflow) =>
        (["event-name", "drop-merge-group", "filter-merge-group"] as const).map((mutation) => ({
          workflow,
          mutation,
        })),
      );
      await pool(cases, async ({ workflow, mutation }) => {
        const dir = tempDir("verify");
        writeRegistryTree(dir);
        const path = join(dir, ".github/workflows", `${workflow}.yml`);
        const text = readFileSync(path, "utf8");
        let changed: string;
        let needle: string;
        if (mutation === "event-name") {
          changed = replaceOnce(
            text,
            `    name: ${workflow}-ci-gate\n`,
            "    name: ${{ github.event_name }}-ci-gate\n",
          );
          needle = `${workflow}-ci-gate name must stay ${workflow}-ci-gate`;
        } else if (mutation === "drop-merge-group") {
          changed = replaceOnce(text, "  merge_group:\n    types: [checks_requested]\n", "");
          needle = "merge_group trigger is required";
        } else {
          changed = replaceOnce(text, "types: [checks_requested]", "types: [destroyed]");
          needle = "merge_group must request checks_requested";
        }
        writeFileSync(path, changed);
        const r = await run(
          ["python3", "-B", planner(root), "verify-workflows", "--repo-root", dir],
          root,
          cleanEnv(),
        );
        expect(r.code, `${workflow} ${mutation}`).not.toBe(0);
        expect(r.stderr, `${workflow} ${mutation}`).toContain(needle);
      });
    },
    TIMEOUT,
  );
});

// =========================================================================
// MarkdownOnlyLaneTest and path broaden additions

const CONTENT_READ_MD = [
  "apps/web/NOTICE.md",
  "packages/editor/src/fonts/README.md",
  "scripts/release-notes-template.md",
  "infra/rust/compose.user.INSTALL.md",
  "scripts/testdata/release/compose.user.INSTALL.md",
];
const FIXTURE_MD = [
  "vendor/markdown/x.md",
  "vendor/markdown/nested/oracle.md",
  "compat/fixtures/markdown-oracle/x.md",
  "compat/fixtures/x.md",
  "tests/fixtures/x.md",
  "crates/collab-engine/fixtures/x.md",
  "crates/collab-engine/fixtures/nested/x.md",
  "scripts/fixtures/x.md",
  "scripts/fixtures/web-e2e/note.md",
];
const ORDINARY_MD = [
  "README.md",
  "NOTES.md",
  "docs/rewrite.md",
  "docs/other.md",
  "scripts/x.md",
  "scripts/sub/x.md",
  "tools/x.md",
  "tools/ci/x.md",
];

/** Markdown paths format-web.sh hands to prettier, minus .prettierignore. */
function derivedPrettierMarkdown(
  script: string,
  ignoreText: string,
): { targets: string[]; paths: string[] } {
  const marker = "set -- ";
  const start = script.indexOf(marker);
  const end = script.indexOf('"$@"', start);
  if (start < 0 || end < 0) throw new Error("format-web.sh default target list not found");
  const targets = script
    .slice(start + marker.length, end)
    .replace(/\\\n/g, " ")
    .split(/\s+/)
    .filter(Boolean)
    .map((token) => token.replace(/^'(.*)'$/, "$1"));
  const ignore = ignoreText
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"));
  const ignored = (path: string): boolean =>
    ignore.some((pattern) => {
      const nested = /^\*\*\/([^*/]+)\/\*\*$/.exec(pattern);
      if (nested) return path.split("/").includes(nested[1] as string);
      if (/[*?[!]/.test(pattern)) throw new Error(`unsupported .prettierignore pattern ${pattern}`);
      return path === pattern || path.startsWith(`${pattern.replace(/\/$/, "")}/`);
    });
  const candidates: string[] = [];
  for (const target of targets) {
    if (/[*?[]/.test(target)) {
      if (target.endsWith(".md"))
        candidates.push(target.replace(/\*\*\//g, "nested/").replace(/\*/g, "file"));
      continue;
    }
    const name = target.split("/").pop() ?? "";
    if (name.includes(".")) {
      if (target.endsWith(".md")) candidates.push(target);
      continue;
    }
    candidates.push(`${target}/prettier-lane.md`);
    const tracked = git(root, "ls-files", "--", `${target}/*.md`);
    if (tracked) candidates.push(...tracked.split("\n"));
  }
  return { targets, paths: [...new Set(candidates)].filter((path) => !ignored(path)) };
}

describe("markdown-only lane", () => {
  // PlanSelectionTest.test_tools_prefix_is_full_path_broaden / test_xtask_src_prefix_is_full_path_broaden
  // MarkdownOnlyLaneTest.test_fixture_and_content_read_markdown_stay_full
  test(
    "fixture, content-read markdown and tools/xtask code stay full",
    async () => {
      const paths = [
        ...FIXTURE_MD,
        ...CONTENT_READ_MD,
        "tools/selected-backend-ci/runtime.ts",
        "xtask/src/main.rs",
      ];
      const runs = await planPaths(paths, ["web"]);
      for (const path of paths) {
        const plan = expectPlan(runs[path]?.web as PlanRun, path);
        expect(plan.mode, path).toBe("full");
        expect(plan.reason_code, path).toBe("FULL_PATH_BROADEN");
        expectFullSelection("web", plan, path);
      }
    },
    TIMEOUT,
  );

  // Exact-set part of MarkdownOnlyLaneTest.test_fixture_and_content_read_markdown_stay_full:
  // the planner's content-read Markdown set is exactly CONTENT_READ_MD, defined once.
  test("content-read markdown set is exactly the probed paths", () => {
    const source = readFileSync(planner(root), "utf8");
    expect(source.match(/^\s*_CONTENT_READ_MARKDOWN\b\s*(?::[^=\n]*)?[|&^-]?=/gm)).toHaveLength(1);
    const head = "\n_CONTENT_READ_MARKDOWN: frozenset[str] = frozenset(\n    {\n";
    const start = source.indexOf(head);
    expect(start).toBeGreaterThan(0);
    const end = source.indexOf("\n    }\n)\n", start + head.length);
    expect(end).toBeGreaterThan(start);
    const lines = source.slice(start + head.length, end).split("\n");
    const members = lines.map((line) => {
      const m = /^ {8}"([^"\\]+)",$/.exec(line);
      expect(m, line).not.toBeNull();
      return (m as RegExpExecArray)[1] as string;
    });
    expect([...members].sort()).toEqual([...CONTENT_READ_MD].sort());
    expect(new Set(members).size).toBe(members.length);
  });

  // MarkdownOnlyLaneTest.test_ordinary_markdown_is_docs_narrow_on_every_gate
  test(
    "ordinary markdown is docs-narrow on every gate",
    async () => {
      const runs = await planPaths(ORDINARY_MD, GATE_WORKFLOWS);
      const checks = ORDINARY_MD.flatMap((path) =>
        GATE_WORKFLOWS.map((workflow) => ({ path, workflow })),
      );
      await pool(checks, async ({ path, workflow }) => {
        const r = runs[path]?.[workflow] as PlanRun;
        const context = `${path} ${workflow}`;
        const plan = expectPlan(r, context);
        expect(plan.mode, context).toBe("narrow");
        expect(plan.reason_code, context).toBe("NARROW_DOCS");
        expect(plan.plan_ok, context).toBe(true);
        expect(
          Object.values(plan.jobs).some((meta) => meta.selected),
          context,
        ).toBe(false);
        const gate = await runGate(
          workflow,
          needsJson(plan, r.outputs, honestResults(plan)),
          plan.tested_sha as string,
          "pull_request",
        );
        expect(gate.code, `${context}: ${gate.stderr}`).toBe(0);
      });
    },
    TIMEOUT,
  );

  // MarkdownOnlyLaneTest.test_markdown_with_code_or_loaded_markdown_is_full
  test(
    "markdown with code or loaded markdown is full",
    async () => {
      const sets: [string, string[]][] = [
        ["docs+code", ["docs/other.md", "src/lib.rs"]],
        ["notes+notice", ["NOTES.md", "apps/web/NOTICE.md"]],
        ["tools+fixture", ["tools/ci/x.md", "compat/fixtures/markdown-oracle/x.md"]],
        ["web-e2e-script", ["scripts/run-web-e2e.sh"]],
        ["vendor-markdown-code", ["vendor/markdown/src/parser.rs"]],
        ["collab-engine-code", ["crates/collab-engine/src/lib.rs"]],
      ];
      const o = origin();
      const prepared = sets.map(([label, paths]) => {
        const head = o.branch(Object.fromEntries(paths.map((path) => [path, `change ${path}\n`])));
        return { label, head, ...o.mergeClone(head) };
      });
      await pool(prepared, async ({ label, head, work, tested }) => {
        const plan = expectPlan(
          await runPlan(work, "web", "pull_request", prPayload(o.base, head), tested),
          label,
        );
        expect(plan.mode, label).toBe("full");
        expect(plan.reason_code, label).toBe("FULL_PATH_BROADEN");
      });
    },
    TIMEOUT,
  );

  // MarkdownOnlyLaneTest.test_prettier_markdown_selects_web_format_check
  test(
    "prettier-checked markdown selects the web format check",
    async () => {
      const derived = derivedPrettierMarkdown(
        readFileSync(join(root, "scripts/format-web.sh"), "utf8"),
        readFileSync(join(root, ".prettierignore"), "utf8"),
      );
      expect(derived.paths.length).toBeGreaterThan(0);
      expect(derived.paths).toContain("scripts/WEB_LINT.md");
      expect(derived.paths).toContain("packages/i18n/NOTICE.md");
      expect(derived.paths).not.toContain("apps/web/NOTICE.md");
      const runs = await planPaths(derived.paths, GATE_WORKFLOWS);
      for (const path of derived.paths) {
        for (const workflow of GATE_WORKFLOWS) {
          const plan = expectPlan(runs[path]?.[workflow] as PlanRun, `${path} ${workflow}`);
          expect(plan.reason_code, `${path} ${workflow}`).not.toBe("NARROW_DOCS");
        }
        expect(runs[path]?.web?.plan?.jobs["web-static"]?.selected, path).toBe(true);
      }
    },
    TIMEOUT,
  );

  // MarkdownOnlyLaneTest.test_added_format_web_directory_fails_until_planner_covers_it
  test(
    "a new format-web directory is docs until the planner reads it",
    async () => {
      const fake = "scripts/fake-prettier-dir";
      const script = readFileSync(join(root, "scripts/format-web.sh"), "utf8");
      expect(script).not.toContain(fake);
      const mutate = (s: string) => replaceOnce(s, "set -- ", `set -- ${fake} `);
      const md = `${fake}/prettier-lane.md`;
      const derived = derivedPrettierMarkdown(
        mutate(script),
        readFileSync(join(root, ".prettierignore"), "utf8"),
      );
      expect(derived.targets).toContain(fake);
      expect(derived.paths).toContain(md);
      // The repository planner has not seen the new target: the derived check
      // would flag this path as docs-only.
      const stale = await planPaths([md], ["web"]);
      expect(expectPlan(stale[md]?.web as PlanRun, "stale").reason_code).toBe("NARROW_DOCS");
      // A planner that reads the edited format-web.sh moves it to the web lane.
      const covered = await planPaths([md], ["web"], plannerCopy({ formatWeb: mutate }));
      const plan = expectPlan(covered[md]?.web as PlanRun, "covered");
      expect(plan.reason_code).not.toBe("NARROW_DOCS");
      expect(plan.jobs["web-static"]?.selected).toBe(true);
    },
    TIMEOUT,
  );

  // MarkdownOnlyLaneTest.test_removing_markdown_lane_entries_drops_their_lane
  test(
    "removing a markdown lane entry drops its lane",
    async () => {
      const cases: [string, string, string][] = [
        [
          '    if path.startswith(".github/"):\n        return "broaden"\n',
          ".github/workflows/note.md",
          "broaden",
        ],
        ["_PLANNER_CONTRACT_MARKDOWN, ", ".agents/skills/fvoci-fast-verify/SKILL.md", "broaden"],
        [
          ", _PARENT_BROADEN_MARKDOWN",
          ".agents/skills/fvoci-standard-implementations/references/candidates.md",
          "broaden",
        ],
        [
          '    if _is_prettier_checked_markdown(path):\n        return "frontend_web_install"\n',
          "scripts/WEB_LINT.md",
          "web",
        ],
        [
          '    if _is_prettier_checked_markdown(path):\n        return "frontend_web_install"\n',
          "packages/i18n/NOTICE.md",
          "web",
        ],
      ];
      const source = readFileSync(planner(root), "utf8");
      const lane = source.slice(
        source.indexOf("\ndef _markdown_lane("),
        source.indexOf("\ndef validate_sha("),
      );
      for (const [needle, path, expected] of cases) {
        expect(lane, path).toContain(needle);
        const real = await planPaths([path], ["web"]);
        const realPlan = expectPlan(real[path]?.web as PlanRun, `${path} real`);
        expect(realPlan.reason_code, path).not.toBe("NARROW_DOCS");
        if (expected === "broaden") expect(realPlan.reason_code, path).toBe("FULL_PATH_BROADEN");
        else expect(realPlan.jobs["web-static"]?.selected, path).toBe(true);
        const copy = plannerCopy({ source: (s) => s.replace(lane, replaceOnce(lane, needle, "")) });
        const dropped = await planPaths([path], ["web"], copy);
        expect(expectPlan(dropped[path]?.web as PlanRun, `${path} dropped`).reason_code, path).toBe(
          "NARROW_DOCS",
        );
      }
    },
    TIMEOUT,
  );
});
