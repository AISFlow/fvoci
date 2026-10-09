// One named test for every method in main 93534e28 scripts/test_ci_selection.py.
// Expectations and fixtures are copied from that source, never from TS output.
import { afterAll, afterEach, expect, spyOn, test } from "bun:test";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  readFileSync,
  cpSync,
  rmSync,
  existsSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import * as S from "../ci_selection";
import type { Mapping, Plan, PlanInputs, GitResult } from "../ci_selection";

const ROOT = resolve(import.meta.dir, "../..");
const SOURCE = join(ROOT, "scripts/ci_selection.py"),
  TARGET = join(ROOT, "scripts/ci_selection.ts");
const temporary: string[] = [];
const tmp = () => {
  const path = mkdtempSync(join(tmpdir(), "fvoci-selection-ts-"));
  temporary.push(path);
  return path;
};
afterEach(() => {
  for (const path of temporary.splice(0)) rmSync(path, { recursive: true, force: true });
});
const read = (path: string) => readFileSync(path, "utf8");
function record(value: unknown): Mapping {
  if (!S.isMapping(value)) throw new Error("fixture requires mapping");
  return value;
}
const sub = (value: Mapping, key: string) => record(value[key]);
const json = (path: string): unknown => JSON.parse(read(path)) as unknown;
const write = (repo: string, rel: string, content = "x\n") => {
  const path = join(repo, rel);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
};
function git(repo: string, ...args: string[]) {
  const p = Bun.spawnSync(["git", ...args], { cwd: repo, stdout: "pipe", stderr: "pipe" });
  expect(p.exitCode, `git ${args[0]}: ${p.stderr.toString()}`).toBe(0);
  return p.stdout.toString().trim();
}
const sha = (repo: string, ref = "HEAD") => git(repo, "rev-parse", ref);
function configure(repo: string) {
  git(repo, "config", "user.email", "ci@test");
  git(repo, "config", "user.name", "ci");
  git(repo, "config", "commit.gpgsign", "false");
}
function copyWorkflows(root: string) {
  cpSync(join(ROOT, ".github/workflows"), join(root, ".github/workflows"), { recursive: true });
}
function stub(root: string, extra: string[] = []) {
  for (const rel of [S.RUST_COLLAB_CI_SCRIPT, S.RUST_CAPACITY_PROBE_SCRIPT])
    write(root, rel, read(join(ROOT, rel)));
  const lines = ["[features]", "db-tests = []", ""];
  for (const name of ["db_integration", ...extra])
    lines.push(
      "[[test]]",
      `name = "${name}"`,
      `path = "tests/${name}.rs"`,
      'required-features = ["db-tests"]',
      "",
    );
  write(root, "Cargo.toml", lines.join("\n"));
}
class GitFixture {
  repo = tmp();
  constructor() {
    git(this.repo, "init", "-b", "main");
    configure(this.repo);
  }
  commit(rel: string, content = "x\n") {
    write(this.repo, rel, content);
    git(this.repo, "add", rel);
    git(this.repo, "commit", "-m", `add ${rel}`);
    return sha(this.repo);
  }
  rename(old: string, next: string) {
    git(this.repo, "mv", old, next);
    git(this.repo, "commit", "-m", `rename ${old}`);
    return sha(this.repo);
  }
  delete(rel: string) {
    git(this.repo, "rm", rel);
    git(this.repo, "commit", "-m", `delete ${rel}`);
    return sha(this.repo);
  }
}
interface CliResult {
  returncode: number;
  stdout: Buffer;
  stderr: Buffer;
}
const cliCounts: Record<string, { zero: number; nonzero: number }> = {};
afterAll(() => {
  console.log("CLI parity invocations: " + JSON.stringify(cliCounts));
});
// Every public invocation uses identical event files, repo, arguments and environment.
// Standard JSON comparison covers artifacts; stdout and exit are byte/exact comparisons.
function runCli(args: string[], env: Record<string, string> = {}, cwd = ROOT): CliResult {
  const opts = {
    cwd,
    env: { ...process.env, ...env },
    stdout: "pipe" as const,
    stderr: "pipe" as const,
  };
  const artifacts = ["--output-plan", "--github-output"].flatMap((key) => {
    const i = args.indexOf(key);
    return i >= 0 && args[i + 1] ? [args[i + 1] ?? ""] : [];
  });
  const py = Bun.spawnSync(["python3", SOURCE, ...args], opts);
  const saved = artifacts.map((path) => (existsSync(path) ? readFileSync(path) : null));
  for (const path of artifacts) if (existsSync(path)) rmSync(path);
  const ts = Bun.spawnSync([process.execPath, TARGET, ...args], opts);
  expect(
    ts.exitCode,
    `CLI exit parity: ${args[0]}\n${ts.stderr.toString()}\n${py.stderr.toString()}`,
  ).toBe(py.exitCode);
  expect(ts.stdout, `CLI stdout bytes: ${args[0]}`).toEqual(py.stdout);
  for (const [i, path] of artifacts.entries()) {
    const actual = existsSync(path) ? readFileSync(path) : null;
    expect(actual, `CLI artifact bytes: ${args[0]} ${artifacts.indexOf(path)}`).toEqual(
      saved[i] ?? null,
    );
  }
  const command = args[0] ?? "";
  const count = cliCounts[command] ?? { zero: 0, nonzero: 0 };
  if (ts.exitCode === 0) count.zero++;
  else count.nonzero++;
  cliCounts[command] = count;
  return { returncode: ts.exitCode, stdout: ts.stdout, stderr: ts.stderr };
}
class PrFixture {
  origin = tmp();
  work = tmp();
  base: string;
  constructor() {
    git(this.origin, "init", "-b", "main");
    configure(this.origin);
    git(this.origin, "config", "uploadpack.allowReachableSHA1InWant", "true");
    copyWorkflows(this.origin);
    stub(this.origin);
    write(this.origin, "README.md", "base docs\n");
    git(this.origin, "add", ".");
    git(this.origin, "commit", "-m", "base");
    this.base = sha(this.origin);
  }
  commit(branch: string, rel: string, content: string) {
    git(this.origin, "checkout", "-B", branch, "main");
    write(this.origin, rel, content);
    git(this.origin, "add", rel);
    git(this.origin, "commit", "-m", `${branch} ${rel}`);
    const head = sha(this.origin);
    git(this.origin, "checkout", "main");
    return head;
  }
  clone() {
    git(this.origin, "clone", this.origin, this.work);
    configure(this.work);
  }
  merge(base: string, head: string) {
    git(this.work, "checkout", "-B", "tested", base);
    git(this.work, "merge", "--no-ff", "-m", "github merge", head);
    return sha(this.work);
  }
  event(base: string, head: string) {
    const path = join(this.work, "event.json");
    writeFileSync(
      path,
      JSON.stringify({ pull_request: { base: { sha: base }, head: { sha: head } } }),
    );
    return path;
  }
  plan(tested: string, base: string, head: string) {
    const out = join(this.work, "plan.json"),
      event = this.event(base, head);
    const p = runCli(
      [
        "plan",
        "--workflow",
        "web",
        "--repo-root",
        this.work,
        "--event-json",
        event,
        "--output-plan",
        out,
      ],
      { GITHUB_EVENT_NAME: "pull_request", GITHUB_SHA: tested },
      this.work,
    );
    expect(p.returncode, p.stderr.toString()).toBe(0);
    return record(json(out));
  }
}
const selected = (plan: Mapping, job: string) => sub(sub(plan, "jobs"), job).selected;
const optIn = (workflow: string, job: string) => job in (S.OPT_IN_JOBS[workflow] ?? {});
function full(workflow: string, plan: Plan) {
  for (const [job, meta] of Object.entries(plan.jobs))
    expect(meta.selected).toBe(!optIn(workflow, job));
}
const plan = (paths: string[] | null, options: Partial<PlanInputs> = {}) =>
  S.buildPlan({
    workflow: "web",
    event_name: "pull_request",
    base_sha: "a".repeat(40),
    head_sha: "b".repeat(40),
    merge_base_sha: "c".repeat(40),
    tested_sha: "b".repeat(40),
    paths,
    ...options,
  });
const all = (paths: string[] | null, options: Partial<PlanInputs> = {}) =>
  Object.fromEntries(
    Object.keys(S.WORKFLOW_JOBS).map((workflow) => [
      workflow,
      plan(paths, { ...options, workflow }),
    ]),
  );
function selectedWorkflows(paths: string[], chosen: Set<string>) {
  for (const [workflow, p] of Object.entries(all(paths))) {
    expect(p.mode).toBe("narrow");
    for (const [job, meta] of Object.entries(p.jobs))
      expect(meta.selected).toBe(chosen.has(workflow) && !optIn(workflow, job));
  }
}
function withEnv<T>(env: Record<string, string>, fn: () => T): T {
  const old = Object.fromEntries(Object.keys(env).map((k) => [k, process.env[k]]));
  Object.assign(process.env, env);
  try {
    return fn();
  } finally {
    for (const [k, v] of Object.entries(old))
      if (v === undefined) delete process.env[k];
      else process.env[k] = v;
  }
}
function resolveInputs(fx: PrFixture, base: string, head: string, tested: string) {
  return withEnv({ GITHUB_SHA: tested }, () =>
    S.resolveSelectionInputs(
      fx.work,
      { pull_request: { base: { sha: base }, head: { sha: head } } },
      "pull_request",
    ),
  );
}
const bytes = (text: string) => new TextEncoder().encode(text);
function fullEventCli(name: string): void {
  const fx = new GitFixture();
  copyWorkflows(fx.repo);
  stub(fx.repo);
  const base = fx.commit("docs/rewrite.md", "a\n"),
    head = fx.commit("docs/rewrite.md", "b\n");
  const event =
    name === "push"
      ? { before: base, after: head }
      : name === "merge_group"
        ? { merge_group: { base_sha: base, head_sha: head } }
        : {};
  const eventPath = join(fx.repo, "event.json"),
    out = join(fx.repo, "plan.json");
  writeFileSync(eventPath, JSON.stringify(event));
  const p = runCli(
    [
      "plan",
      "--workflow",
      "web",
      "--repo-root",
      fx.repo,
      "--event-json",
      eventPath,
      "--output-plan",
      out,
    ],
    { GITHUB_EVENT_NAME: name, GITHUB_SHA: head },
  );
  expect(p.returncode, p.stderr.toString()).toBe(0);
  const result = record(json(out));
  expect(result.mode).toBe("full");
  expect(result.reason_code).toBe(`FULL_EVENT_${name.toUpperCase()}`);
  expect(result.plan_ok).toBe(true);
  expect(selected(result, "web-checks")).toBe(true);
}

