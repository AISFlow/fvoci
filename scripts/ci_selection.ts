// A-only port of main 93534e2831aea473523674fc458015dfbd375cb1.
// Existing Python callers and workflow contracts remain authoritative until B.
import { readFileSync, writeFileSync, readdirSync, existsSync, statSync } from "node:fs";
import { resolve, join, basename, extname } from "node:path";
import { parseArgs } from "node:util";

export const ROOT = resolve(import.meta.dir, "..");
export type Mapping = Record<string, unknown>;
export function isMapping(value: unknown): value is Mapping {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
const mapping = (value: unknown): Mapping => (isMapping(value) ? value : {});
const strings = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((v: unknown) => typeof v === "string");
const keysEqual = (value: Mapping, keys: Iterable<string>) =>
  Bun.deepEquals(Object.keys(value).sort(), [...keys].sort());
const same = (a: unknown, b: unknown) => Bun.deepEquals(a, b);
const read = (path: string) => readFileSync(path, "utf8");
const isFile = (path: string) => existsSync(path) && statSync(path).isFile();
const isDir = (path: string) => existsSync(path) && statSync(path).isDirectory();
const difference = (a: ReadonlySet<string>, b: ReadonlySet<string>) =>
  new Set([...a].filter((v) => !b.has(v)));
const intersect = (a: ReadonlySet<string>, b: ReadonlySet<string>) =>
  new Set([...a].filter((v) => b.has(v)));
// Python's diagnostic quote convention, restricted to the scalar/list values in this policy.
function repr(value: unknown): string {
  if (value === null || value === undefined) return "None";
  if (value === true) return "True";
  if (value === false) return "False";
  if (typeof value === "string") {
    if (value.includes("'") && !value.includes('"')) return JSON.stringify(value);
    return "'" + JSON.stringify(value).slice(1, -1).replace(/\\"/g, '"').replace(/'/g, "\\'") + "'";
  }
  if (Array.isArray(value)) return "[" + value.map((v: unknown) => repr(v)).join(", ") + "]";
  return JSON.stringify(value);
}
export const PLAN_VERSION = 3;
export const WORKFLOW_JOBS: Record<string, readonly string[]> = {
  web: ["web-static", "web-checks", "workspace-browser-shard", "collaboration-flow"],
  rust: ["fast", "postgres", "collaboration"],
  documents: ["native-extraction"],
  "collab-engine": ["native-collab-engine"],
  install: ["install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"],
};
export const OPT_IN_JOBS: Record<string, Record<string, string>> = {
  install: { "upgrade-smoke-arm64": "run_upgrade_smoke_arm" },
};
export const OPT_IN_RUNNER: Record<string, string> = { "upgrade-smoke-arm64": "ubuntu-24.04-arm" };
export const WORKFLOW_YAML: Record<string, string> = {
  web: "web.yml",
  rust: "rust.yml",
  documents: "documents.yml",
  "collab-engine": "collab-engine.yml",
  install: "install.yml",
};
export const RELEASE_WORKFLOW_FILE = "release.yml";
export const RELEASE_WRITE_SCOPES: Record<string, ReadonlySet<string>> = {
  build: new Set(["packages"]),
  index: new Set(["packages"]),
  publish: new Set(["packages"]),
  release: new Set(["contents"]),
};
export const CI_BASE_WORKFLOW_FILE = "ci-base-image.yml";
export const CI_BASE_WRITE_SCOPES: Record<string, ReadonlySet<string>> = {
  build: new Set(),
  push: new Set(["packages"]),
  "push-manifest": new Set(["packages"]),
};
export const PLAN_JOB_ID = "ci-plan";
export const PLAN_OUTPUT_KEYS = ["mode", "reason_code", "plan_ok", "plan_json"];
export const PYYAML_PIN = "PyYAML==6.0.3";
export const REQUIREMENTS_FILE = "scripts/ci_selection_requirements.txt";
export const SELECTOR_REGRESSION_WRAPPER = "scripts/test-ci-selection.sh";
export const GATE_NEEDS_JSON_EXPR = "${{ toJSON(needs) }}";
export const GATE_TESTED_SHA_EXPR = "${{ github.sha }}";
export const KNOWN_EVENTS = new Set(["pull_request", "push", "merge_group", "workflow_dispatch"]);
export const ALWAYS_FULL_EVENTS = new Set(["push", "merge_group", "workflow_dispatch"]);
export const ALLOWED_PLAN_KEYS = new Set([
  "version",
  "workflow",
  "mode",
  "reason_code",
  "plan_ok",
  "base_sha",
  "head_sha",
  "merge_base_sha",
  "tested_sha",
  "path_count",
  "jobs",
]);
export const BROADEN_PREFIXES = [
  ".github/",
  "migrations/",
  "src/",
  "tests/",
  "scripts/",
  "vendor/",
  "compat/",
  "infra/",
  ".agents/",
  "packages/",
  "crates/",
  "patches/",
];
export const BROADEN_EXACT = new Set([
  "Cargo.toml",
  "Cargo.lock",
  "rust-toolchain.toml",
  "Dockerfile",
  ".dockerignore",
  "package.json",
  "bun.lock",
  "eslint.config.mjs",
  ".prettierrc.json",
  ".prettierignore",
  "bunfig.toml",
  ".bun-version",
]);
export const MANIFEST_MARKERS = [
  "/package.json",
  "/package-lock.json",
  "/bun.lock",
  "/Cargo.toml",
  "/Cargo.lock",
  "/pnpm-lock.yaml",
  "/yarn.lock",
];
export const EXPLICIT_DOCS = new Set([
  "README.md",
  "RUNNING.md",
  "docs/rewrite.md",
  "docs/RELEASING.md",
  "docs/collab-engine-comparison.md",
  "AGENTS.md",
  ".agents/environment.md",
]);
const WEB_BROADEN_PREFIXES = [
  "apps/web/openapi.json",
  "apps/web/src/generated/",
  "apps/web/package.json",
  "apps/web/playwright.config.ts",
];
const EDITOR_UI_PREFIXES = ["packages/editor/src/react/", "packages/editor/src/vue/"];
const EDITOR_UI_EXACT = new Set([
  "packages/editor/src/clipboard.ts",
  "packages/editor/src/gutter-actions.ts",
  "packages/editor/src/menu-roving.ts",
]);
const UI_SUFFIXES = [".ts", ".tsx", ".vue", ".css"];
const BROWSER_UI_HELPERS = new Set([
  "apps/web/e2e/helpers.ts",
  "apps/web/e2e/mfa-helpers.ts",
  "apps/web/e2e/workspace-wiki-vue-editor.ts",
  "apps/web/e2e-pending/collab-helpers.ts",
  "apps/web/e2e-pending/collab-helpers.test.ts",
]);
export type NarrowFamily = "docs" | "frontend_web_install" | "web_tests";
export type Mode = "full" | "narrow";
export function validateSha(ref: string): boolean {
  return /^[0-9a-f]{40}$/.test(ref);
}
export function sanitizeReasonCode(code: string): string {
  if (!/^[A-Z][A-Z0-9_]{0,63}$/.test(code)) throw new Error(`unsafe reason code: ${repr(code)}`);
  return code;
}
export const selectOutputKey = (job: string) => `select_${job.replaceAll("-", "_")}`;
export const gateJobId = (workflow: string) => `${workflow}-ci-gate`;
export const expectedSelectIf = (job: string) =>
  `needs.${PLAN_JOB_ID}.outputs.${selectOutputKey(job)} == 'true'`;
export const canonicalGateRun = (workflow: string) =>
  `set -euo pipefail\npython3 scripts/ci_selection.py gate --workflow ${workflow} --needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n`;
export const normalizeRunScript = (text: string) => text.replaceAll("\r\n", "\n").trim() + "\n";
export const scriptLines = (text: string) =>
  text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
export function classifyPath(path: string): NarrowFamily | "broaden" | "unknown" {
  if (
    !path ||
    path.includes("\\") ||
    path.split("/").some((part) => ["", ".", ".."].includes(part))
  )
    return "unknown";
  if (EXPLICIT_DOCS.has(path)) return "docs";
  if (
    /^apps\/web\/(?:e2e|e2e-pending)\/[^/]+\.spec\.ts$/.test(path) ||
    BROWSER_UI_HELPERS.has(path)
  )
    return "web_tests";
  if (path === "packages/editor/src/react/schema.tsx") return "broaden";
  if (
    EDITOR_UI_EXACT.has(path) ||
    (EDITOR_UI_PREFIXES.some((p) => path.startsWith(p)) &&
      UI_SUFFIXES.some((s) => path.endsWith(s)))
  )
    return "frontend_web_install";
  if (/^packages\/editor\/test\/[^/]+\.test\.ts$/.test(path)) return "web_tests";
  if (
    BROADEN_EXACT.has(path) ||
    BROADEN_PREFIXES.some((p) => path.startsWith(p)) ||
    MANIFEST_MARKERS.some((m) => path.includes(m))
  )
    return "broaden";
  if (path.startsWith("docs/") || WEB_BROADEN_PREFIXES.some((p) => path.startsWith(p)))
    return "broaden";
  if (path.startsWith("apps/web/src/")) {
    if (!UI_SUFFIXES.some((s) => path.endsWith(s))) return "broaden";
    return path.endsWith(".test.ts") || path.endsWith(".test.tsx")
      ? "web_tests"
      : "frontend_web_install";
  }
  return path.startsWith("apps/web/") ? "broaden" : "unknown";
}
export function parseNameStatusZ(data: Uint8Array): [string[], string | null] {
  if (data.length && data.at(-1) !== 0) return [[], "DIFF_TRUNCATED"];
  const fields = new TextDecoder("utf-8", { fatal: true }).decode(data).split("\0");
  if (fields.at(-1) === "") fields.pop();
  const paths: string[] = [];
  let i = 0;
  while (i < fields.length) {
    const status = fields[i++] ?? "";
    if (!status) return [[], "DIFF_EMPTY_STATUS"];
    if (!/^[ACDMRTU][0-9]*$/.test(status)) return [[], "DIFF_BAD_STATUS"];
    if (status[0] === "R" || status[0] === "C") {
      if (i + 1 >= fields.length) return [[], "DIFF_TRUNCATED_RENAME"];
      paths.push(fields[i++] ?? "", fields[i++] ?? "");
    } else {
      if (i >= fields.length)
        return [[], status[0] === "D" ? "DIFF_TRUNCATED_DELETE" : "DIFF_TRUNCATED_PATH"];
      paths.push(fields[i++] ?? "");
    }
  }
  return [paths, null];
}
export interface GitResult {
  returncode: number;
  stdout: Uint8Array;
  stderr: Uint8Array;
}
export const gitOperations = {
  run(repo: string, ...args: string[]): GitResult {
    // Bun's sync declaration omits the null exit code observed on signal termination.
    const p: { exitCode: number | null; stdout: Uint8Array; stderr: Uint8Array } = Bun.spawnSync(
      ["git", ...args],
      { cwd: repo, stdout: "pipe", stderr: "pipe" },
    );
    return { returncode: p.exitCode ?? -1, stdout: p.stdout, stderr: p.stderr };
  },
};
const gitText = (p: GitResult) => new TextDecoder().decode(p.stdout).trim();
export function gitRevParse(
  repo: string,
  ref: string,
  requireShaRef = true,
): [string | null, string | null] {
  if (requireShaRef && !validateSha(ref)) return [null, "SHA_INVALID"];
  const p = gitOperations.run(repo, "rev-parse", ref);
  if (p.returncode !== 0) return [null, "REV_PARSE_FAILED"];
  const sha = gitText(p);
  return validateSha(sha) ? [sha, null] : [null, "SHA_INVALID"];
}
export function gitMergeBase(repo: string, a: string, b: string): [string | null, string | null] {
  const p = gitOperations.run(repo, "merge-base", a, b);
  if (p.returncode !== 0) return [null, "MERGE_BASE_FAILED"];
  const sha = gitText(p);
  return validateSha(sha) ? [sha, null] : [null, "SHA_INVALID"];
}
export function gitDiffPaths(repo: string, base: string, head: string): [string[], string | null] {
  const p = gitOperations.run(repo, "diff", "--name-status", "-z", "-M", base, head);
  return p.returncode !== 0 ? [[], "GIT_DIFF_FAILED"] : parseNameStatusZ(p.stdout);
}
export function gitFetchOrigin(repo: string, ...refs: string[]): string | null {
  if (refs.some((ref) => !validateSha(ref))) return "SHA_INVALID";
  return gitOperations.run(repo, "fetch", "--no-tags", "origin", ...refs).returncode !== 0
    ? "FETCH_FAILED"
    : null;
}
// Explicit seams preserve the original tests' function-specific failure injection.
export const selectionGit = { diffPaths: gitDiffPaths, fetchOrigin: gitFetchOrigin };
export function diffPathsForPr(
  repo: string,
  base: string,
  head: string,
): [string[] | null, string | null, string | null] {
  const [a, ae] = gitRevParse(repo, base);
  if (ae || !a) return [null, ae, null];
  const [b, be] = gitRevParse(repo, head);
  if (be || !b) return [null, be, null];
  const [mb, me] = gitMergeBase(repo, a, b);
  if (me || !mb) return [null, me, null];
  const [paths, e] = selectionGit.diffPaths(repo, mb, b);
  return e ? [null, e, mb] : [paths, null, mb];
}
export const gitObjectExists = (repo: string, sha: string) =>
  gitOperations.run(repo, "cat-file", "-e", `${sha}^{commit}`).returncode === 0;
export function ensureCommitShas(repo: string, ...shas: string[]): string | null {
  if (shas.some((sha) => !validateSha(sha))) return "SHA_INVALID";
  const missing = shas.filter((sha) => !gitObjectExists(repo, sha));
  return missing.length ? selectionGit.fetchOrigin(repo, ...missing) : null;
}
export function gitCommitParents(repo: string, sha: string): [string[] | null, string | null] {
  if (!validateSha(sha)) return [null, "SHA_INVALID"];
  const p = gitOperations.run(repo, "rev-list", "--parents", "-n", "1", sha);
  if (p.returncode !== 0) return [null, "REV_LIST_PARENTS_FAILED"];
  const parts = gitText(p).split(/\s+/).filter(Boolean);
  if (!parts.length) return [null, "REV_LIST_PARENTS_EMPTY"];
  if (parts[0] !== sha) return [null, "REV_LIST_COMMIT_MISMATCH"];
  const parents = parts.slice(1);
  return parents.some((v) => !validateSha(v)) ? [null, "SHA_INVALID"] : [parents, null];
}
export function prCheckoutNarrowBlock(
  repo: string,
  tested: string,
  base: string,
  head: string,
): string | null {
  const [parents, e] = gitCommitParents(repo, tested);
  if (e || !parents) return e;
  if (parents.length !== 2) return "FULL_PR_CHECKOUT_NOT_MERGE";
  if (parents[1] !== head) return "FULL_PR_MERGE_PARENTS_MISMATCH";
  if (
    parents[0] !== base &&
    gitOperations.run(repo, "merge-base", "--is-ancestor", base, parents[0] ?? "").returncode !== 0
  )
    return "FULL_PR_MERGE_PARENTS_MISMATCH";
  return null;
}
export interface SelectionDecision {
  mode: Mode;
  reason_code: string;
  families: ReadonlySet<NarrowFamily>;
}
export function decideFromPaths(paths: string[]): SelectionDecision {
  if (!paths.length) return { mode: "full", reason_code: "FULL_EMPTY_DIFF", families: new Set() };
  const families = new Set<NarrowFamily>();
  for (const path of paths) {
    const kind = classifyPath(path);
    if (kind === "broaden" || kind === "unknown")
      return {
        mode: "full",
        reason_code: kind === "broaden" ? "FULL_PATH_BROADEN" : "FULL_UNKNOWN_PATH",
        families: new Set(),
      };
    families.add(kind);
  }
  return {
    mode: "narrow",
    reason_code: families.has("frontend_web_install")
      ? "NARROW_FRONTEND_WEB_INSTALL"
      : families.has("web_tests")
        ? "NARROW_WEB_TESTS"
        : "NARROW_DOCS",
    families,
  };
}
export function workflowJobSelected(
  workflow: string,
  job: string,
  decision: SelectionDecision,
): boolean {
  if (job in (OPT_IN_JOBS[workflow] ?? {})) return false;
  return (
    decision.mode === "full" ||
    (["web", "install"].includes(workflow) && decision.families.has("frontend_web_install")) ||
    (workflow === "web" && decision.families.has("web_tests"))
  );
}
export interface Plan {
  version: number;
  workflow: string;
  mode: Mode;
  reason_code: string;
  plan_ok: boolean;
  base_sha: string | null;
  head_sha: string | null;
  merge_base_sha: string | null;
  tested_sha: string | null;
  path_count: number;
  jobs: Record<string, { selected: boolean }>;
}
export interface PlanInputs {
  workflow: string;
  event_name: string;
  base_sha: string | null;
  head_sha: string | null;
  merge_base_sha: string | null;
  tested_sha: string | null;
  paths: string[] | null;
  fatal_error?: string | null;
  force_full_reason?: string | null;
  opt_in_inputs?: ReadonlySet<string>;
}
export function buildPlan(input: PlanInputs): Plan {
  const { workflow, event_name, paths } = input;
  if (!(workflow in WORKFLOW_JOBS)) throw new Error(`unknown workflow: ${workflow}`);
  let decision: SelectionDecision;
  let plan_ok = true;
  const full = (reason_code: string): SelectionDecision => ({
    mode: "full",
    reason_code: sanitizeReasonCode(reason_code),
    families: new Set(),
  });
  if (input.fatal_error) {
    decision = full(input.fatal_error);
    plan_ok = false;
  } else if (ALWAYS_FULL_EVENTS.has(event_name))
    decision = full(`FULL_EVENT_${event_name.toUpperCase()}`);
  else if (!KNOWN_EVENTS.has(event_name)) {
    decision = full("FULL_EVENT_UNKNOWN");
    plan_ok = false;
  } else if (input.force_full_reason) decision = full(input.force_full_reason);
  else if (paths === null) {
    decision = full("FULL_MISSING_PATHS");
    plan_ok = false;
  } else decision = decideFromPaths(paths);
  const jobs = Object.fromEntries(
    (WORKFLOW_JOBS[workflow] ?? []).map((job) => [
      job,
      { selected: workflowJobSelected(workflow, job, decision) },
    ]),
  );
  if (plan_ok && event_name === "workflow_dispatch")
    for (const [job, name] of Object.entries(OPT_IN_JOBS[workflow] ?? {}))
      if (input.opt_in_inputs?.has(name)) {
        const entry = jobs[job];
        if (entry) entry.selected = true;
      }
  return {
    version: PLAN_VERSION,
    workflow,
    mode: decision.mode,
    reason_code: decision.reason_code,
    plan_ok,
    base_sha: input.base_sha,
    head_sha: input.head_sha,
    merge_base_sha: input.merge_base_sha,
    tested_sha: input.tested_sha,
    path_count: paths?.length ?? 0,
    jobs,
  };
}
export function eventShas(event: Mapping, name: string): [string | null, string | null] {
  const e =
    name === "pull_request"
      ? mapping(event.pull_request)
      : name === "merge_group"
        ? mapping(event.merge_group)
        : event;
  const a =
    name === "pull_request"
      ? mapping(e.base).sha
      : name === "merge_group"
        ? e.base_sha
        : name === "push"
          ? e.before
          : null;
  const b =
    name === "pull_request"
      ? mapping(e.head).sha
      : name === "merge_group"
        ? e.head_sha
        : name === "push"
          ? e.after
          : null;
  return [typeof a === "string" ? a : null, typeof b === "string" ? b : null];
}
export function loadEvent(path: string): unknown {
  return JSON.parse(read(path)) as unknown;
}
export function dispatchOptIns(
  workflow: string,
  eventName: string,
  event: unknown,
): [ReadonlySet<string>, string | null] {
  const chosen = new Set<string>();
  if (eventName !== "workflow_dispatch") return [chosen, null];
  if (!isMapping(event)) return [chosen, "DISPATCH_EVENT_INVALID"];
  const raw = event.inputs;
  if (raw === undefined || raw === null) return [chosen, null];
  if (!isMapping(raw)) return [chosen, "DISPATCH_INPUTS_INVALID"];
  const allowed = new Set(Object.values(OPT_IN_JOBS[workflow] ?? {}));
  if (Object.keys(raw).some((k) => !allowed.has(k))) return [chosen, "DISPATCH_INPUTS_UNKNOWN"];
  for (const [name, v] of Object.entries(raw)) {
    if (v === true || v === "true") chosen.add(name);
    else if (v !== false && v !== "false") return [new Set(), "DISPATCH_INPUT_VALUE_INVALID"];
  }
  return [chosen, null];
}
export interface ResolvedInputs {
  paths: string[] | null;
  fatal_error: string | null;
  force_full_reason: string | null;
  base_sha: string | null;
  head_sha: string | null;
  merge_base_sha: string | null;
  tested_sha: string | null;
}
export function resolveSelectionInputs(repo: string, event: Mapping, name: string): ResolvedInputs {
  const tested = (process.env.GITHUB_SHA ?? "").trim();
  const result = (
    fatal_error: string | null = null,
    force_full_reason: string | null = null,
    base_sha: string | null = null,
    head_sha: string | null = null,
    merge_base_sha: string | null = null,
    paths: string[] | null = null,
    tested_sha: string | null = tested,
  ): ResolvedInputs => ({
    paths,
    fatal_error,
    force_full_reason,
    base_sha,
    head_sha,
    merge_base_sha,
    tested_sha,
  });
  if (!validateSha(tested)) return result("TESTED_SHA_INVALID", null, null, null, null, null, null);
  const [now, e] = gitRevParse(repo, "HEAD", false);
  if (e) return result("HEAD_REV_PARSE_FAILED");
  if (now !== tested) return result("TESTED_SHA_MISMATCH");
  if (name === "workflow_dispatch") return result();
  if (!KNOWN_EVENTS.has(name)) return result("EVENT_UNKNOWN");
  const [base, head] = eventShas(event, name);
  if (name === "push" || name === "merge_group") return result(null, null, base, head);
  if (!base || !head) return result("MISSING_BASE_OR_HEAD", null, base, head);
  if (!validateSha(base) || !validateSha(head)) return result("SHA_INVALID", null, base, head);
  const fe = ensureCommitShas(repo, base, head);
  if (fe) return result(fe, null, base, head);
  const block = prCheckoutNarrowBlock(repo, tested, base, head);
  if (block) return result(null, block, base, head);
  const [paths, de, mb] = diffPathsForPr(repo, base, head);
  if (de) return result(de, null, base, head, mb);
  const [parents, pe] = gitCommitParents(repo, tested);
  if (pe || !parents) return result(pe, null, base, head, mb);
  const [merged, me] = selectionGit.diffPaths(repo, parents[0] ?? "", tested);
  if (me) return result(me, null, base, head, mb);
  return result(null, null, base, head, mb, [...new Set([...(paths ?? []), ...merged])].sort());
}
export function writeGithubOutputs(plan: Plan, path: string | null): void {
  if (path === null) return;
  sanitizeReasonCode(plan.reason_code);
  const lines = [
    `mode=${plan.mode}`,
    `reason_code=${plan.reason_code}`,
    `plan_ok=${String(plan.plan_ok)}`,
  ];
  for (const [job, meta] of Object.entries(plan.jobs)) {
    if (typeof meta.selected !== "boolean") throw new Error("job.selected must be bool");
    lines.push(`${selectOutputKey(job)}=${String(meta.selected)}`);
  }
  // JSON.stringify's standard replacer controls Python's sort_keys output without a serializer.
  const payload = JSON.stringify(
    plan,
    [...new Set([...Object.keys(plan), ...Object.keys(plan.jobs), "selected"])].sort(),
  );
  writeFileSync(path, lines.join("\n") + `\nplan_json<<PLAN_EOF\n${payload}\nPLAN_EOF\n`);
}

export function loadYamlMapping(path: string): [Mapping | null, string | null] {
  try {
    const value: unknown = Bun.YAML.parse(read(path));
    return isMapping(value)
      ? [value, null]
      : [null, `${basename(path)}: workflow YAML must be a mapping`];
  } catch (error) {
    return [
      null,
      `${basename(path)}: YAML parse failed: ${error instanceof Error ? error.message : String(error)}`,
    ];
  }
}
export function needsList(job: Mapping): [string[] | null, string | null] {
  const value = job.needs;
  if (value === undefined || value === null) return [[], null];
  if (typeof value === "string") return [[value], null];
  return strings(value) ? [[...value], null] : [null, "needs must be a string or list of strings"];
}
export function runSteps(job: Mapping): Mapping[] {
  return Array.isArray(job.steps)
    ? job.steps.filter((v: unknown): v is Mapping => isMapping(v) && typeof v.run === "string")
    : [];
}
export const runScripts = (job: Mapping): string[] => runSteps(job).map((step) => String(step.run));
export function listWorkflowFiles(root: string): string[] {
  const dir = join(root, ".github/workflows");
  return isDir(dir)
    ? readdirSync(dir)
        .filter((v) => [".yml", ".yaml"].includes(extname(v)) && isFile(join(dir, v)))
        .sort()
        .map((v) => join(dir, v))
    : [];
}
export const RUST_WORKFLOW_FILE = "rust.yml";
export const RUST_COLLAB_CI_SCRIPT = "scripts/run-rust-collaboration-ci-tests.sh";
export const RUST_CAPACITY_PROBE_SCRIPT = "scripts/collab-capacity-probe.sh";
export const RUST_POSTGRES_RUNNER_ARCH: Record<string, string> = {
  "ubuntu-24.04": "x64",
  "ubuntu-24.04-arm": "arm64",
};
export const RUST_INTEGRATION_MANUAL_TARGETS = new Set(["collab_capacity_probe"]);
export const RUST_DB_TESTS_FEATURE = "db-tests";
export const RUST_POSTGRES_INTEGRATION_STEP = "PostgreSQL integration tests";
export const RUST_S3_INTEGRATION_STEP =
  "S3-compatible storage integration tests (pinned test server)";
export const RUST_COLLAB_INTEGRATION_STEP =
  "WebSocket, PostgreSQL and native helper integration tests";
export const RUST_COLLAB_INTEGRATION_RUN = "bash scripts/run-rust-collaboration-ci-tests.sh";
export const RUST_COLLAB_MATRIX_RUNNERS = new Set(["ubuntu-24.04", "ubuntu-24.04-arm"]);
export const RUST_S3_INTEGRATION_STEP_IF = "matrix.shard == 'b'";
export const RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS = new Set([
  "collab_wire",
  "markdown_process",
  "docx_export_process",
  "pdf_export_process",
  "pptx_export_process",
  "doctor_conversion",
  "office_extract_process",
  "static_api",
]);
const RUST_POSTGRES_INTEGRATION_RUN_CANONICAL =
  "cargo test --locked --offline --no-fail-fast --features db-tests ${{ matrix.tests }}";
const RUST_S3_INTEGRATION_RUN_CANONICAL =
  "bash scripts/start-test-minio.sh cargo test --locked --offline --no-fail-fast --features db-tests --test attachment_s3_integration";
export const RUST_COLLAB_LIBTEST_ARGS = new Set(["--nocapture", "--test-threads=1"]);
const cargoName = (v: string) => /^[A-Za-z0-9_-]+$/.test(v);
function crateAttributes(path: string): string[] {
  return read(path)
    .split(/\r?\n/)
    .map((v) => v.trim())
    .filter((v) => v.startsWith("#!["));
}
export function rootDbIntegrationRegistryTargets(
  root: string,
): [Set<string> | null, string | null] {
  const path = join(root, "Cargo.toml");
  if (!isFile(path)) return [null, "rust: missing root Cargo.toml"];
  let data: Mapping;
  try {
    data = mapping(Bun.TOML.parse(read(path)));
  } catch (error) {
    return [
      null,
      `rust: Cargo.toml parse failed: ${error instanceof Error ? error.message : String(error)}`,
    ];
  }
  const targets = new Set<string>();
  const entries: unknown[] = Array.isArray(data.test) ? data.test : [];
  for (const entry of entries) {
    if (!isMapping(entry)) return [null, "rust: Cargo.toml [[test]] entry must be a table"];
    const name = entry.name;
    if (typeof name !== "string" || !name) return [null, "rust: Cargo.toml [[test]] missing name"];
    const features = entry["required-features"] ?? [];
    if (!strings(features))
      return [null, `rust: Cargo.toml [[test]] ${name} required-features must be a string list`];
    if (features.includes(RUST_DB_TESTS_FEATURE)) targets.add(name);
  }
  const pkg = mapping(data.package);
  const autotests = !("autotests" in pkg) || Boolean(pkg.autotests);
  const dir = join(root, "tests");
  if (autotests && isDir(dir))
    for (const name of readdirSync(dir)
      .filter((v) => v.endsWith(".rs") && isFile(join(dir, v)))
      .sort()) {
      const path = join(dir, name),
        stem = name.slice(0, -3),
        attrs = crateAttributes(path);
      let declares = false;
      for (const attr of attrs) {
        if (attr.includes("extract-native-tests")) {
          declares = false;
          break;
        }
        if (attr.includes('feature = "db-tests"') || attr.includes('feature="db-tests"')) {
          declares = true;
          break;
        }
      }
      if (declares) {
        targets.add(stem);
        continue;
      }
      if (
        attrs.some((v) => v.includes("extract-native-tests")) ||
        RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS.has(stem)
      )
        continue;
      if (attrs.length || read(path).trim())
        return [
          null,
          `rust: tests/${stem}.rs is not registered and has no crate #![cfg(feature = "db-tests")]; add CI inventory or an explicit fast/native exclusion`,
        ];
    }
  return [targets, null];
}
export const cargoTestFlagsInText = (text: string): Set<string> =>
  new Set([...text.matchAll(/(?:^|\s)--test\s+([A-Za-z0-9_-]+)/g)].map((v) => v[1] ?? ""));
function rustWorkflowJobs(root: string): [Mapping | null, string | null] {
  const path = join(root, ".github/workflows", RUST_WORKFLOW_FILE);
  if (!isFile(path)) return [null, `rust: missing workflow file ${RUST_WORKFLOW_FILE}`];
  const [data, e] = loadYamlMapping(path);
  if (e) return [null, `rust: ${e}`];
  const jobs = data?.jobs;
  return isMapping(jobs) ? [jobs, null] : [null, "rust: jobs mapping missing"];
}
function postgresRows(job: Mapping): [Mapping[] | null, string | null] {
  if (!isMapping(job.strategy)) return [null, "rust: postgres job strategy missing"];
  if (!isMapping(job.strategy.matrix)) return [null, "rust: postgres job matrix missing"];
  const rows = job.strategy.matrix.include;
  if (!Array.isArray(rows) || !rows.length)
    return [null, "rust: postgres job matrix.include missing"];
  if (!rows.every((v: unknown) => isMapping(v)))
    return [null, "rust: postgres matrix.include row must be a mapping"];
  return [rows, null];
}
const collapseShellWords = (text: string) => text.trim().split(/\s+/).join(" ");
export function cargoCommandSuppressionError(
  norm: string,
  context: string,
  allowed: ReadonlySet<string> = new Set(["--nocapture"]),
): string | null {
  if (norm.includes("--no-run")) return `rust: ${context} must not use --no-run`;
  if (norm.includes("--exclude")) return `rust: ${context} must not use --exclude`;
  for (const op of ["||", "&&", "|", ";", "&"])
    if (norm.includes(op)) return `rust: ${context} must not contain shell operator ${repr(op)}`;
  const pos = norm.indexOf(" -- ");
  if (pos >= 0 && !allowed.has(norm.slice(pos + 4).trim()))
    return `rust: ${context} must not use libtest filter after --`;
  return null;
}
export function validateMatrixTestsFragment(text: string): string | null {
  const trimmed = text.trim();
  if (!trimmed) return "rust: postgres matrix row missing tests command fragment";
  const e = cargoCommandSuppressionError(trimmed, "postgres matrix tests");
  if (e) return e;
  const tokens = trimmed.split(/\s+/),
    err = "rust: postgres matrix tests must be --test NAME pairs only";
  if (tokens.length % 2) return err;
  for (let i = 0; i < tokens.length; i += 2)
    if (tokens[i] !== "--test" || !cargoName(tokens[i + 1] ?? "")) return err;
  return null;
}
export function validateCargoTestInvocation(
  tokens: string[],
  context: string,
  requireTests: boolean,
): string | null {
  const err = `rust: ${context} must invoke cargo test with --features db-tests`;
  if (tokens.length < 2 || tokens[0] !== "cargo" || tokens[1] !== "test") return err;
  let i = 2,
    locked = false,
    offline = false,
    nofail = false,
    features = false,
    tests = false;
  while (i < tokens.length) {
    const t = tokens[i];
    if (t === "--locked") {
      locked = true;
      i++;
      continue;
    }
    if (t === "--offline") {
      offline = true;
      i++;
      continue;
    }
    if (t === "--no-fail-fast") {
      nofail = true;
      i++;
      continue;
    }
    if (t === "--features") {
      if (tokens[i + 1] !== RUST_DB_TESTS_FEATURE) return err;
      features = true;
      i += 2;
      continue;
    }
    if (t === "--test") {
      if (!cargoName(tokens[i + 1] ?? ""))
        return `rust: ${context} must use --test NAME pairs only`;
      tests = true;
      i += 2;
      continue;
    }
    if (t === "${{" && tokens[i + 1] === "matrix.tests" && tokens[i + 2] === "}}") {
      i += 3;
      continue;
    }
    return `rust: ${context} must not use unknown cargo test flag ${repr(t)}`;
  }
  if (!locked || !offline || !nofail || !features) return err;
  return requireTests && !tests ? `rust: ${context} must declare at least one --test target` : null;
}
export function postgresMatrixInventory(
  jobs: Mapping,
): [Record<string, Set<string>>, string | null] {
  const job = jobs.postgres;
  if (!isMapping(job)) return [{}, "rust: postgres job missing"];
  const [rows, e] = postgresRows(job);
  if (e || !rows) return [{}, e];
  const per: Record<string, Set<string>> = { x64: new Set(), arm64: new Set() };
  for (const row of rows) {
    const runner = row.runner,
      text = row.tests;
    if (typeof runner !== "string" || !(runner in RUST_POSTGRES_RUNNER_ARCH))
      return [{}, `rust: postgres matrix row has unknown runner ${repr(runner)}`];
    if (typeof text !== "string")
      return [{}, "rust: postgres matrix row missing tests command fragment"];
    const e = validateMatrixTestsFragment(text);
    if (e) return [{}, e];
    const bucket = per[RUST_POSTGRES_RUNNER_ARCH[runner] ?? ""];
    for (const target of cargoTestFlagsInText(text)) bucket?.add(target);
  }
  return [per, null];
}
function postgresSteps(jobs: Mapping): [unknown[] | null, string | null] {
  const job = jobs.postgres;
  if (!isMapping(job)) return [null, "rust: postgres job missing"];
  return Array.isArray(job.steps) ? [job.steps, null] : [null, "rust: postgres job steps missing"];
}
function uniqueNamedStep(
  steps: unknown[],
  name: string,
  job: string,
): [Mapping | null, string | null] {
  const matches = steps.filter((v): v is Mapping => isMapping(v) && v.name === name);
  if (!matches.length) return [null, `rust: missing ${job} step ${repr(name)}`];
  if (matches.length !== 1)
    return [null, `rust: ${job} step ${repr(name)} must appear exactly once`];
  return [matches[0] ?? null, null];
}
function executionStepMasked(step: Mapping, job: string, name: string): string | null {
  return "continue-on-error" in step && step["continue-on-error"] !== false
    ? `rust: ${job} step ${repr(name)} must not use continue-on-error`
    : null;
}
function verifyIntegrationRun(run: string, s3: boolean): string | null {
  const norm = normalizeRunScript(run).trim();
  const err = s3
    ? "rust: S3 integration step must invoke start-test-minio.sh with a db-tests cargo test"
    : "rust: PostgreSQL integration step must execute cargo test with --features db-tests and ${{ matrix.tests }}";
  if (norm.startsWith("echo ")) return err;
  const e = cargoCommandSuppressionError(
    norm,
    s3 ? "S3 integration step" : "PostgreSQL integration step",
  );
  if (e) return e;
  return collapseShellWords(norm) ===
    (s3 ? RUST_S3_INTEGRATION_RUN_CANONICAL : RUST_POSTGRES_INTEGRATION_RUN_CANONICAL)
    ? null
    : err;
}
export function verifyPostgresIntegrationExecution(jobs: Mapping): string[] {
  const [steps, e] = postgresSteps(jobs);
  if (e || !steps) return e ? [e] : [];
  const [step, se] = uniqueNamedStep(steps, RUST_POSTGRES_INTEGRATION_STEP, "postgres");
  if (se || !step) return se ? [se] : [];
  const mask = executionStepMasked(step, "postgres", RUST_POSTGRES_INTEGRATION_STEP);
  if (mask) return [mask];
  if ("if" in step)
    return [
      `rust: postgres step ${repr(RUST_POSTGRES_INTEGRATION_STEP)} must not have an if condition`,
    ];
  if (typeof step.run !== "string")
    return [
      `rust: postgres step ${repr(RUST_POSTGRES_INTEGRATION_STEP)} must have a string run command`,
    ];
  const re = verifyIntegrationRun(step.run, false);
  return re ? [re] : [];
}
export function postgresS3Inventory(jobs: Mapping): [Set<string>, string | null] {
  const empty = new Set<string>();
  const [steps, e] = postgresSteps(jobs);
  if (e || !steps) return [empty, e];
  const [step, se] = uniqueNamedStep(steps, RUST_S3_INTEGRATION_STEP, "postgres");
  if (se || !step) return [empty, se];
  const mask = executionStepMasked(step, "postgres", RUST_S3_INTEGRATION_STEP);
  if (mask) return [empty, mask];
  if (step.if !== RUST_S3_INTEGRATION_STEP_IF)
    return [
      empty,
      `rust: S3 integration step if must be ${repr(RUST_S3_INTEGRATION_STEP_IF)}, got ${repr(step.if)}`,
    ];
  if (typeof step.run !== "string")
    return [
      empty,
      `rust: postgres step ${repr(RUST_S3_INTEGRATION_STEP)} must have a string run command`,
    ];
  const re = verifyIntegrationRun(step.run, true);
  return re ? [empty, re] : [cargoTestFlagsInText(step.run), null];
}
export function verifyCollaborationWorkflowExecution(jobs: Mapping): string[] {
  const job = jobs.collaboration;
  if (!isMapping(job)) return ["rust: collaboration job missing"];
  if (!isMapping(job.strategy)) return ["rust: collaboration job strategy missing"];
  if (!isMapping(job.strategy.matrix)) return ["rust: collaboration job matrix missing"];
  const rows = job.strategy.matrix.include;
  if (!Array.isArray(rows) || !rows.length)
    return ["rust: collaboration job matrix.include missing"];
  const runners = new Set<string>();
  const include: unknown[] = rows;
  for (const row of include) {
    if (!isMapping(row)) return ["rust: collaboration matrix.include row must be a mapping"];
    if (typeof row.runner !== "string") return ["rust: collaboration matrix row missing runner"];
    runners.add(row.runner);
  }
  const missing = [...difference(RUST_COLLAB_MATRIX_RUNNERS, runners)].sort();
  if (missing.length) return ["rust: collaboration matrix missing runners: " + missing.join(", ")];
  if (!Array.isArray(job.steps)) return ["rust: collaboration job steps missing"];
  const [step, e] = uniqueNamedStep(job.steps, RUST_COLLAB_INTEGRATION_STEP, "collaboration");
  if (e || !step) return e ? [e] : [];
  const mask = executionStepMasked(step, "collaboration", RUST_COLLAB_INTEGRATION_STEP);
  if (mask) return [mask];
  if ("if" in step) return ["rust: collaboration integration step must not have an if condition"];
  if (typeof step.run !== "string")
    return [
      `rust: collaboration step ${repr(RUST_COLLAB_INTEGRATION_STEP)} must have a string run command`,
    ];
  return normalizeRunScript(step.run) === normalizeRunScript(RUST_COLLAB_INTEGRATION_RUN)
    ? []
    : [`rust: collaboration integration step must execute ${RUST_COLLAB_INTEGRATION_RUN}`];
}
export function collaborationScriptCargoCommands(text: string): string[] {
  const lines = text.split(/\r?\n/),
    commands: string[] = [];
  for (const [i, line] of lines.entries()) {
    const stripped = line.trim();
    if (!stripped.startsWith("cargo test ")) continue;
    const trimContinuation = (v: string) => v.replace(/\\+$/, "").trim();
    const parts = [trimContinuation(stripped)];
    let next = i + 1;
    while (next < lines.length) {
      const cont = lines[next]?.trim() ?? "";
      if (cont.startsWith("--test ")) {
        parts.push(trimContinuation(cont));
        next++;
        continue;
      }
      if (cont.startsWith("-- ")) parts.push(trimContinuation(cont));
      break;
    }
    commands.push(parts.join(" "));
  }
  return commands;
}
export function collaborationScriptInventory(root: string): [Set<string>, string | null] {
  const path = join(root, RUST_COLLAB_CI_SCRIPT),
    empty = new Set<string>();
  if (!isFile(path))
    return [empty, `rust: missing collaboration CI script ${RUST_COLLAB_CI_SCRIPT}`];
  const commands = collaborationScriptCargoCommands(read(path));
  if (!commands.length)
    return [empty, "rust: collaboration CI script missing cargo test invocation"];
  const tests = new Set<string>();
  for (const command of commands) {
    const e = cargoCommandSuppressionError(
      command,
      "collaboration CI script cargo test",
      RUST_COLLAB_LIBTEST_ARGS,
    );
    if (e) return [empty, e];
    const args = command.split(" -- ", 1)[0] ?? "";
    const shape = validateCargoTestInvocation(
      args.trim().split(/\s+/),
      "collaboration CI script cargo test",
      true,
    );
    if (shape) return [empty, shape];
    const invocation = cargoTestFlagsInText(args),
      repeated = [...intersect(tests, invocation)].sort();
    if (repeated.length)
      return [
        empty,
        "rust: collaboration CI script runs --test targets more than once: " + repeated.join(", "),
      ];
    for (const t of invocation) tests.add(t);
  }
  return tests.size
    ? [tests, null]
    : [empty, "rust: collaboration CI script declares no --test targets"];
}
export function verifyRustSuiteRegistry(root: string = ROOT): string[] {
  const errors: string[] = [];
  if (!isFile(join(root, ".github/workflows", RUST_WORKFLOW_FILE)))
    return [`rust: missing workflow file ${RUST_WORKFLOW_FILE}`];
  if (!isFile(join(root, "Cargo.toml"))) return ["rust: missing root Cargo.toml"];
  const [targets, ce] = rootDbIntegrationRegistryTargets(root);
  if (ce || !targets) return ce ? [ce] : errors;
  const [jobs, je] = rustWorkflowJobs(root);
  if (je || !jobs) return je ? [je] : errors;
  errors.push(...verifyPostgresIntegrationExecution(jobs));
  const [per, me] = postgresMatrixInventory(jobs);
  if (me) return [...errors, me];
  const [s3, se] = postgresS3Inventory(jobs);
  if (se) return [...errors, se];
  errors.push(...verifyCollaborationWorkflowExecution(jobs));
  const [collab, coe] = collaborationScriptInventory(root);
  if (coe) return [...errors, coe];
  const x = per.x64 ?? new Set<string>(),
    a = per.arm64 ?? new Set<string>();
  const onlyx = [...difference(x, a)].sort(),
    onlya = [...difference(a, x)].sort();
  if (onlyx.length) errors.push("rust: postgres matrix missing on arm64: " + onlyx.join(", "));
  if (onlya.length) errors.push("rust: postgres matrix missing on x64: " + onlya.join(", "));
  const overlap = new Set([...intersect(x, collab), ...intersect(x, s3), ...intersect(collab, s3)]);
  if (overlap.size)
    errors.push(
      "rust: integration target assigned to multiple CI buckets: " + [...overlap].sort().join(", "),
    );
  if (
    intersect(RUST_INTEGRATION_MANUAL_TARGETS, targets).size &&
    !isFile(join(root, RUST_CAPACITY_PROBE_SCRIPT))
  )
    errors.push(`rust: missing manual probe script ${RUST_CAPACITY_PROBE_SCRIPT}`);
  const assigned = new Set([...x, ...collab, ...s3, ...RUST_INTEGRATION_MANUAL_TARGETS]);
  const missing = [
    ...difference(difference(targets, RUST_INTEGRATION_MANUAL_TARGETS), assigned),
  ].sort();
  if (missing.length)
    errors.push(
      "rust: Cargo.toml db-tests integration targets missing from rust.yml inventory: " +
        missing.join(", "),
    );
  return errors;
}

export function verifyWorkflowWriteScopes(
  data: Mapping,
  name: string,
  allowed: Record<string, ReadonlySet<string>>,
): string[] {
  const errors: string[] = [];
  if (!same(data.permissions, { contents: "read" }))
    errors.push(`${name}: top-level permissions must be exactly contents: read`);
  const jobs = data.jobs;
  if (!isMapping(jobs) || !Object.keys(jobs).length)
    return [...errors, `${name}: jobs mapping missing`];
  for (const [id, spec] of Object.entries(jobs)) {
    if (!isMapping(spec)) {
      errors.push(`${name}: ${id} must be a mapping`);
      continue;
    }
    const perms = spec.permissions ?? {};
    if (!isMapping(perms)) {
      errors.push(`${name}: ${id} permissions must be a scope mapping`);
      continue;
    }
    const writes = new Set(Object.keys(perms).filter((scope) => perms[scope] === "write"));
    const unexpected = [...difference(writes, allowed[id] ?? new Set())].sort();
    if (unexpected.length) errors.push(`${name}: ${id} may not write ${repr(unexpected)}`);
  }
  return errors;
}
export function verifyReleaseWorkflow(path: string): string[] {
  const name = basename(path),
    [data, e] = loadYamlMapping(path);
  if (e || !data) return [`${name}: ${String(e)}`];
  const errors: string[] = [],
    triggers = data.on ?? data.true;
  if (!isMapping(triggers) || !keysEqual(triggers, ["push", "workflow_dispatch"]))
    errors.push(`${name}: triggers must be exactly push (tags) and workflow_dispatch`);
  else {
    const push = triggers.push,
      tags = mapping(push).tags;
    if (
      !isMapping(push) ||
      !keysEqual(push, ["tags"]) ||
      !strings(tags) ||
      !tags.length ||
      !tags.every((tag) => tag.startsWith("v0."))
    )
      errors.push(`${name}: push must list only v0.* tags`);
  }
  const c = data.concurrency;
  if (
    !isMapping(c) ||
    typeof c.group !== "string" ||
    c.group.includes("${{") ||
    c["cancel-in-progress"] !== false
  )
    errors.push(`${name}: concurrency must be one fixed group with cancel-in-progress: false`);
  return [...errors, ...verifyWorkflowWriteScopes(data, name, RELEASE_WRITE_SCOPES)];
}
export function verifyOptInWiring(workflow: string, data: Mapping, jobs: Mapping): string[] {
  const errors: string[] = [],
    triggers = data.on ?? data.true;
  if (!isMapping(triggers) || !("workflow_dispatch" in triggers))
    return [`${workflow}: workflow_dispatch trigger missing`];
  const dispatch = triggers.workflow_dispatch;
  if (dispatch !== null && dispatch !== undefined && !isMapping(dispatch))
    return [`${workflow}: workflow_dispatch must be a mapping`];
  const inputs = mapping(dispatch).inputs;
  if (inputs !== null && inputs !== undefined && !isMapping(inputs))
    return [`${workflow}: workflow_dispatch inputs must be a mapping`];
  const optins = OPT_IN_JOBS[workflow] ?? {},
    expected = Object.values(optins).sort();
  if (!keysEqual(mapping(inputs), expected))
    return [`${workflow}: workflow_dispatch inputs must be exactly ${repr(expected)}`];
  for (const name of expected) {
    const spec = mapping(inputs)[name];
    if (!isMapping(spec) || spec.type !== "boolean" || spec.default !== false)
      errors.push(`${workflow}: input ${name} must be type boolean with default false`);
  }
  for (const job of Object.keys(optins)) {
    const spec = jobs[job];
    if (!isMapping(spec)) continue;
    if (spec["runs-on"] !== OPT_IN_RUNNER[job])
      errors.push(`${workflow}: ${job} runs-on must be ${String(OPT_IN_RUNNER[job])}`);
    if ("strategy" in spec)
      errors.push(`${workflow}: ${job} must be a single job without a matrix`);
  }
  return errors;
}
export function verifyWorkflowRegistry(root: string = ROOT): string[] {
  const errors: string[] = [],
    dir = join(root, ".github/workflows");
  const allowed = new Set([
    ...Object.values(WORKFLOW_YAML),
    RELEASE_WORKFLOW_FILE,
    CI_BASE_WORKFLOW_FILE,
  ]);
  if (!isDir(dir)) return ["missing .github/workflows directory"];
  for (const path of listWorkflowFiles(root))
    if (!allowed.has(basename(path))) errors.push(`unknown workflow file ${basename(path)}`);
  for (const [workflow, filename] of Object.entries(WORKFLOW_YAML)) {
    const path = join(dir, filename);
    if (!isFile(path)) {
      errors.push(`${workflow}: missing workflow file ${filename}`);
      continue;
    }
    const [data, pe] = loadYamlMapping(path);
    if (pe || !data) {
      errors.push(`${workflow}: ${String(pe)}`);
      continue;
    }
    const triggers = data.on ?? data.true;
    if (!isMapping(triggers) || !("pull_request" in triggers))
      errors.push(`${workflow}: pull_request trigger is required for the stable gate`);
    else if (triggers.pull_request !== null)
      errors.push(`${workflow}: pull_request must be unfiltered so required gates always run`);
    const jobs = data.jobs;
    if (!isMapping(jobs) || !Object.keys(jobs).length) {
      errors.push(`${workflow}: jobs mapping missing`);
      continue;
    }
    if (Object.keys(jobs).some((id) => !/^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(id))) {
      errors.push(`${workflow}: invalid job id`);
      continue;
    }
    const reserved = gateJobId(workflow),
      expected = WORKFLOW_JOBS[workflow] ?? [];
    if (!(PLAN_JOB_ID in jobs))
      errors.push(`${workflow}: missing reserved plan job ${PLAN_JOB_ID}`);
    if (!(reserved in jobs)) errors.push(`${workflow}: missing reserved gate job ${reserved}`);
    for (const job of expected) {
      if (!(job in jobs)) errors.push(`${workflow}: missing registered job id ${job}`);
      else if (job === PLAN_JOB_ID || job === reserved)
        errors.push(`${workflow}: registered job collides with reserved id ${job}`);
    }
    for (const job of Object.keys(jobs))
      if (job !== PLAN_JOB_ID && job !== reserved && !expected.includes(job))
        errors.push(`${workflow}: unregistered job id ${job}`);
    const plan = jobs[PLAN_JOB_ID];
    if (isMapping(plan)) {
      const steps = Array.isArray(plan.steps) ? plan.steps : [];
      const checkouts = steps.filter(
        (step: unknown): step is Mapping =>
          isMapping(step) &&
          typeof step.uses === "string" &&
          step.uses.startsWith("actions/checkout@"),
      );
      if (checkouts.length !== 1 || !same(checkouts[0]?.with, { "fetch-depth": 0 }))
        errors.push(
          `${workflow}: ci-plan must checkout the event merge with fetch-depth: 0 and no ref override`,
        );
      const envs = [
        data.env,
        plan.env,
        ...steps.filter((v: unknown): v is Mapping => isMapping(v)).map((v) => v.env),
      ];
      if (envs.some((v) => isMapping(v) && "GITHUB_SHA" in v))
        errors.push(`${workflow}: ci-plan must not override trusted GITHUB_SHA`);
      if ("if" in plan) errors.push(`${workflow}: ${PLAN_JOB_ID} must not have an if condition`);
      const outputs = plan.outputs;
      if (!isMapping(outputs)) errors.push(`${workflow}: ${PLAN_JOB_ID} outputs mapping missing`);
      else {
        for (const key of PLAN_OUTPUT_KEYS)
          if (!(key in outputs)) errors.push(`${workflow}: ${PLAN_JOB_ID} missing output ${key}`);
        for (const job of expected)
          if (!(selectOutputKey(job) in outputs))
            errors.push(`${workflow}: missing selector output ${selectOutputKey(job)}`);
      }
      const runs = runScripts(plan).join("\n");
      if (!runs.includes(REQUIREMENTS_FILE))
        errors.push(`${workflow}: ${PLAN_JOB_ID} must install pinned ${REQUIREMENTS_FILE}`);
      if (!runs.includes("scripts/ci_selection.py plan"))
        errors.push(`${workflow}: ${PLAN_JOB_ID} must invoke ci_selection.py plan`);
      if (!runs.includes(`--workflow ${workflow}`) && !runs.includes(`--workflow=${workflow}`))
        errors.push(`${workflow}: ${PLAN_JOB_ID} must pass --workflow ${workflow}`);
      const lines = scriptLines(runs),
        wrapper = lines.indexOf(`bash ${SELECTOR_REGRESSION_WRAPPER}`);
      if (workflow === "rust") {
        const at = lines.findIndex((line) => line.includes("scripts/ci_selection.py plan"));
        if (wrapper < 0 || at < 0 || wrapper > at)
          errors.push(
            `${workflow}: ${PLAN_JOB_ID} must run ${SELECTOR_REGRESSION_WRAPPER} before plan output`,
          );
      } else if (runs.includes(SELECTOR_REGRESSION_WRAPPER))
        errors.push(
          `${workflow}: ${PLAN_JOB_ID} must not duplicate ${SELECTOR_REGRESSION_WRAPPER}`,
        );
    } else if (PLAN_JOB_ID in jobs) errors.push(`${workflow}: ${PLAN_JOB_ID} must be a mapping`);
    for (const job of expected) {
      const spec = jobs[job];
      if (!isMapping(spec)) continue;
      const [needs, ne] = needsList(spec);
      if (ne) errors.push(`${workflow}: ${job} ${ne}`);
      else if (!needs?.includes(PLAN_JOB_ID))
        errors.push(`${workflow}: ${job} must need ${PLAN_JOB_ID}`);
      if (spec.if !== expectedSelectIf(job))
        errors.push(`${workflow}: ${job} if must be ${repr(expectedSelectIf(job))}`);
    }
    const gate = jobs[reserved];
    if (isMapping(gate)) {
      if (gate.if !== "always()") errors.push(`${workflow}: ${reserved} must use if: always()`);
      const [needs, ne] = needsList(gate);
      if (ne) errors.push(`${workflow}: ${reserved} ${ne}`);
      else if (!same([...new Set(needs ?? [])].sort(), [PLAN_JOB_ID, ...expected].sort()))
        errors.push(
          `${workflow}: ${reserved} needs must be ${PLAN_JOB_ID} and every registered job`,
        );
      const steps = runSteps(gate),
        step = steps[0];
      if (steps.length !== 1 || !step)
        errors.push(`${workflow}: ${reserved} must have exactly one run step`);
      else {
        if (!same(step.env, { NEEDS_JSON: GATE_NEEDS_JSON_EXPR, TESTED_SHA: GATE_TESTED_SHA_EXPR }))
          errors.push(
            `${workflow}: ${reserved} env must be exactly NEEDS_JSON=${GATE_NEEDS_JSON_EXPR} and TESTED_SHA=${GATE_TESTED_SHA_EXPR}`,
          );
        if (normalizeRunScript(String(step.run)) !== canonicalGateRun(workflow))
          errors.push(`${workflow}: ${reserved} must use the canonical gate invocation`);
      }
    } else if (reserved in jobs) errors.push(`${workflow}: ${reserved} must be a mapping`);
    errors.push(...verifyOptInWiring(workflow, data, jobs));
  }
  const release = join(dir, RELEASE_WORKFLOW_FILE);
  if (isFile(release)) errors.push(...verifyReleaseWorkflow(release));
  const image = join(dir, CI_BASE_WORKFLOW_FILE);
  if (isFile(image)) {
    const [data, e] = loadYamlMapping(image);
    if (e || !data) errors.push(`${basename(image)}: ${String(e)}`);
    else errors.push(...verifyWorkflowWriteScopes(data, basename(image), CI_BASE_WRITE_SCOPES));
  }
  return [...errors, ...verifyRustSuiteRegistry(root)];
}

export function validatePlanSchema(plan: unknown, workflow: string): string | null {
  if (!isMapping(plan)) return "PLAN_TOP_TYPE";
  if (Object.keys(plan).some((k) => !ALLOWED_PLAN_KEYS.has(k))) return "PLAN_UNKNOWN_KEYS";
  if (plan.version !== PLAN_VERSION) return "PLAN_VERSION";
  if (plan.workflow !== workflow) return "PLAN_WORKFLOW";
  if (plan.mode !== "full" && plan.mode !== "narrow") return "PLAN_MODE";
  if (typeof plan.reason_code !== "string" || !/^[A-Z][A-Z0-9_]{0,63}$/.test(plan.reason_code))
    return "PLAN_REASON_CODE";
  if (typeof plan.plan_ok !== "boolean") return "PLAN_OK_TYPE";
  if (!plan.plan_ok) return "PLAN_NOT_OK";
  if (typeof plan.tested_sha !== "string" || !validateSha(plan.tested_sha))
    return "PLAN_TESTED_SHA";
  const jobs = plan.jobs;
  if (!isMapping(jobs)) return "PLAN_JOBS";
  if (!keysEqual(jobs, WORKFLOW_JOBS[workflow] ?? [])) return "PLAN_JOB_SET";
  for (const job of WORKFLOW_JOBS[workflow] ?? []) {
    const entry = jobs[job];
    if (!isMapping(entry)) return "PLAN_JOB_MISSING";
    if (Object.keys(entry).some((k) => k !== "selected")) return "PLAN_JOB_UNKNOWN_KEYS";
    if (typeof entry.selected !== "boolean") return "PLAN_SELECTED_TYPE";
  }
  return null;
}
function needResult(entry: unknown): [string | null, string | null] {
  if (!isMapping(entry)) return [null, "NEED_ENTRY_TYPE"];
  if (!("result" in entry)) return [null, "NEED_RESULT_MISSING"];
  if (typeof entry.result !== "string") return [null, "NEED_RESULT_TYPE"];
  return ["success", "failure", "cancelled", "skipped"].includes(entry.result)
    ? [entry.result, null]
    : [null, "NEED_RESULT_INVALID"];
}
function needOutputs(entry: Mapping): [Record<string, string> | null, string | null] {
  const raw = entry.outputs;
  if (raw === undefined || raw === null) return [{}, null];
  if (!isMapping(raw) || Object.values(raw).some((v) => typeof v !== "string"))
    return [null, "NEED_OUTPUTS_TYPE"];
  return [raw as Record<string, string>, null];
}
export function loadNeedsContext(
  raw: string,
  workflow: string,
): [unknown, Record<string, string> | null, string | null] {
  let needs: unknown;
  try {
    needs = JSON.parse(raw) as unknown;
  } catch {
    return [null, null, "NEEDS_MALFORMED"];
  }
  if (!isMapping(needs)) return [null, null, "NEEDS_TYPE"];
  const expected = WORKFLOW_JOBS[workflow] ?? [];
  if (!keysEqual(needs, [PLAN_JOB_ID, ...expected])) return [null, null, "NEEDS_KEY_SET"];
  const entry = needs[PLAN_JOB_ID],
    [result, re] = needResult(entry);
  if (re) return [null, null, `PLAN_${re}`];
  if (result !== "success") return [null, null, "PLAN_RESULT"];
  const [outputs, oe] = needOutputs(mapping(entry));
  if (oe) return [null, null, `PLAN_${oe}`];
  const json = outputs?.plan_json;
  if (!json?.trim()) return [null, null, "PLAN_JSON_MISSING"];
  let plan: unknown;
  try {
    plan = JSON.parse(json) as unknown;
  } catch {
    return [null, null, "PLAN_JSON_MALFORMED"];
  }
  const results: Record<string, string> = {};
  for (const job of expected) {
    const [v, e] = needResult(needs[job]);
    if (e) return [null, null, `JOB_${e}`];
    if (v !== null) results[job] = v;
  }
  return [plan, results, null];
}
export function gateOptInError(workflow: string, plan: Mapping): string | null {
  const optins = OPT_IN_JOBS[workflow];
  if (!optins) return null;
  const name = (process.env.GITHUB_EVENT_NAME ?? "").trim();
  if (!KNOWN_EVENTS.has(name)) return "EVENT_NAME";
  const path = (process.env.GITHUB_EVENT_PATH ?? "").trim();
  if (!path) return "EVENT_PATH_MISSING";
  let event: unknown;
  try {
    event = loadEvent(path);
  } catch {
    return "EVENT_MALFORMED";
  }
  const [chosen, e] = dispatchOptIns(workflow, name, event);
  if (e) return e;
  for (const [job, input] of Object.entries(optins))
    if (mapping(mapping(plan.jobs)[job]).selected !== chosen.has(input))
      return `OPT_IN_MISMATCH ${job}`;
  return null;
}
function argumentsFor(
  argv: string[],
  options: Record<string, { type: "string"; default?: string }>,
): Record<string, string | undefined> {
  const parsed = parseArgs({ args: argv, options, strict: true, allowPositionals: false });
  return parsed.values;
}
function workflowArgument(args: Record<string, string | undefined>): string {
  const workflow = args.workflow;
  if (!workflow || !(workflow in WORKFLOW_JOBS))
    throw new Error("--workflow must be a registered workflow");
  return workflow;
}
function requiredArgument(args: Record<string, string | undefined>, name: string): string {
  const value = args[name];
  if (value === undefined) throw new Error(`--${name} is required`);
  return value;
}
export function cmdPlan(argv: string[]): number {
  const args = argumentsFor(argv, {
    workflow: { type: "string" },
    "repo-root": { type: "string", default: ROOT },
    "event-json": { type: "string" },
    "output-plan": { type: "string" },
    "github-output": { type: "string" },
  });
  const workflow = workflowArgument(args),
    repo = args["repo-root"] ?? ROOT,
    eventPath = requiredArgument(args, "event-json"),
    out = requiredArgument(args, "output-plan");
  const errors = verifyWorkflowRegistry(repo);
  if (errors.length) {
    console.error("plan: workflow registry validation failed");
    for (const e of errors) console.error(e);
    return 1;
  }
  const name = (process.env.GITHUB_EVENT_NAME ?? "").trim();
  if (!name) {
    console.error("plan: GITHUB_EVENT_NAME required");
    return 1;
  }
  const event = loadEvent(eventPath),
    resolved = resolveSelectionInputs(repo, mapping(event), name),
    [optins, oe] = dispatchOptIns(workflow, name, event);
  const plan = buildPlan({
    workflow,
    event_name: name,
    ...resolved,
    fatal_error: resolved.fatal_error ?? oe,
    opt_in_inputs: optins,
  });
  writeFileSync(out, JSON.stringify(plan, null, 2) + "\n");
  writeGithubOutputs(plan, args["github-output"] ?? null);
  console.log(
    `{"mode": ${JSON.stringify(plan.mode)}, "reason_code": ${JSON.stringify(plan.reason_code)}}`,
  );
  return 0;
}
export function cmdGate(argv: string[]): number {
  const args = argumentsFor(argv, {
    workflow: { type: "string" },
    "needs-json": { type: "string" },
    "tested-sha": { type: "string" },
  });
  const workflow = workflowArgument(args),
    tested = requiredArgument(args, "tested-sha");
  const fail = (message: string) => {
    console.error(message);
    return 1;
  };
  if (!validateSha(tested)) return fail("gate: tested-sha invalid");
  const raw = args["needs-json"] ?? process.env.NEEDS_JSON;
  if (!raw?.trim()) return fail("gate: needs json missing");
  const [plan, results, e] = loadNeedsContext(raw, workflow);
  if (e) return fail(`gate: needs error ${e}`);
  const se = validatePlanSchema(plan, workflow);
  if (se) return fail(`gate: plan schema error ${se}`);
  const validated = mapping(plan);
  if (validated.tested_sha !== tested) return fail("gate: tested_sha mismatch");
  const oe = gateOptInError(workflow, validated);
  if (oe) return fail(`gate: opt-in error ${oe}`);
  for (const job of WORKFLOW_JOBS[workflow] ?? []) {
    const selected = mapping(mapping(validated.jobs)[job]).selected,
      result = results?.[job];
    if (selected && result !== "success")
      return fail(`gate: selected job ${job} must succeed, got ${String(result)}`);
    if (!selected && result !== "skipped")
      return fail(`gate: unselected job ${job} must be skipped, got ${String(result)}`);
  }
  console.log("gate: ok");
  return 0;
}
export function cmdVerifyWorkflows(argv: string[]): number {
  const args = argumentsFor(argv, { "repo-root": { type: "string", default: ROOT } }),
    errors = verifyWorkflowRegistry(args["repo-root"] ?? ROOT);
  for (const e of errors) console.error(e);
  return errors.length ? 1 : 0;
}
export function main(argv: string[] = process.argv.slice(2)): number {
  const [command, ...rest] = argv;
  if (!command) {
    console.error("usage: ci_selection.py {plan,gate,verify-workflows} ...");
    return 2;
  }
  try {
    if (command === "plan") return cmdPlan(rest);
    if (command === "gate") return cmdGate(rest);
    if (command === "verify-workflows") return cmdVerifyWorkflows(rest);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    return 2;
  }
  console.error(`unknown command: ${command}`);
  return 2;
}
if (import.meta.main) process.exitCode = main();
