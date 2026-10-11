import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { buildPlan, type Plan } from "./plan.ts";
import { PLANNER_ROOT } from "./paths.ts";
import { OPT_IN_JOBS, WORKFLOWS, type Workflow } from "./registry.ts";

// Throwaway git repositories shaped like the GitHub checkouts ci-plan sees.

export const SHA_A = "a".repeat(40);
export const SHA_B = "b".repeat(40);
export const SHA_C = "c".repeat(40);

// Each test file registers afterEach(removeScratch). A hook here would attach only to
// the first file that imports this cached module, leak every later file's
// repositories and remove them all under one hook timeout.
const scratch: string[] = [];
export function removeScratch(): void {
  for (const dir of scratch.splice(0)) rmSync(dir, { recursive: true, force: true });
}

export function tempDir(label: string): string {
  const dir = mkdtempSync(join(tmpdir(), `fvoci-plan-${label}-`));
  scratch.push(dir);
  return dir;
}

/** The parent environment without any GITHUB_* event a CI job exported. */
export function cleanEnv(extra: Record<string, string | undefined> = {}): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined && !key.startsWith("GITHUB_")) env[key] = value;
  }
  for (const [key, value] of Object.entries(extra)) if (value !== undefined) env[key] = value;
  return env;
}

export function git(cwd: string, ...args: string[]): string {
  const proc = Bun.spawnSync(["git", ...args], {
    cwd,
    env: cleanEnv(),
    stdout: "pipe",
    stderr: "pipe",
  });
  if (proc.exitCode !== 0)
    throw new Error(`git ${args.join(" ")} failed: ${proc.stderr.toString()}`);
  return proc.stdout.toString().trim();
}

export function writeFile(repo: string, rel: string, content = "x\n"): void {
  mkdirSync(dirname(join(repo, rel)), { recursive: true });
  writeFileSync(join(repo, rel), content);
}

/** The real workflow files. */
export function copyWorkflows(dst: string): void {
  const src = join(PLANNER_ROOT, ".github", "workflows");
  mkdirSync(join(dst, ".github", "workflows"), { recursive: true });
  for (const name of readdirSync(src)) {
    if (/\.ya?ml$/.test(name))
      copyFileSync(join(src, name), join(dst, ".github", "workflows", name));
  }
}

// Besides the workflows, the files the complete registry check reads.
const REGISTRY_INPUTS = [
  "Cargo.toml",
  "scripts/run-rust-collaboration-ci-tests.sh",
  "scripts/collab-capacity-probe.sh",
  "xtask/selected-library-filters.txt",
];

/** The real workflows and registry inputs, so the plan's registry check passes. */
export function copyRegistryInputs(dst: string): void {
  copyWorkflows(dst);
  for (const rel of REGISTRY_INPUTS) {
    mkdirSync(dirname(join(dst, rel)), { recursive: true });
    copyFileSync(join(PLANNER_ROOT, rel), join(dst, rel));
  }
}

function configure(repo: string): void {
  git(repo, "config", "user.email", "ci@test");
  git(repo, "config", "user.name", "ci");
  git(repo, "config", "commit.gpgsign", "false");
}

/** A single-branch repository with commit/rename/delete helpers. */
export class Repo {
  readonly dir: string;
  constructor(dir = tempDir("repo")) {
    this.dir = dir;
    git(dir, "init", "-q", "-b", "main");
    configure(dir);
  }
  head(): string {
    return git(this.dir, "rev-parse", "HEAD");
  }
  commit(files: Record<string, string>, message = "change"): string {
    for (const [rel, content] of Object.entries(files)) writeFile(this.dir, rel, content);
    git(this.dir, "add", "-A");
    git(this.dir, "commit", "-q", "-m", message);
    return this.head();
  }
  rename(from: string, to: string): string {
    mkdirSync(dirname(join(this.dir, to)), { recursive: true });
    git(this.dir, "mv", from, to);
    git(this.dir, "commit", "-q", "-m", `rename ${from}`);
    return this.head();
  }
  remove(rel: string): string {
    git(this.dir, "rm", "-q", rel);
    git(this.dir, "commit", "-q", "-m", `delete ${rel}`);
    return this.head();
  }
}

/** Origin plus work clones with GitHub-like PR merge checkouts. */
export class PrCheckout {
  readonly origin = new Repo(tempDir("origin"));
  readonly base: string;
  private branches = 0;

  constructor() {
    git(this.origin.dir, "config", "uploadpack.allowReachableSHA1InWant", "true");
    copyRegistryInputs(this.origin.dir);
    this.base = this.origin.commit({ "README.md": "base docs\n" }, "base");
  }
  /** A PR head branched from main. */
  branch(files: Record<string, string>): string {
    const name = `pr-${String(this.branches++)}`;
    git(this.origin.dir, "checkout", "-q", "-B", name, "main");
    const sha = this.origin.commit(files, name);
    git(this.origin.dir, "checkout", "-q", "main");
    return sha;
  }
  /** Advance main on origin. */
  advance(files: Record<string, string>): string {
    return this.origin.commit(files, "advance");
  }
  clone(): string {
    const work = tempDir("work");
    git(work, "clone", "-q", this.origin.dir, ".");
    configure(work);
    return work;
  }
  /** A clone checked out at the merge of `head` into `first` (GitHub's merge ref). */
  merge(first: string, head: string): { work: string; tested: string } {
    const work = this.clone();
    git(work, "checkout", "-q", "-B", "tested", first);
    git(work, "merge", "-q", "--no-ff", "-m", "github merge", head);
    return { work, tested: git(work, "rev-parse", "HEAD") };
  }
  /** Rewrite the tested merge's tree (conflict resolution or injected files). */
  amend(work: string, edit: () => void): string {
    edit();
    git(work, "add", "-A");
    git(work, "commit", "-q", "--amend", "--no-edit");
    return git(work, "rev-parse", "HEAD");
  }
}

export const prEvent = (base: unknown, head: unknown) => ({
  pull_request: { draft: false, base: { sha: base }, head: { sha: head } },
});

export function isOptIn(workflow: Workflow, job: string): boolean {
  return Object.hasOwn(OPT_IN_JOBS[workflow] ?? {}, job);
}

export function selectedJobs(plan: Plan): string[] {
  return Object.entries(plan.jobs)
    .filter(([, meta]) => meta.selected)
    .map(([job]) => job);
}

export function planAll(
  paths: readonly string[] | null,
  eventName = "pull_request",
  extra: Partial<Parameters<typeof buildPlan>[0]> = {},
): Record<Workflow, Plan> {
  const plans = {} as Record<Workflow, Plan>;
  for (const workflow of WORKFLOWS) {
    plans[workflow] = buildPlan({
      workflow,
      eventName,
      baseSha: SHA_A,
      headSha: SHA_B,
      mergeBaseSha: SHA_C,
      testedSha: SHA_B,
      paths,
      ...extra,
    });
  }
  return plans;
}