test("ClassifyPathsTest.test_explicit_docs_only", () => {
  expect(S.classifyPath("docs/rewrite.md")).toBe("docs");
  expect(S.classifyPath("docs/other.md")).toBe("broaden");
});
test("ClassifyPathsTest.test_frontend_src_narrow", () => {
  expect(S.classifyPath("apps/web/src/foo.ts")).toBe("frontend_web_install");
});
test("ClassifyPathsTest.test_generated_broadens", () => {
  expect(S.classifyPath("apps/web/src/generated/api.ts")).toBe("broaden");
});
test("ClassifyPathsTest.test_e2e_specs_select_web", () => {
  expect(S.classifyPath("apps/web/e2e/foo.spec.ts")).toBe("web_tests");
});
test("ClassifyPathsTest.test_packages_broaden", () => {
  expect(S.classifyPath("packages/editor/x.ts")).toBe("broaden");
});
test("ClassifyPathsTest.test_native_crate_broadens", () => {
  expect(S.classifyPath("crates/collab-engine/src/x.rs")).toBe("broaden");
});
test("ClassifyPathsTest.test_compat_fixtures_broaden", () => {
  expect(S.classifyPath("compat/fixtures/x")).toBe("broaden");
});
test("DiffParseTest.test_valid_rename_delete", () => {
  expect(
    S.parseNameStatusZ(bytes("A\0src/new.ts\0D\0src/old.ts\0R100\0old-name\0new-name\0")),
  ).toEqual([["src/new.ts", "src/old.ts", "old-name", "new-name"], null]);
});
test("DiffParseTest.test_truncated_rename_fails", () => {
  expect(S.parseNameStatusZ(bytes("R100\0only-old\0"))).toEqual([[], "DIFF_TRUNCATED_RENAME"]);
});
test("DiffParseTest.test_missing_trailing_nul_fails", () => {
  expect(S.parseNameStatusZ(bytes("A\0file.ts"))[1]).toBe("DIFF_TRUNCATED");
});
test("PlanSelectionTest.test_frontend_selects_web_and_install", () => {
  for (const [workflow, job, expected] of [
    ["web", "web-checks", true],
    ["install", "install-smoke", true],
    ["rust", "fast", false],
  ] as const) {
    const p = plan(["apps/web/src/x.ts"], { workflow });
    expect(p.jobs[job]?.selected).toBe(expected);
    expect(Object.keys(p.jobs[job] ?? {})).toEqual(["selected"]);
  }
});
test("PlanSelectionTest.test_docs_skips_product_jobs", () => {
  const p = plan(["docs/rewrite.md"]);
  expect(p.mode).toBe("narrow");
  expect(p.jobs["web-checks"]?.selected).toBe(false);
});
test("PlanSelectionTest.test_crate_change_is_full", () => {
  expect(S.decideFromPaths(["crates/document-extract/src/lib.rs"]).mode).toBe("full");
});
test("PlanSelectionTest.test_main_push_is_full", () => {
  const p = plan(["docs/rewrite.md"], { event_name: "push", merge_base_sha: null });
  expect(p.mode).toBe("full");
  expect(p.reason_code).toBe("FULL_EVENT_PUSH");
  expect(p.plan_ok).toBe(true);
  expect(p.jobs["web-checks"]?.selected).toBe(true);
  fullEventCli("push");
});
test("PlanSelectionTest.test_manual_dispatch_is_full", () => {
  const p = plan(["docs/rewrite.md"], {
    event_name: "workflow_dispatch",
    base_sha: null,
    head_sha: null,
    merge_base_sha: null,
  });
  expect(p.mode).toBe("full");
  expect(p.reason_code).toBe("FULL_EVENT_WORKFLOW_DISPATCH");
  expect(p.plan_ok).toBe(true);
  fullEventCli("workflow_dispatch");
});
test("PlanSelectionTest.test_merge_group_is_full", () => {
  const p = plan(["docs/rewrite.md"], { event_name: "merge_group", merge_base_sha: null });
  expect(p.mode).toBe("full");
  expect(p.reason_code).toBe("FULL_EVENT_MERGE_GROUP");
  expect(p.plan_ok).toBe(true);
  fullEventCli("merge_group");
});
test("PlanSelectionTest.test_parent_mismatch_cannot_narrow", () => {
  const p = plan(["docs/rewrite.md"], {
    merge_base_sha: null,
    tested_sha: "c".repeat(40),
    force_full_reason: "FULL_PR_MERGE_PARENTS_MISMATCH",
  });
  expect(p.mode).toBe("full");
  expect(p.plan_ok).toBe(true);
  expect(p.jobs["web-checks"]?.selected).toBe(true);
});
test("GitIntegrationTest.test_multi_commit_and_merge_base", () => {
  const fx = new GitFixture(),
    base = fx.commit("docs/rewrite.md", "a\n"),
    head = fx.commit("docs/rewrite.md", "b\n");
  const [paths, e, mb] = S.diffPathsForPr(fx.repo, base, head);
  expect(e).toBeNull();
  expect(mb).toBeTruthy();
  expect(paths).toContain("docs/rewrite.md");
});
test("GitIntegrationTest.test_rename_in_diff", () => {
  const fx = new GitFixture();
  fx.commit("apps/web/src/a.ts");
  const mid = sha(fx.repo);
  fx.rename("apps/web/src/a.ts", "apps/web/src/b.ts");
  const [paths, e] = S.diffPathsForPr(fx.repo, mid, sha(fx.repo));
  expect(e).toBeNull();
  expect(paths?.some((p) => p.includes("b.ts"))).toBe(true);
});
test("GitIntegrationTest.test_base_advance_second_pr_commit", () => {
  const fx = new GitFixture(),
    base = fx.commit("README.md");
  fx.commit("apps/web/src/z.ts");
  const [paths, e] = S.diffPathsForPr(fx.repo, base, sha(fx.repo));
  expect(e).toBeNull();
  expect(paths).toContain("apps/web/src/z.ts");
});
test("GitIntegrationTest.test_multicommit_rename_and_delete", () => {
  const fx = new GitFixture();
  fx.commit("keep.md", "keep\n");
  fx.commit("apps/web/src/old.ts", "old\n");
  const base = fx.commit("gone.ts", "gone\n");
  fx.rename("apps/web/src/old.ts", "apps/web/src/new.ts");
  fx.delete("gone.ts");
  const [paths, e] = S.diffPathsForPr(fx.repo, base, sha(fx.repo));
  expect(e).toBeNull();
  for (const p of ["apps/web/src/old.ts", "apps/web/src/new.ts", "gone.ts"])
    expect(paths).toContain(p);
});
test("PrCheckoutBindingTest.test_valid_merge_tree_can_narrow_docs", () => {
  const fx = new PrFixture(),
    head = fx.commit("pr", "README.md", "docs only\n");
  fx.clone();
  const p = fx.plan(fx.merge(fx.base, head), fx.base, head);
  expect(p.mode).toBe("narrow");
  expect(p.reason_code).toBe("NARROW_DOCS");
  expect(selected(p, "web-checks")).toBe(false);
});
test("PrCheckoutBindingTest.test_valid_merge_frontend_selects_web", () => {
  const fx = new PrFixture(),
    head = fx.commit("pr", "apps/web/src/x.ts", "export {}\n");
  fx.clone();
  const p = fx.plan(fx.merge(fx.base, head), fx.base, head);
  expect(p.mode).toBe("narrow");
  expect(p.reason_code).toBe("NARROW_FRONTEND_WEB_INSTALL");
  expect(selected(p, "web-checks")).toBe(true);
});
test("PrCheckoutBindingTest.test_unrelated_code_merge_vs_docs_event_cannot_narrow", () => {
  const fx = new PrFixture(),
    docs = fx.commit("docs-pr", "README.md", "docs only\n"),
    code = fx.commit("code-pr", "src/lib.rs", "fn x() {}\n");
  fx.clone();
  const p = fx.plan(fx.merge(fx.base, code), fx.base, docs);
  expect(p.mode).toBe("full");
  expect(p.reason_code).toBe("FULL_PR_MERGE_PARENTS_MISMATCH");
  expect(selected(p, "web-checks")).toBe(true);
});
test("PrCheckoutBindingTest.test_base_advance_with_exact_head_can_narrow", () => {
  const fx = new PrFixture(),
    head = fx.commit("docs-pr", "README.md", "docs only\n");
  write(fx.origin, "src/lib.rs", "fn advanced() {}\n");
  git(fx.origin, "add", "src/lib.rs");
  git(fx.origin, "commit", "-m", "advance base");
  const advanced = sha(fx.origin);
  fx.clone();
  const tested = fx.merge(advanced, head),
    p = fx.plan(tested, fx.base, head);
  expect(p.mode).toBe("narrow");
  expect(p.reason_code).toBe("NARROW_DOCS");
  expect(p.base_sha).toBe(fx.base);
  expect(p.tested_sha).toBe(tested);
});
test("PrCheckoutBindingTest.test_direct_head_checkout_cannot_narrow", () => {
  const fx = new PrFixture(),
    head = fx.commit("docs-pr", "README.md", "docs only\n");
  fx.clone();
  git(fx.work, "checkout", "--detach", head);
  const p = fx.plan(head, fx.base, head);
  expect(p.mode).toBe("full");
  expect(p.reason_code).toBe("FULL_PR_CHECKOUT_NOT_MERGE");
});

interface GatePlan extends Mapping {
  jobs: Record<string, Mapping>;
}
function gatePlan(workflow: string, selected: Record<string, boolean>, ok = true): GatePlan {
  return {
    version: S.PLAN_VERSION,
    workflow,
    mode: "narrow",
    reason_code: "NARROW_DOCS",
    plan_ok: ok,
    tested_sha: "a".repeat(40),
    jobs: Object.fromEntries(
      (S.WORKFLOW_JOBS[workflow] ?? []).map((job) => [job, { selected: selected[job] ?? false }]),
    ),
  };
}
interface NeedsOptions {
  plan_result?: string;
  plan_outputs?: Mapping;
  extra?: Mapping;
  omit_jobs?: ReadonlySet<string>;
  job_entries?: Mapping;
}
function needs(
  plan: unknown,
  workflow: string,
  results: Record<string, string> = {},
  opts: NeedsOptions = {},
): string {
  const output: Mapping = {
    "ci-plan": {
      result: opts.plan_result ?? "success",
      outputs: opts.plan_outputs ?? { plan_json: JSON.stringify(plan) },
    },
  };
  for (const job of S.WORKFLOW_JOBS[workflow] ?? []) {
    if (opts.omit_jobs?.has(job)) continue;
    output[job] =
      opts.job_entries && job in opts.job_entries
        ? opts.job_entries[job]
        : { result: results[job] ?? "skipped", outputs: {} };
  }
  Object.assign(output, opts.extra);
  return JSON.stringify(output);
}
interface GateOptions extends NeedsOptions {
  tested?: string;
  needs_json?: string;
  event_name?: string;
  event?: unknown;
}
function gate(
  plan: unknown,
  workflow: string,
  results: Record<string, string> = {},
  opts: GateOptions = {},
): number {
  const chosen: Record<string, string> = {};
  for (const [job, name] of Object.entries(S.OPT_IN_JOBS[workflow] ?? {}))
    if (
      S.isMapping(plan) &&
      S.isMapping(plan.jobs) &&
      S.isMapping(plan.jobs[job]) &&
      plan.jobs[job].selected === true
    )
      chosen[name] = "true";
  const name =
    opts.event_name ?? (Object.keys(chosen).length ? "workflow_dispatch" : "pull_request");
  const event =
    opts.event_name === undefined
      ? Object.keys(chosen).length
        ? { inputs: chosen }
        : {}
      : opts.event;
  const path = join(tmp(), "event.json");
  writeFileSync(path, typeof event === "string" ? event : JSON.stringify(event));
  return runCli(
    [
      "gate",
      "--workflow",
      workflow,
      "--needs-json",
      opts.needs_json ?? needs(plan, workflow, results, opts),
      "--tested-sha",
      opts.tested ?? "a".repeat(40),
    ],
    { GITHUB_EVENT_NAME: name, GITHUB_EVENT_PATH: path },
  ).returncode;
}
const docsPlan = (selected = true) => {
  const p = gatePlan("documents", { "native-extraction": selected });
  p.mode = "full";
  return p;
};
test("GateSchemaTest.test_unselected_must_be_skipped", () => {
  expect(
    gate(gatePlan("web", { "web-checks": false }), "web", {
      "web-checks": "success",
      "workspace-browser-shard": "skipped",
      "collaboration-flow": "skipped",
    }),
  ).toBe(1);
});
test("GateSchemaTest.test_web_lint_job_failure_reaches_required_gate", () => {
  const p = gatePlan("web", { "web-static": true });
  expect(gate(p, "web", { "web-static": "failure" })).toBe(1);
  expect(gate(p, "web", { "web-static": "success" })).toBe(0);
});
test("GateSchemaTest.test_selected_missing_needs_key_rejected", () => {
  expect(gate(docsPlan(), "documents", {}, { omit_jobs: new Set(["native-extraction"]) })).toBe(1);
});
test("GateSchemaTest.test_malformed_needs_rejected", () => {
  expect(
    gate(
      gatePlan("documents", { "native-extraction": true }),
      "documents",
      {},
      { needs_json: "{not-json" },
    ),
  ).toBe(1);
});
test("GateSchemaTest.test_needs_list_rejected", () => {
  expect(
    gate(
      gatePlan("documents", { "native-extraction": true }),
      "documents",
      {},
      { needs_json: "[]" },
    ),
  ).toBe(1);
});
test("GateSchemaTest.test_missing_needs_json_rejected", () => {
  expect(
    runCli(["gate", "--workflow", "documents", "--tested-sha", "a".repeat(40), "--needs-json", ""])
      .returncode,
  ).toBe(1);
});
test("GateSchemaTest.test_missing_result_field_rejected", () => {
  expect(
    gate(docsPlan(), "documents", {}, { job_entries: { "native-extraction": { outputs: {} } } }),
  ).toBe(1);
});
test("GateSchemaTest.test_result_wrong_type_rejected", () => {
  expect(
    gate(
      docsPlan(),
      "documents",
      {},
      { job_entries: { "native-extraction": { result: 1, outputs: {} } } },
    ),
  ).toBe(1);
});
test("GateSchemaTest.test_plan_result_failure_rejected", () => {
  expect(
    gate(docsPlan(), "documents", { "native-extraction": "success" }, { plan_result: "failure" }),
  ).toBe(1);
});
test("GateSchemaTest.test_extra_unknown_job_rejected", () => {
  expect(
    gate(
      gatePlan("documents", { "native-extraction": false }),
      "documents",
      { "native-extraction": "skipped" },
      { extra: { mystery: { result: "success", outputs: {} } } },
    ),
  ).toBe(1);
});
test("GateSchemaTest.test_plan_not_ok_rejected", () => {
  expect(
    gate(gatePlan("web", { "web-checks": true }, false), "web", {
      "web-checks": "success",
      "workspace-browser-shard": "skipped",
      "collaboration-flow": "skipped",
    }),
  ).toBe(1);
});
test("GateSchemaTest.test_selected_failure_rejected", () => {
  expect(gate(docsPlan(), "documents", { "native-extraction": "failure" })).toBe(1);
});
test("GateSchemaTest.test_selected_cancelled_rejected", () => {
  expect(gate(docsPlan(), "documents", { "native-extraction": "cancelled" })).toBe(1);
});
test("GateSchemaTest.test_selected_skip_rejected", () => {
  expect(gate(docsPlan(), "documents", { "native-extraction": "skipped" })).toBe(1);
});
test("GateSchemaTest.test_selected_success_ok", () => {
  expect(gate(docsPlan(), "documents", { "native-extraction": "success" })).toBe(0);
});
test("GateSchemaTest.test_invalid_json_top_type_rejected", () => {
  expect(gate(["not", "an", "object"], "documents", { "native-extraction": "success" })).toBe(1);
});
test("GateSchemaTest.test_unknown_plan_keys_rejected", () => {
  const p = gatePlan("documents", { "native-extraction": false });
  p.extra = "nope";
  expect(gate(p, "documents", { "native-extraction": "skipped" })).toBe(1);
});
test("GateSchemaTest.test_unknown_job_keys_rejected", () => {
  const p = gatePlan("documents", { "native-extraction": false });
  sub(p.jobs, "native-extraction").reason_code = "NARROW_DOCS";
  expect(gate(p, "documents", { "native-extraction": "skipped" })).toBe(1);
});
test("GateSchemaTest.test_strict_bool_rejects_string_true", () => {
  const p = gatePlan("documents", { "native-extraction": false });
  sub(p.jobs, "native-extraction").selected = "true";
  expect(gate(p, "documents", { "native-extraction": "skipped" })).toBe(1);
});
test("GateSchemaTest.test_strict_bool_rejects_integer", () => {
  const p = gatePlan("documents", { "native-extraction": false });
  sub(p.jobs, "native-extraction").selected = 1;
  expect(gate(p, "documents", { "native-extraction": "success" })).toBe(1);
});
test("GateSchemaTest.test_invalid_plan_json_rejected", () => {
  expect(
    gate(
      docsPlan(),
      "documents",
      { "native-extraction": "success" },
      { plan_outputs: { plan_json: "{bad" } },
    ),
  ).toBe(1);
});
test("GateSchemaTest.test_tested_sha_mismatch_rejected", () => {
  expect(
    gate(docsPlan(), "documents", { "native-extraction": "success" }, { tested: "b".repeat(40) }),
  ).toBe(1);
});
function installPlan(
  name: string,
  event: unknown = {},
  paths: string[] | null = null,
  opts: Partial<PlanInputs> = {},
): Plan {
  const [chosen, e] = S.dispatchOptIns("install", name, event);
  return plan(paths, {
    workflow: "install",
    event_name: name,
    ...opts,
    fatal_error: opts.fatal_error ?? e,
    opt_in_inputs: chosen,
  });
}
test("OptInSelectionTest.test_ordinary_events_never_select_upgrade", () => {
  const cases: [string, string[]][] = [
    ["pull_request", ["apps/web/src/x.ts"]],
    ["pull_request", ["docs/rewrite.md"]],
    ["pull_request", ["src/main.rs"]],
    ["pull_request", ["scripts/upgrade-smoke.sh"]],
    ["push", ["src/main.rs"]],
    ["merge_group", ["src/main.rs"]],
  ];
  for (const [name, paths] of cases)
    for (const event of [{}, { inputs: { run_upgrade_smoke_arm: "true" } }])
      expect(installPlan(name, event, paths).jobs["upgrade-smoke-arm64"]?.selected).toBe(false);
  const p = installPlan("pull_request", {}, ["apps/web/src/x.ts"]);
  expect(p.jobs["install-smoke"]?.selected).toBe(true);
  expect(p.jobs["backup-restore-smoke"]?.selected).toBe(true);
});
test("OptInSelectionTest.test_manual_dispatch_off_by_default", () => {
  for (const event of [
    {},
    { inputs: null },
    { inputs: {} },
    { inputs: { run_upgrade_smoke_arm: "false" } },
    { inputs: { run_upgrade_smoke_arm: false } },
  ]) {
    const p = installPlan("workflow_dispatch", event);
    expect(p.plan_ok).toBe(true);
    expect(p.reason_code).toBe("FULL_EVENT_WORKFLOW_DISPATCH");
    expect(p.jobs["upgrade-smoke-arm64"]?.selected).toBe(false);
    expect(p.jobs["install-smoke"]?.selected).toBe(true);
    expect(p.jobs["backup-restore-smoke"]?.selected).toBe(true);
  }
});
test("OptInSelectionTest.test_manual_dispatch_opt_in_selects_upgrade", () => {
  for (const value of ["true", true]) {
    const p = installPlan("workflow_dispatch", { inputs: { run_upgrade_smoke_arm: value } });
    expect(p.plan_ok).toBe(true);
    expect(Object.fromEntries(Object.entries(p.jobs).map(([k, v]) => [k, v.selected]))).toEqual({
      "install-smoke": true,
      "backup-restore-smoke": true,
      "upgrade-smoke-arm64": true,
    });
  }
});
test("OptInSelectionTest.test_opt_in_input_ignored_by_other_workflows_and_fatal_plans", () => {
  const p = plan(null, {
    workflow: "install",
    event_name: "workflow_dispatch",
    base_sha: null,
    head_sha: null,
    merge_base_sha: null,
    fatal_error: "TESTED_SHA_MISMATCH",
    opt_in_inputs: new Set(["run_upgrade_smoke_arm"]),
  });
  expect(p.plan_ok).toBe(false);
  expect(p.jobs["upgrade-smoke-arm64"]?.selected).toBe(false);
  expect(
    S.dispatchOptIns("web", "workflow_dispatch", { inputs: { run_upgrade_smoke_arm: "true" } })[1],
  ).toBe("DISPATCH_INPUTS_UNKNOWN");
});
test("OptInSelectionTest.test_malformed_dispatch_inputs_fail_plan", () => {
  const cases: [unknown, string][] = [
    [[], "DISPATCH_EVENT_INVALID"],
    [{ inputs: "true" }, "DISPATCH_INPUTS_INVALID"],
    [{ inputs: { run_upgrade_smoke_arm: "TRUE" } }, "DISPATCH_INPUT_VALUE_INVALID"],
    [{ inputs: { run_upgrade_smoke_arm: 1 } }, "DISPATCH_INPUT_VALUE_INVALID"],
    [{ inputs: { run_upgrade_smoke_arm: null } }, "DISPATCH_INPUT_VALUE_INVALID"],
    [{ inputs: { run_upgrade_smoke_arm: "true", old: "d".repeat(40) } }, "DISPATCH_INPUTS_UNKNOWN"],
  ];
  for (const [event, code] of cases) {
    const p = installPlan("workflow_dispatch", event);
    expect(p.plan_ok).toBe(false);
    expect(p.reason_code).toBe(code);
    expect(p.jobs["upgrade-smoke-arm64"]?.selected).toBe(false);
  }
});
test("OptInSelectionTest.test_plan_cli_dispatch_opt_in_outputs", () => {
  const fx = new GitFixture();
  copyWorkflows(fx.repo);
  stub(fx.repo);
  const head = fx.commit("README.md");
  for (const [value, expected, ok] of [
    ["true", "true", "true"],
    ["false", "false", "true"],
    ["maybe", "false", "false"],
  ]) {
    const event = join(fx.repo, "event.json"),
      output = join(fx.repo, "gh-out.txt");
    writeFileSync(event, JSON.stringify({ inputs: { run_upgrade_smoke_arm: value } }));
    const p = runCli(
      [
        "plan",
        "--workflow",
        "install",
        "--repo-root",
        fx.repo,
        "--event-json",
        event,
        "--output-plan",
        join(fx.repo, "plan.json"),
        "--github-output",
        output,
      ],
      { GITHUB_EVENT_NAME: "workflow_dispatch", GITHUB_SHA: head },
    );
    expect(p.returncode, p.stderr.toString()).toBe(0);
    const lines = read(output).split("\n");
    expect(lines).toContain(`select_upgrade_smoke_arm64=${expected}`);
    expect(lines).toContain(`plan_ok=${ok}`);
    expect(lines).toContain("select_install_smoke=true");
  }
});

test("WorkflowRegistryTest.test_eslint_prettier_run_once_in_lightweight_locked_web_static", () => {
  const [data, e] = S.loadYamlMapping(join(ROOT, ".github/workflows/web.yml"));
  expect(e).toBeNull();
  const jobs = sub(record(data), "jobs");
  const lint = Object.entries(jobs).flatMap(([id, job]) =>
    S.runSteps(record(job))
      .filter((step) => String(step.run).includes("bun run lint"))
      .map((step) => ({ id, step })),
  );
  expect(lint).toHaveLength(1);
  const match = lint[0];
  expect(match?.id).toBe("web-static");
  const step = record(match?.step);
  expect(String(step.run).trimEnd().split("\n")).toEqual([
    "set -euo pipefail",
    "bun run lint:fixtures",
    "bun run lint",
    "bun run format:check",
  ]);
  expect("continue-on-error" in step).toBe(false);
  expect("if" in step).toBe(false);
  const steps = record(jobs["web-static"]).steps;
  expect(Array.isArray(steps)).toBe(true);
  if (!Array.isArray(steps)) throw new Error("steps");
  expect(
    steps.findIndex((v: unknown) => S.isMapping(v) && String(v.run ?? "").includes("bun ci")),
  ).toBeLessThan(steps.indexOf(step));
  expect(record(jobs["web-ci-gate"]).needs).toContain("web-static");
  expect(record(jobs["web-static"]).needs).toBe("ci-plan");
  expect(
    steps.some((v: unknown) => S.isMapping(v) && /cargo|rustup/.test(String(v.run ?? ""))),
  ).toBe(false);
  expect(
    Object.entries(jobs).some(
      ([id, job]) =>
        id !== "web-static" &&
        S.runScripts(record(job)).some((run) => run.includes("format:check")),
    ),
  ).toBe(false);
});
test("WorkflowRegistryTest.test_workflows_match_planner", () => {
  expect(S.verifyWorkflowRegistry()).toEqual([]);
  expect(runCli(["verify-workflows"]).returncode).toBe(0);
});
test("WorkflowRegistryTest.test_rust_suite_inventory_matches_repo", () => {
  expect(S.verifyRustSuiteRegistry(ROOT)).toEqual([]);
});
test("WorkflowRegistryTest.test_requirements_pin_pyyaml", () => {
  expect(read(join(ROOT, "scripts/ci_selection_requirements.txt"))).toContain("PyYAML==6.0.3");
});

function registryRoot(): string {
  const root = tmp();
  copyWorkflows(root);
  stub(root);
  return root;
}
function mutateRust(root: string, change: (data: Mapping) => void) {
  const path = join(root, ".github/workflows/rust.yml"),
    [data, e] = S.loadYamlMapping(path);
  expect(e).toBeNull();
  const value = record(data);
  change(value);
  writeFileSync(path, Bun.YAML.stringify(value));
}
function namedStep(data: Mapping, job: string, name: string): Mapping {
  const steps = sub(sub(data, "jobs"), job).steps;
  if (!Array.isArray(steps)) throw new Error("fixture steps");
  const step: unknown = steps.find((v: unknown) => S.isMapping(v) && v.name === name);
  return record(step);
}
function rows(data: Mapping): Mapping[] {
  const value = sub(sub(sub(sub(data, "jobs"), "postgres"), "strategy"), "matrix").include;
  if (!Array.isArray(value)) throw new Error("matrix rows");
  return value.map((v: unknown) => record(v));
}
function rustErrors(change: (data: Mapping) => void): string[] {
  const root = registryRoot();
  mutateRust(root, change);
  return S.verifyRustSuiteRegistry(root);
}
function collabInventory(root: string, body: string) {
  write(root, S.RUST_COLLAB_CI_SCRIPT, "#!/usr/bin/env bash\n" + body);
  return S.collaborationScriptInventory(root);
}
const cargoPrefix = "cargo test --locked --offline --no-fail-fast --features db-tests \\\n";
test("RustSuiteRegistryTest.test_new_cargo_target_without_ci_row_fails", () => {
  const root = registryRoot();
  stub(root, ["missing_db_target_probe"]);
  const errors = S.verifyRustSuiteRegistry(root);
  expect(errors.length).toBeGreaterThan(0);
  expect(errors.join("\n")).toContain("missing_db_target_probe");
  expect(errors.join("\n")).toContain("missing from rust.yml inventory");
});
test("RustSuiteRegistryTest.test_postgres_arm64_row_omission_fails", () => {
  const errors = rustErrors((data) => {
    for (const row of rows(data))
      if (row.runner === "ubuntu-24.04-arm")
        row.tests = String(row.tests).replace(" --test search_meili", "");
  });
  expect(errors.length).toBeGreaterThan(0);
  expect(errors.join("\n")).toContain("search_meili");
  expect(errors.join("\n")).toContain("postgres matrix missing");
});
test("RustSuiteRegistryTest.test_collaboration_script_two_invocations_union_all_targets", () => {
  const [tests, e] = S.collaborationScriptInventory(registryRoot());
  expect(e).toBeNull();
  expect(tests.size).toBe(10);
  expect(tests.has("task_collab_integration")).toBe(true);
  expect(tests.has("collab_product")).toBe(true);
});
test("RustSuiteRegistryTest.test_collaboration_script_target_in_two_invocations_fails", () => {
  const body =
    cargoPrefix +
    "  --test collab_product \\\n  --test task_collab_integration \\\n  | tee log\n" +
    cargoPrefix +
    "  --test task_collab_integration \\\n  -- --test-threads=1 \\\n  | tee -a log\n";
  const [tests, e] = collabInventory(registryRoot(), body);
  expect(tests).toEqual(new Set());
  expect(e).toContain("more than once");
  expect(e).toContain("task_collab_integration");
});
test("RustSuiteRegistryTest.test_collaboration_script_libtest_suffix_allows_only_scheduling", () => {
  for (const [suffix, allowed] of [
    ["--test-threads=1", true],
    ["--nocapture", true],
    ["some_filter", false],
    ["--skip personal_transfer", false],
    ["--ignored", false],
    ["--test-threads=4", false],
  ] as const) {
    const [tests, e] = collabInventory(
      registryRoot(),
      cargoPrefix + "  --test task_collab_integration \\\n  -- " + suffix + " \\\n  | tee log\n",
    );
    if (allowed) {
      expect(e).toBeNull();
      expect(tests).toEqual(new Set(["task_collab_integration"]));
    } else {
      expect(tests).toEqual(new Set());
      expect(e).toContain("libtest filter");
    }
  }
});
test("RustSuiteRegistryTest.test_trimmed_inventory_with_real_workflow_passes", () => {
  expect(S.verifyRustSuiteRegistry(registryRoot())).toEqual([]);
});
test("RustSuiteRegistryTest.test_verify_workflows_surfaces_rust_inventory_failure", () => {
  const root = registryRoot();
  stub(root, ["missing_db_target_probe"]);
  const p = runCli(["verify-workflows", "--repo-root", root]);
  expect(p.returncode).toBe(1);
  expect(p.stderr.toString()).toContain("missing_db_target_probe");
});
test("RustSuiteRegistryTest.test_missing_cargo_fails_instead_of_silent_pass", () => {
  const root = registryRoot();
  rmSync(join(root, "Cargo.toml"));
  expect(S.verifyRustSuiteRegistry(root).join("\n")).toContain("missing root Cargo.toml");
});
test("RustSuiteRegistryTest.test_autodiscovered_root_test_without_ci_row_fails", () => {
  const root = registryRoot();
  write(root, "tests/missing_db_target_probe.rs", '#![cfg(feature = "db-tests")]\n');
  expect(S.verifyRustSuiteRegistry(root).some((e) => e.includes("missing_db_target_probe"))).toBe(
    true,
  );
});
test("RustSuiteRegistryTest.test_postgres_decoy_test_string_without_matrix_execution_fails", () => {
  const root = registryRoot(),
    path = join(root, ".github/workflows/rust.yml");
  const decoy =
    "      - name: PostgreSQL integration tests decoy\n        run: echo --test db_integration --features db-tests\n";
  writeFileSync(
    path,
    read(path)
      .replace(
        "      - name: PostgreSQL integration tests\n",
        decoy + "      - name: PostgreSQL integration tests\n",
      )
      .replace(
        "        run: cargo test --locked --offline --no-fail-fast --features db-tests ${{ matrix.tests }}\n",
        "        run: cargo test --locked --offline --no-fail-fast --features db-tests\n",
      ),
  );
  expect(S.verifyRustSuiteRegistry(root).join("\n")).toContain("matrix.tests");
});
test("RustSuiteRegistryTest.test_s3_missing_db_features_on_execution_command_fails", () => {
  const errors = rustErrors((data) => {
    namedStep(data, "postgres", S.RUST_S3_INTEGRATION_STEP).run =
      "bash scripts/start-test-minio.sh cargo test --locked --offline --no-fail-fast --test attachment_s3_integration";
  });
  expect(errors.some((e) => e.includes("S3 integration step"))).toBe(true);
});
test("RustSuiteRegistryTest.test_late_crate_cfg_autotest_is_registered", () => {
  const root = registryRoot();
  write(
    root,
    "tests/missing_db_target_probe.rs",
    "//! pad\n".repeat(12) + '#![cfg(feature = "db-tests")]\n',
  );
  expect(S.verifyRustSuiteRegistry(root).some((e) => e.includes("missing_db_target_probe"))).toBe(
    true,
  );
});
test("RustSuiteRegistryTest.test_item_level_cfg_only_root_test_fails", () => {
  const root = registryRoot();
  write(root, "tests/missing_db_target_probe.rs", '#[cfg(feature = "db-tests")]\nmod suite {}\n');
  const joined = S.verifyRustSuiteRegistry(root).join("\n");
  expect(joined).toContain("missing_db_target_probe");
  expect(joined).toContain("no crate");
});
test("RustSuiteRegistryTest.test_postgres_integration_step_if_false_fails", () => {
  expect(
    rustErrors((data) => {
      namedStep(data, "postgres", S.RUST_POSTGRES_INTEGRATION_STEP).if = "false";
    }).some((e) => e.includes("must not have an if condition")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_s3_integration_step_if_false_fails", () => {
  expect(
    rustErrors((data) => {
      namedStep(data, "postgres", S.RUST_S3_INTEGRATION_STEP).if = "false";
    }).some((e) => e.includes("S3 integration step if must be")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_collaboration_integration_step_if_false_fails", () => {
  expect(
    rustErrors((data) => {
      namedStep(data, "collaboration", S.RUST_COLLAB_INTEGRATION_STEP).if = "false";
    }).some((e) => e.includes("collaboration integration step must not have an if condition")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_postgres_integration_continue_on_error_fails", () => {
  expect(
    rustErrors((data) => {
      namedStep(data, "postgres", S.RUST_POSTGRES_INTEGRATION_STEP)["continue-on-error"] = true;
    }).some((e) => e.includes("continue-on-error")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_collaboration_echo_script_not_execution_fails", () => {
  const root = registryRoot(),
    path = join(root, ".github/workflows/rust.yml");
  writeFileSync(
    path,
    read(path).replace(
      "        run: bash scripts/run-rust-collaboration-ci-tests.sh\n",
      "        run: echo bash scripts/run-rust-collaboration-ci-tests.sh\n",
    ),
  );
  expect(
    S.verifyRustSuiteRegistry(root).some((e) => e.includes("collaboration integration step")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_postgres_integration_no_run_suffix_fails", () => {
  expect(
    rustErrors((data) => {
      const s = namedStep(data, "postgres", S.RUST_POSTGRES_INTEGRATION_STEP);
      s.run = String(s.run) + " --no-run";
    }).some((e) => e.includes("--no-run")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_postgres_integration_exclude_fails", () => {
  expect(
    rustErrors((data) => {
      const s = namedStep(data, "postgres", S.RUST_POSTGRES_INTEGRATION_STEP);
      s.run = String(s.run).replace(
        "${{ matrix.tests }}",
        "--exclude fvoci-server ${{ matrix.tests }}",
      );
    }).some((e) => e.includes("--exclude")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_postgres_integration_libtest_skip_fails", () => {
  expect(
    rustErrors((data) => {
      const s = namedStep(data, "postgres", S.RUST_POSTGRES_INTEGRATION_STEP);
      s.run = String(s.run) + " -- --skip '*'";
    }).some((e) => e.includes("libtest filter")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_postgres_integration_shell_or_true_fails", () => {
  expect(
    rustErrors((data) => {
      const s = namedStep(data, "postgres", S.RUST_POSTGRES_INTEGRATION_STEP);
      s.run = String(s.run) + " || true";
    }).some((e) => e.includes("shell operator")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_postgres_matrix_tests_no_run_fragment_fails", () => {
  expect(
    rustErrors((data) => {
      const row = rows(data)[0];
      if (!row) throw new Error("row");
      row.tests = "--no-run --test db_integration";
    }).some((e) => e.includes("--no-run") || e.includes("--test NAME")),
  ).toBe(true);
});
test("RustSuiteRegistryTest.test_postgres_integration_continue_on_error_string_fails", () => {
  expect(
    rustErrors((data) => {
      namedStep(data, "postgres", S.RUST_POSTGRES_INTEGRATION_STEP)["continue-on-error"] = "true";
    }).some((e) => e.includes("continue-on-error")),
  ).toBe(true);
});

function noGreen(root: string, needle: string) {
  const event = join(root, "event.json"),
    out = join(root, "green-plan.json"),
    gh = join(root, "github-output.txt");
  writeFileSync(event, "{}");
  const p = runCli(
    [
      "plan",
      "--workflow",
      "web",
      "--repo-root",
      root,
      "--event-json",
      event,
      "--output-plan",
      out,
      "--github-output",
      gh,
    ],
    { GITHUB_EVENT_NAME: "pull_request", GITHUB_SHA: "a".repeat(40) },
  );
  expect(p.returncode, p.stderr.toString()).toBe(1);
  expect(p.stderr.toString()).toContain("workflow registry validation failed");
  expect(p.stderr.toString()).toContain(needle);
  expect(existsSync(out)).toBe(false);
  expect(existsSync(gh)).toBe(false);
}
function replaceWorkflow(root: string, file: string, old: string, next: string, once = false) {
  const path = join(root, ".github/workflows", file),
    text = read(path);
  expect(text).toContain(old);
  writeFileSync(path, once ? text.replace(old, next) : text.replaceAll(old, next));
}
const gateInvocation =
  '          python3 scripts/ci_selection.py gate --workflow web --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n';
test("RegistryMutationCliTest.test_pr_path_filters_cannot_leave_gate_pending", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    "  pull_request:\n",
    "  pull_request:\n    paths: ['apps/web/**']\n",
    true,
  );
  noGreen(root, "pull_request must be unfiltered");
});
test("RegistryMutationCliTest.test_checkout_ref_and_repository_overrides_rejected", () => {
  for (const override of ["ref: attacker-head", "repository: attacker/repo", "fetch-depth: 1"]) {
    const root = registryRoot();
    replaceWorkflow(root, "web.yml", "          fetch-depth: 0", "          " + override, true);
    noGreen(root, "ci-plan must checkout the event merge");
  }
});
test("RegistryMutationCliTest.test_runner_sha_yaml_override_rejected", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    "          GITHUB_EVENT_NAME:",
    "          GITHUB_SHA: attacker-head\n          GITHUB_EVENT_NAME:",
    true,
  );
  noGreen(root, "must not override trusted GITHUB_SHA");
});
test("RegistryMutationCliTest.test_new_job_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    "  web-ci-gate:",
    "\n  new-suite:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_new_suite == 'true'\n    runs-on: ubuntu-24.04\n    steps:\n      - run: echo new\n  web-ci-gate:",
  );
  noGreen(root, "unregistered job id new-suite");
});
test("RegistryMutationCliTest.test_install_opt_in_wiring_mutations_rejected_before_outputs", () => {
  const cases: [string, string, string][] = [
    ["        default: false\n", "        default: true\n", "input run_upgrade_smoke_arm must be"],
    ["        type: boolean\n", "        type: string\n", "input run_upgrade_smoke_arm must be"],
    [
      "      run_upgrade_smoke_arm:\n",
      "      upgrade_old:\n        type: string\n      run_upgrade_smoke_arm:\n",
      "workflow_dispatch inputs must be exactly",
    ],
    [
      "  upgrade-smoke-arm64:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n    runs-on: ubuntu-24.04-arm\n",
      "  upgrade-smoke-arm64:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n    runs-on: ubuntu-24.04\n",
      "upgrade-smoke-arm64 runs-on must be ubuntu-24.04-arm",
    ],
    [
      "    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n",
      "    if: github.event_name == 'workflow_dispatch'\n",
      "upgrade-smoke-arm64 if must be",
    ],
    [
      "    needs: [ci-plan, install-smoke, backup-restore-smoke, upgrade-smoke-arm64]\n",
      "    needs: [ci-plan, install-smoke, backup-restore-smoke]\n",
      "install-ci-gate needs must be",
    ],
    [
      "      select_upgrade_smoke_arm64: ${{ steps.plan.outputs.select_upgrade_smoke_arm64 }}\n",
      "",
      "missing selector output select_upgrade_smoke_arm64",
    ],
  ];
  for (const [old, next, needle] of cases) {
    const root = registryRoot();
    expect(read(join(root, ".github/workflows/install.yml")).split(old)).toHaveLength(2);
    replaceWorkflow(root, "install.yml", old, next);
    noGreen(root, needle);
  }
});
test("RegistryMutationCliTest.test_opt_in_input_on_other_workflow_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    "  workflow_dispatch:\n",
    "  workflow_dispatch:\n    inputs:\n      run_upgrade_smoke_arm:\n        type: boolean\n        default: false\n",
    true,
  );
  noGreen(root, "web: workflow_dispatch inputs must be exactly []");
});
test("RegistryMutationCliTest.test_gate_suffixed_product_job_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    "  web-ci-gate:",
    "\n  sneaky-ci-gate:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_sneaky_ci_gate == 'true'\n    runs-on: ubuntu-24.04\n    steps:\n      - run: echo sneaky\n  web-ci-gate:",
  );
  noGreen(root, "unregistered job id sneaky-ci-gate");
});
test("RegistryMutationCliTest.test_release_workflow_must_stay_tag_only", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "release.yml",
    "  workflow_dispatch:",
    "  pull_request:\n  workflow_dispatch:",
    true,
  );
  noGreen(root, "release.yml: triggers must be exactly push (tags) and workflow_dispatch");
});
test("RegistryMutationCliTest.test_ci_base_workflow_write_scope_registration_passes", () => {
  expect(S.verifyWorkflowRegistry(registryRoot())).toEqual([]);
});
test("RegistryMutationCliTest.test_ci_base_workflow_extra_write_scope_rejected_before_outputs", () => {
  for (const [job, scope] of [
    ["build", "packages"],
    ["push", "issues"],
    ["push", "contents"],
    ["push-manifest", "contents"],
    ["push-manifest", "issues"],
  ] as const) {
    const root = registryRoot(),
      path = join(root, ".github/workflows/ci-base-image.yml");
    let text = read(path);
    const jobAt = text.indexOf(`  ${job}:\n`, text.indexOf("jobs:\n"));
    expect(jobAt).toBeGreaterThanOrEqual(0);
    let at: number, addition: string;
    if (job === "build") {
      at = jobAt + `  ${job}:\n`.length;
      addition = `    permissions:\n      ${scope}: write\n`;
    } else {
      at = text.indexOf("    permissions:\n", jobAt) + "    permissions:\n".length;
      addition = `      ${scope}: write\n`;
      if (scope === "contents")
        text = text.slice(0, at) + text.slice(at).replace("      contents: read\n", "");
    }
    writeFileSync(path, text.slice(0, at) + addition + text.slice(at));
    noGreen(root, `ci-base-image.yml: ${job} may not write ['${scope}']`);
  }
});
test("RegistryMutationCliTest.test_release_workflow_write_scope_outside_listed_job_rejected", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "release.yml",
    "      contents: read\n      checks: read\n",
    "      contents: write\n      checks: read\n",
    true,
  );
  noGreen(root, "release.yml: verify may not write ['contents']");
});
test("RegistryMutationCliTest.test_release_workflow_per_tag_concurrency_rejected", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "release.yml",
    "  group: release-ghcr-fvoci\n",
    "  group: release-${{ github.ref_name }}\n",
    true,
  );
  noGreen(root, "release.yml: concurrency must be one fixed group with cancel-in-progress: false");
});
test("RegistryMutationCliTest.test_release_publish_job_may_not_write_contents", () => {
  const root = registryRoot(),
    path = join(root, ".github/workflows/release.yml"),
    text = read(path),
    marker = "  publish:\n    needs: [verify, index, smoke]\n";
  expect(text).toContain(marker);
  const at = text.indexOf("      packages: write\n", text.indexOf(marker));
  writeFileSync(path, text.slice(0, at) + "      contents: write\n" + text.slice(at));
  noGreen(root, "release.yml: publish may not write ['contents']");
});
test("RegistryMutationCliTest.test_new_workflow_rejected_before_outputs", () => {
  const root = registryRoot();
  write(
    root,
    ".github/workflows/extra.yml",
    "name: Extra\non: push\njobs:\n  extra-job:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: echo x\n",
  );
  noGreen(root, "unknown workflow file extra.yml");
});
test("RegistryMutationCliTest.test_missing_selector_output_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    "      select_web_checks: ${{ steps.plan.outputs.select_web_checks }}\n",
    "",
  );
  noGreen(root, "missing selector output select_web_checks");
});
test("RegistryMutationCliTest.test_gate_needs_mismatch_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    "    needs: [ci-plan, web-static, web-checks, workspace-browser-shard, collaboration-flow]",
    "    needs: [ci-plan, web-checks]",
  );
  noGreen(root, "needs must be ci-plan and every registered job");
});
test("RegistryMutationCliTest.test_swapped_needs_json_expr_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "rust.yml",
    "NEEDS_JSON: ${{ toJSON(needs) }}",
    "NEEDS_JSON: ${{ toJSON(needs.postgres) }}",
  );
  noGreen(root, "env must be exactly");
});
test("RegistryMutationCliTest.test_forged_needs_json_literal_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "rust.yml",
    "NEEDS_JSON: ${{ toJSON(needs) }}",
    'NEEDS_JSON: \'{"fast":{"result":"success"}}\'',
  );
  noGreen(root, "env must be exactly");
});
test("RegistryMutationCliTest.test_swapped_tested_sha_expr_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "rust.yml",
    "TESTED_SHA: ${{ github.sha }}",
    "TESTED_SHA: ${{ needs.fast.result }}",
  );
  noGreen(root, "env must be exactly");
});
test("RegistryMutationCliTest.test_extra_job_env_mapping_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "rust.yml",
    "          TESTED_SHA: ${{ github.sha }}\n",
    "          TESTED_SHA: ${{ github.sha }}\n          JOB_FAST: ${{ needs.postgres.result }}\n",
  );
  noGreen(root, "env must be exactly");
});
test("RegistryMutationCliTest.test_decoy_echo_gate_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    gateInvocation,
    gateInvocation.replace("          python3", "          echo python3"),
  );
  noGreen(root, "canonical gate invocation");
});
test("RegistryMutationCliTest.test_commented_gate_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(
    root,
    "web.yml",
    gateInvocation,
    gateInvocation.replace("          python3", "          # python3") + "          true\n",
  );
  noGreen(root, "canonical gate invocation");
});
test("RegistryMutationCliTest.test_rust_plan_missing_selector_wrapper_rejected_before_outputs", () => {
  const root = registryRoot();
  replaceWorkflow(root, "rust.yml", "          bash scripts/test-ci-selection.sh\n", "", true);
  noGreen(root, "must run scripts/test-ci-selection.sh");
});
test("RegistryMutationCliTest.test_web_plan_duplicate_selector_wrapper_rejected_before_outputs", () => {
  const root = registryRoot();
  const marker = "          python3 scripts/ci_selection.py plan \\\n";
  replaceWorkflow(
    root,
    "web.yml",
    marker,
    "          bash scripts/test-ci-selection.sh\n" + marker,
    true,
  );
  noGreen(root, "must not duplicate scripts/test-ci-selection.sh");
});

test("ImpactUnionTest.test_browser_and_unit_tests_run_web_without_install", () => {
  for (const path of [
    "apps/web/e2e/new-flow.spec.ts",
    "apps/web/e2e-pending/workspace-wiki-vue-collab.spec.ts",
    "apps/web/e2e/helpers.ts",
    "apps/web/e2e/mfa-helpers.ts",
    "apps/web/e2e/workspace-wiki-vue-editor.ts",
    "apps/web/e2e-pending/collab-helpers.ts",
    "apps/web/e2e-pending/collab-helpers.test.ts",
    "apps/web/src/vue/router.test.ts",
    "packages/editor/test/vue-menu-selection.test.ts",
  ]) {
    selectedWorkflows([path], new Set(["web"]));
    selectedWorkflows(["docs/rewrite.md", path], new Set(["web"]));
  }
});
test("ImpactUnionTest.test_editor_ui_keeps_browser_and_install", () => {
  for (const path of [
    "packages/editor/src/vue/FvociEditor.vue",
    "packages/editor/src/react/block-menu.tsx",
    "packages/editor/src/react/editor.css",
    "packages/editor/src/clipboard.ts",
    "packages/editor/src/gutter-actions.ts",
    "packages/editor/src/menu-roving.ts",
  ]) {
    selectedWorkflows([path], new Set(["web", "install"]));
    selectedWorkflows(["README.md", "apps/web/e2e/foo.spec.ts", path], new Set(["web", "install"]));
  }
});
test("ImpactUnionTest.test_new_explanatory_docs_are_exact", () => {
  selectedWorkflows(["docs/RELEASING.md", "docs/collab-engine-comparison.md"], new Set());
  for (const path of [
    "docs/fixtures/example.md",
    "docs/generated/api.md",
    "docs/other.md",
    "docs/collab-engine-comparison.md.bak",
  ])
    expect(S.decideFromPaths([path]).mode).toBe("full");
});
test("ImpactUnionTest.test_backend_contracts_harness_and_unknown_stay_full", () => {
  for (const path of [
    "packages/editor/src/tiptap-schema.ts",
    "packages/editor/src/collab-tiptap.ts",
    "packages/editor/src/json.ts",
    "packages/editor/src/export/pdf.tsx",
    "packages/editor/src/fonts/NotoSansKR.ttf",
    "packages/editor/src/react/schema.tsx",
    "packages/editor/src/vue/new.wasm",
    "packages/editor/test/schema-dump.ts",
    "packages/editor/test/setup/vue-sfc.ts",
    "packages/editor/tsconfig.json",
    "packages/i18n/src/locales/ko.json",
    "apps/web/e2e/fixtures/markdown-import.zip",
    "apps/web/e2e/nested/foo.spec.ts",
    "apps/web/e2e/new-harness.ts",
    "apps/web/e2e-pending/collab-restart.ts",
    "apps/web/e2e-pending/collab-wire.ts",
    "apps/web/e2e-pending/collab-attachment-oracle.ts",
    "apps/web/e2e-pending/collab-playwright.config.ts",
    "apps/web/src/generated/api.test.ts",
    "apps/web/src/fixtures/backend.sql",
    "apps/web/src/new-contract.json",
    "scripts/run-web-e2e.sh",
    "scripts/ci_selection.py",
    "src/auth.rs",
    "migrations/045.sql",
    "new-unknown-file.ts",
    "apps/web/src/../../src/main.rs",
    "apps/web//src/test.ts",
  ])
    for (const [workflow, p] of Object.entries(
      all(["README.md", "apps/web/e2e/foo.spec.ts", path]),
    )) {
      expect(p.mode).toBe("full");
      full(workflow, p);
    }
});
test("ImpactUnionTest.test_candidate_golden_plans", () => {
  const snapshot = record(json(join(ROOT, "scripts/fixtures/ci-selection/candidates.json")));
  if (!Array.isArray(snapshot.candidates)) throw new Error("candidates");
  const candidates = snapshot.candidates.map((v: unknown) => record(v));
  expect(new Set(candidates.map((v) => v.number))).toEqual(
    new Set([265, 267, 269, 270, 271, 263, 280]),
  );
  for (const item of candidates) {
    expect(S.validateSha(String(item.head_sha))).toBe(true);
    if (!Array.isArray(item.paths) || !item.paths.every((v: unknown) => typeof v === "string"))
      throw new Error("paths");
    selectedWorkflows(
      item.paths as string[],
      new Set(item.number === 263 || item.number === 280 ? [] : ["web", "install"]),
    );
  }
});

test("PrMergeImpactTest.test_exact_parents_merge_only_add_delete_rename_force_full", () => {
  for (const operation of ["add", "delete", "rename"]) {
    const fx = new PrFixture();
    write(fx.origin, "src/keep.rs", "base backend\n");
    git(fx.origin, "add", ".");
    git(fx.origin, "commit", "-m", "backend base");
    const base = sha(fx.origin),
      head = fx.commit("docs", "README.md", "changed docs\n");
    fx.clone();
    fx.merge(base, head);
    if (operation === "add") write(fx.work, "src/injected.rs", "merge only\n");
    else if (operation === "delete") git(fx.work, "rm", "src/keep.rs");
    else {
      mkdirSync(join(fx.work, "apps/web/src"), { recursive: true });
      git(fx.work, "mv", "src/keep.rs", "apps/web/src/disguised.ts");
    }
    git(fx.work, "add", ".");
    git(fx.work, "commit", "--amend", "--no-edit");
    const input = resolveInputs(fx, base, head, sha(fx.work));
    expect(input.fatal_error).toBeNull();
    expect(input.force_full_reason).toBeNull();
    expect(input.paths).toContain("README.md");
    expect(S.decideFromPaths(input.paths ?? []).mode).toBe("full");
  }
});
test("PrMergeImpactTest.test_advanced_base_merge_resolution_only_change_is_classified", () => {
  const fx = new PrFixture(),
    head = fx.commit("docs", "README.md", "changed docs\n");
  write(fx.origin, "src/advanced.rs", "base backend\n");
  git(fx.origin, "add", ".");
  git(fx.origin, "commit", "-m", "advanced base");
  const advanced = sha(fx.origin);
  fx.clone();
  fx.merge(advanced, head);
  write(fx.work, "src/advanced.rs", "merge resolution\n");
  git(fx.work, "add", ".");
  git(fx.work, "commit", "--amend", "--no-edit");
  const input = resolveInputs(fx, fx.base, head, sha(fx.work));
  expect(input.force_full_reason).toBeNull();
  expect(input.paths).toContain("src/advanced.rs");
  expect(S.decideFromPaths(input.paths ?? []).mode).toBe("full");
});
test("PrMergeImpactTest.test_cumulative_head_changes_survive_merge_tree_omission", () => {
  const fx = new PrFixture(),
    head = fx.commit("backend", "src/new.rs", "backend change\n");
  fx.clone();
  fx.merge(fx.base, head);
  git(fx.work, "rm", "src/new.rs");
  write(fx.work, "README.md", "merge omitted backend\n");
  git(fx.work, "add", ".");
  git(fx.work, "commit", "--amend", "--no-edit");
  const input = resolveInputs(fx, fx.base, head, sha(fx.work));
  expect(input.paths).toContain("src/new.rs");
  expect(S.decideFromPaths(input.paths ?? []).mode).toBe("full");
});
test("PrMergeImpactTest.test_unrelated_or_older_first_parent_cannot_narrow", () => {
  for (const kind of ["unrelated", "older"]) {
    const fx = new PrFixture(),
      head = fx.commit("docs", "README.md", "changed docs\n");
    write(fx.origin, "src/advanced.rs", "advance\n");
    git(fx.origin, "add", ".");
    git(fx.origin, "commit", "-m", "advance");
    const base = sha(fx.origin);
    fx.clone();
    let tested: string;
    if (kind === "older") tested = fx.merge(fx.base, head);
    else {
      git(fx.work, "checkout", "--orphan", "unrelated");
      git(fx.work, "rm", "-rf", ".");
      write(fx.work, "unrelated", "x\n");
      git(fx.work, "add", ".");
      git(fx.work, "commit", "-m", "unrelated root");
      const tree = sha(fx.work, `${head}^{tree}`),
        unrelated = sha(fx.work);
      tested = git(
        fx.work,
        "commit-tree",
        tree,
        "-p",
        unrelated,
        "-p",
        head,
        "-m",
        "spoofed merge",
      );
      git(fx.work, "checkout", "--detach", tested);
    }
    const input = resolveInputs(fx, base, head, tested);
    expect(input.force_full_reason).toBe("FULL_PR_MERGE_PARENTS_MISMATCH");
    expect(input.paths).toBeNull();
  }
});
test("PrMergeImpactTest.test_missing_history_and_tested_sha_mismatch_fail_closed", () => {
  const fx = new PrFixture(),
    head = fx.commit("docs", "README.md", "changed docs\n");
  fx.clone();
  const tested = fx.merge(fx.base, head);
  expect(resolveInputs(fx, fx.base, head, head).fatal_error).toBe("TESTED_SHA_MISMATCH");
  const mock = spyOn(S.selectionGit, "fetchOrigin").mockReturnValue("FETCH_FAILED");
  try {
    const input = resolveInputs(fx, "f".repeat(40), head, tested);
    expect(input.fatal_error).toBe("FETCH_FAILED");
    expect(input.paths).toBeNull();
  } finally {
    mock.mockRestore();
  }
});
test("PrMergeImpactTest.test_actual_merge_diff_failure_is_fatal", () => {
  const fx = new PrFixture(),
    head = fx.commit("docs", "README.md", "changed docs\n");
  fx.clone();
  const tested = fx.merge(fx.base, head),
    mock = spyOn(S.selectionGit, "diffPaths")
      .mockReturnValueOnce([["README.md"], null])
      .mockReturnValueOnce([[], "GIT_DIFF_FAILED"]);
  try {
    const input = resolveInputs(fx, fx.base, head, tested);
    expect(input.fatal_error).toBe("GIT_DIFF_FAILED");
    expect(input.paths).toBeNull();
  } finally {
    mock.mockRestore();
  }
});

const AGENT_DOCS = ["AGENTS.md", ".agents/environment.md"];
function docsOnly(paths: string[]) {
  for (const p of Object.values(all(paths))) {
    expect(p.mode).toBe("narrow");
    expect(p.reason_code).toBe("NARROW_DOCS");
    expect(p.plan_ok).toBe(true);
    for (const meta of Object.values(p.jobs)) expect(meta.selected).toBe(false);
  }
}
function fullPaths(paths: string[], reason?: string) {
  for (const [workflow, p] of Object.entries(all(paths))) {
    expect(p.mode).toBe("full");
    if (reason) expect(p.reason_code).toBe(reason);
    full(workflow, p);
  }
}
function diff(fx: GitFixture, base: string): string[] {
  const [paths, e] = S.diffPathsForPr(fx.repo, base, sha(fx.repo));
  expect(e).toBeNull();
  return paths ?? [];
}
test("AgentDocsSelectionTest.test_exact_agent_docs_classify_as_docs", () => {
  for (const path of AGENT_DOCS) expect(S.classifyPath(path)).toBe("docs");
});
test("AgentDocsSelectionTest.test_other_agents_paths_stay_broaden_or_unknown", () => {
  for (const path of [
    ".agents/skills/fvoci-fast-verify/SKILL.md",
    ".agents/skills/fvoci-standard-implementations/references/candidates.md",
    ".agents/environment.md.bak",
    ".agents/environment.mdx",
    ".agents/other.md",
    ".agents/sub/environment.md",
  ])
    expect(S.classifyPath(path)).toBe("broaden");
  for (const path of ["agents/environment.md", "AGENTS.MD", "apps/AGENTS.md", "AGENTS.md.orig"])
    expect(S.classifyPath(path)).toBe("unknown");
  expect(S.classifyPath("docs/AGENTS.md")).toBe("broaden");
  expect(S.classifyPath(".agents/")).toBe("unknown");
  expect(S.classifyPath("scripts/AGENTS.md")).toBe("broaden");
});
test("AgentDocsSelectionTest.test_explicit_docs_never_overlap_build_inputs", () => {
  for (const path of S.EXPLICIT_DOCS) {
    expect(S.BROADEN_EXACT.has(path)).toBe(false);
    for (const marker of S.MANIFEST_MARKERS) expect(path).not.toContain(marker);
    expect(path.endsWith("/")).toBe(false);
    expect(path).not.toContain("*");
  }
});
test("AgentDocsSelectionTest.test_pr135_cumulative_paths_docs_only", () => {
  for (const paths of [
    ["AGENTS.md"],
    [".agents/environment.md"],
    AGENT_DOCS,
    [".agents/environment.md", "AGENTS.md", "docs/rewrite.md", "README.md"],
  ])
    docsOnly(paths);
});
test("AgentDocsSelectionTest.test_agent_docs_with_code_or_selector_is_full", () => {
  for (const extra of [
    "src/lib.rs",
    "tests/db_integration.rs",
    "migrations/0001_init.sql",
    ".github/workflows/rust.yml",
    ".github/workflows/web.yml",
    "scripts/ci_selection.py",
    "scripts/test_ci_selection.py",
    "scripts/test-ci-selection.sh",
    "scripts/ci_selection_requirements.txt",
    ".agents/skills/fvoci-fast-verify/SKILL.md",
  ])
    fullPaths([...AGENT_DOCS, extra], "FULL_PATH_BROADEN");
});
test("AgentDocsSelectionTest.test_agent_docs_with_executable_config_is_full", () => {
  for (const extra of [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "Dockerfile",
    ".dockerignore",
    "infra/rust/Dockerfile",
    "apps/web/package.json",
    "apps/web/playwright.config.ts",
    "crates/collab-engine/Cargo.toml",
    "packages/editor/package.json",
    "package.json",
    "bun.lock",
    "eslint.config.mjs",
    ".prettierrc.json",
    ".prettierignore",
    "bunfig.toml",
    ".bun-version",
    "patches/@volar%2Ftypescript@2.4.28.patch",
    "scripts/document-convert/package.json",
  ])
    fullPaths([...AGENT_DOCS, extra], "FULL_PATH_BROADEN");
});
test("AgentDocsSelectionTest.test_agent_docs_with_fixture_or_unknown_is_full", () => {
  for (const extra of ["compat/fixtures/x.json", "scripts/fixtures/web-e2e/x.sh", "docs/other.md"])
    fullPaths([...AGENT_DOCS, extra], "FULL_PATH_BROADEN");
  for (const extra of [".gitignore", "LICENSE", "third-party/x.md", "apps/AGENTS.md", "notes.md"])
    fullPaths([...AGENT_DOCS, extra], "FULL_UNKNOWN_PATH");
});
test("AgentDocsSelectionTest.test_agent_docs_with_frontend_unions_impacts", () => {
  for (const [workflow, p] of Object.entries(all([...AGENT_DOCS, "apps/web/src/x.ts"]))) {
    expect(p.mode).toBe("narrow");
    expect(p.reason_code).toBe("NARROW_FRONTEND_WEB_INSTALL");
    for (const [job, meta] of Object.entries(p.jobs))
      expect(meta.selected).toBe(["web", "install"].includes(workflow) && !optIn(workflow, job));
  }
});
test("AgentDocsSelectionTest.test_always_full_events_ignore_agent_docs", () => {
  expect(S.ALWAYS_FULL_EVENTS).toEqual(new Set(["push", "merge_group", "workflow_dispatch"]));
  for (const [name, reason] of [
    ["push", "FULL_EVENT_PUSH"],
    ["merge_group", "FULL_EVENT_MERGE_GROUP"],
    ["workflow_dispatch", "FULL_EVENT_WORKFLOW_DISPATCH"],
  ])
    for (const [workflow, p] of Object.entries(all(AGENT_DOCS, { event_name: name }))) {
      expect(p.mode).toBe("full");
      expect(p.reason_code).toBe(reason);
      expect(p.plan_ok).toBe(true);
      full(workflow, p);
    }
});
test("AgentDocsSelectionTest.test_diff_failure_fails_closed", () => {
  for (const fatal of [
    "GIT_DIFF_FAILED",
    "DIFF_TRUNCATED",
    "DIFF_TRUNCATED_RENAME",
    "FETCH_FAILED",
  ])
    for (const [workflow, p] of Object.entries(all(AGENT_DOCS, { fatal_error: fatal }))) {
      expect(p.mode).toBe("full");
      expect(p.reason_code).toBe(fatal);
      expect(p.plan_ok).toBe(false);
      full(workflow, p);
    }
  for (const p of Object.values(all(null))) {
    expect(p.reason_code).toBe("FULL_MISSING_PATHS");
    expect(p.plan_ok).toBe(false);
  }
  expect(S.parseNameStatusZ(bytes("M\0AGENTS.md\0M\0.agents/environment.md"))).toEqual([
    [],
    "DIFF_TRUNCATED",
  ]);
  expect(S.parseNameStatusZ(bytes("R100\0AGENTS.md\0"))).toEqual([[], "DIFF_TRUNCATED_RENAME"]);
});
test("AgentDocsSelectionTest.test_git_diff_command_failure_fails_closed", () => {
  const fx = new GitFixture(),
    base = fx.commit("AGENTS.md", "a\n"),
    [paths, e] = S.diffPathsForPr(fx.repo, base, "f".repeat(40));
  expect(paths).toBeNull();
  expect(["REV_PARSE_FAILED", "MERGE_BASE_FAILED"]).toContain(e);
  const head = fx.commit(".agents/environment.md", "b\n"),
    real = S.gitOperations.run;
  const mock = spyOn(S.gitOperations, "run").mockImplementation(
    (repo: string, ...args: string[]): GitResult =>
      args[0] === "diff"
        ? { returncode: 128, stdout: bytes(""), stderr: bytes("boom") }
        : real(repo, ...args),
  );
  try {
    const [p, err] = S.diffPathsForPr(fx.repo, base, head);
    expect(p).toBeNull();
    expect(err).toBe("GIT_DIFF_FAILED");
  } finally {
    mock.mockRestore();
  }
});
test("AgentDocsSelectionTest.test_real_diff_modify_agent_docs_is_docs_only", () => {
  const fx = new GitFixture();
  fx.commit("AGENTS.md", "a\n");
  const base = fx.commit(".agents/environment.md", "a\n");
  fx.commit("AGENTS.md", "b\n");
  fx.commit(".agents/environment.md", "b\n");
  const paths = diff(fx, base);
  expect([...paths].sort()).toEqual([".agents/environment.md", "AGENTS.md"]);
  expect(S.decideFromPaths(paths).reason_code).toBe("NARROW_DOCS");
});
test("AgentDocsSelectionTest.test_real_diff_delete_agent_docs", () => {
  const fx = new GitFixture();
  fx.commit("AGENTS.md", "a\n");
  const base = fx.commit(".agents/skills/x/SKILL.md", "a\n");
  fx.delete("AGENTS.md");
  let paths = diff(fx, base);
  expect(paths).toEqual(["AGENTS.md"]);
  expect(S.decideFromPaths(paths).reason_code).toBe("NARROW_DOCS");
  fx.delete(".agents/skills/x/SKILL.md");
  paths = diff(fx, base);
  expect(paths).toContain(".agents/skills/x/SKILL.md");
  expect(S.decideFromPaths(paths).mode).toBe("full");
});
test("AgentDocsSelectionTest.test_real_diff_rename_checks_old_and_new_names", () => {
  for (const [old, next, reason] of [
    ["AGENTS.md", "notes/AGENTS.md", "FULL_UNKNOWN_PATH"],
    [".agents/environment.md", ".agents/skills/environment.md", "FULL_PATH_BROADEN"],
    [".agents/skills/x/SKILL.md", ".agents/environment.md", "FULL_PATH_BROADEN"],
    ["src/env.md", "AGENTS.md", "FULL_PATH_BROADEN"],
    ["AGENTS.md", "Cargo.toml", "FULL_PATH_BROADEN"],
  ] as const) {
    const fx = new GitFixture(),
      base = fx.commit(old, "role record line\n".repeat(20));
    mkdirSync(dirname(join(fx.repo, next)), { recursive: true });
    fx.rename(old, next);
    const paths = diff(fx, base);
    expect(paths).toEqual([old, next]);
    const decision = S.decideFromPaths(paths);
    expect(decision.mode).toBe("full");
    expect(decision.reason_code).toBe(reason);
  }
});
test("AgentDocsSelectionTest.test_real_diff_rename_between_agent_docs_stays_docs", () => {
  const fx = new GitFixture(),
    base = fx.commit(".agents/environment.md", "role record line\n".repeat(20));
  fx.rename(".agents/environment.md", "AGENTS.md");
  const paths = diff(fx, base);
  expect(paths).toEqual([".agents/environment.md", "AGENTS.md"]);
  expect(S.decideFromPaths(paths).reason_code).toBe("NARROW_DOCS");
});
test("AgentDocsSelectionTest.test_pr_merge_checkout_agent_docs_narrow_docs", () => {
  const fx = new PrFixture();
  git(fx.origin, "checkout", "-B", "pr", "main");
  for (const rel of AGENT_DOCS) write(fx.origin, rel, "role record\n");
  git(fx.origin, "add", ...AGENT_DOCS);
  git(fx.origin, "commit", "-m", "agent docs");
  const head = sha(fx.origin);
  git(fx.origin, "checkout", "main");
  fx.clone();
  const p = fx.plan(fx.merge(fx.base, head), fx.base, head);
  expect(p.mode).toBe("narrow");
  expect(p.reason_code).toBe("NARROW_DOCS");
  expect(p.path_count).toBe(2);
  expect(p.plan_ok).toBe(true);
  expect(Object.values(sub(p, "jobs")).some((v) => record(v).selected)).toBe(false);
});

test("AgentDocsGateTest.test_opt_in_gate_selected_bad_result_or_missing_rejected", () => {
  const p = gatePlan("install", { "upgrade-smoke-arm64": true }),
    ok = { "upgrade-smoke-arm64": "success" };
  expect(gate(p, "install", ok)).toBe(0);
  for (const result of ["failure", "cancelled", "skipped"])
    expect(gate(p, "install", { "upgrade-smoke-arm64": result })).toBe(1);
  expect(gate(p, "install", ok, { omit_jobs: new Set(["upgrade-smoke-arm64"]) })).toBe(1);
});
test("AgentDocsGateTest.test_opt_in_gate_unchosen_ran_rejected", () => {
  const p = gatePlan("install", {});
  const cases: [string, unknown][] = [
    ["pull_request", {}],
    ["push", {}],
    ["merge_group", {}],
    ["workflow_dispatch", { inputs: { run_upgrade_smoke_arm: "false" } }],
  ];
  for (const [name, event] of cases) {
    expect(gate(p, "install", {}, { event_name: name, event })).toBe(0);
    for (const result of ["success", "failure", "cancelled"])
      expect(
        gate(p, "install", { "upgrade-smoke-arm64": result }, { event_name: name, event }),
      ).toBe(1);
  }
});
test("AgentDocsGateTest.test_opt_in_gate_rejects_plan_override", () => {
  const forced = gatePlan("install", { "upgrade-smoke-arm64": true });
  const cases: [string, unknown][] = [
    ["pull_request", { inputs: { run_upgrade_smoke_arm: "true" } }],
    ["push", {}],
    ["merge_group", {}],
    ["workflow_dispatch", { inputs: { run_upgrade_smoke_arm: "false" } }],
    ["workflow_dispatch", {}],
  ];
  for (const [name, event] of cases)
    expect(
      gate(forced, "install", { "upgrade-smoke-arm64": "success" }, { event_name: name, event }),
    ).toBe(1);
  expect(
    gate(
      gatePlan("install", {}),
      "install",
      {},
      { event_name: "workflow_dispatch", event: { inputs: { run_upgrade_smoke_arm: "true" } } },
    ),
  ).toBe(1);
});
test("AgentDocsGateTest.test_opt_in_gate_malformed_event_rejected", () => {
  const p = gatePlan("install", {});
  const cases: [string, unknown][] = [
    ["workflow_dispatch", "{bad"],
    ["workflow_dispatch", []],
    ["workflow_dispatch", { inputs: [] }],
    ["workflow_dispatch", { inputs: { run_upgrade_smoke_arm: "yes" } }],
    ["workflow_dispatch", { inputs: { run_upgrade_smoke_arm: 1 } }],
    ["workflow_dispatch", { inputs: { other: "true" } }],
    ["schedule", {}],
  ];
  for (const [name, event] of cases)
    expect(gate(p, "install", {}, { event_name: name, event })).toBe(1);
  for (const missing of ["GITHUB_EVENT_NAME", "GITHUB_EVENT_PATH"])
    withEnv({ GITHUB_EVENT_NAME: "push", GITHUB_EVENT_PATH: "/nonexistent" }, () => {
      delete process.env[missing];
      expect(
        runCli([
          "gate",
          "--workflow",
          "install",
          "--needs-json",
          needs(p, "install"),
          "--tested-sha",
          "a".repeat(40),
        ]).returncode,
      ).toBe(1);
    });
});
test("AgentDocsGateTest.test_docs_plan_all_skipped_passes", () => {
  for (const workflow of Object.keys(S.WORKFLOW_JOBS))
    expect(gate(gatePlan(workflow, {}), workflow)).toBe(0);
});
test("AgentDocsGateTest.test_docs_plan_unselected_ran_rejected", () => {
  for (const [workflow, jobs] of Object.entries(S.WORKFLOW_JOBS))
    for (const job of jobs)
      for (const result of ["success", "failure", "cancelled"])
        expect(gate(gatePlan(workflow, {}), workflow, { [job]: result })).toBe(1);
});
test("AgentDocsGateTest.test_full_plan_selected_bad_result_rejected", () => {
  for (const [workflow, jobs] of Object.entries(S.WORKFLOW_JOBS)) {
    const p = gatePlan(workflow, Object.fromEntries(jobs.map((job) => [job, true])));
    p.mode = "full";
    p.reason_code = "FULL_PATH_BROADEN";
    const ok = Object.fromEntries(jobs.map((job) => [job, "success"]));
    expect(gate(p, workflow, ok)).toBe(0);
    for (const job of jobs) {
      for (const result of ["failure", "cancelled", "skipped"])
        expect(gate(p, workflow, { ...ok, [job]: result })).toBe(1);
      expect(gate(p, workflow, ok, { omit_jobs: new Set([job]) })).toBe(1);
    }
  }
});
