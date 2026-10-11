// Checks scripts/run-web-e2e.sh runs as `bun tools/web-e2e/run-web-e2e.ts
// <command> ...`: committed API qualification, shard plan line parsing, and
// the selected companion's SQLite prefix containment, private diagnostics
// directory and launcher stage receipt. A refusal prints one stderr line and
// exits 1; a usage error exits 2.
import {
  appendFileSync,
  closeSync,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { validateSpecRelpath } from "./groups.ts";

/** A refusal: the CLI prints the message to stderr and exits 1. */
export class CheckError extends Error {}
class UsageError extends Error {}

function fail(message: string): never {
  throw new CheckError(message);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

const LANE_JOBS = [
  "collaboration-install-on",
  "collaboration-postgres-on",
  "collaboration-sqlite-on",
  "collaboration-postgres-off",
  "collaboration-sqlite-off",
];
const COMMITTED_OUTPUTS = ["apps/web/openapi.json", "apps/web/src/generated/api.ts"];

export interface CommittedApiRequest {
  /** The --ci-shard index; empty when no shard was requested. */
  shard: string;
  selected: boolean;
  browserPhase: "" | "prepare" | "consume";
}

/** The GitHub jobs allowed to consume committed API outputs for this request. */
export function committedApiJobs(
  request: CommittedApiRequest,
  githubJob: string | undefined,
): ReadonlySet<string> {
  let job = request.shard ? "workspace-browser-shard" : "collaboration-flow";
  if (request.browserPhase === "prepare") job = "workspace-browser-build";
  if (githubJob === "collaboration-build" && request.selected) job = "collaboration-build";
  if (!request.shard && !request.selected && request.browserPhase !== "prepare") {
    fail("requires a browser shard or selected companion");
  }
  return new Set(job === "collaboration-flow" ? [job, ...LANE_JOBS] : [job]);
}

/** One git command's result; spawn failure and signals are refusals. */
export type Git = (args: string[]) => { status: number; stdout: Uint8Array };

export function spawnGit(root: string): Git {
  return (args) => {
    let result;
    try {
      result = Bun.spawnSync(["git", "-C", root, ...args], {
        stdin: "ignore",
        stdout: "pipe",
        stderr: "inherit",
      });
    } catch (error) {
      fail(`cannot run git ${args.join(" ")}: ${errorText(error)}`);
    }
    // Bun's types say number, but a signal leaves the exit code null.
    if (result.signalCode !== undefined || !Number.isInteger(result.exitCode)) {
      fail(`git ${args.join(" ")} ended by signal ${String(result.signalCode)}`);
    }
    return { status: result.exitCode, stdout: result.stdout };
  };
}

function gitBytes(git: Git, args: string[]): Uint8Array {
  const { status, stdout } = git(args);
  if (status !== 0) fail(`git ${args.join(" ")} failed with exit ${String(status)}`);
  return stdout;
}

function gitLine(git: Git, args: string[]): string {
  let text: string;
  try {
    text = UTF8.decode(gitBytes(git, args));
  } catch (error) {
    if (error instanceof CheckError) throw error;
    fail(`git ${args.join(" ")} printed invalid UTF-8`);
  }
  return text.endsWith("\n") ? text.slice(0, -1) : text;
}

function isPhysicalFile(path: string): boolean {
  try {
    return existsSync(path) && realpathSync(path) === path && lstatSync(path).isFile();
  } catch {
    return false;
  }
}

function isPrivateDirectory(path: string): boolean {
  try {
    const stat = statSync(path);
    return (
      !lstatSync(path).isSymbolicLink() &&
      stat.isDirectory() &&
      stat.uid === process.getuid?.() &&
      (stat.mode & 0o777) === 0o700
    );
  } catch {
    return false;
  }
}

function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

/**
 * Qualify the committed OpenAPI outputs of ROOT for a browser-only CI run:
 * the allocated job, a clean checkout at GITHUB_SHA, and physical regular
 * files whose bytes equal HEAD. Returns the tested SHA.
 */
export function qualifyCommittedApi(
  root: string,
  request: CommittedApiRequest,
  env: Record<string, string | undefined>,
  git: Git = spawnGit(root),
): string {
  if (env.CI !== "true" || env.GITHUB_ACTIONS !== "true") fail("requires GitHub CI");
  const allowed = committedApiJobs(request, env.GITHUB_JOB);
  if (env.GITHUB_JOB === undefined || !allowed.has(env.GITHUB_JOB)) {
    fail("requires the allocated browser job");
  }
  const sha = env.GITHUB_SHA ?? "";
  if (!/^[0-9a-f]{40}$/.test(sha) || gitLine(git, ["rev-parse", "HEAD"]) !== sha) {
    fail("checkout HEAD differs from tested SHA");
  }
  // ROOT is compared unresolved (as a pathlib path): a wrapper reached through
  // a symlink is refused.
  const base = normalizePath(root);
  let toplevel: string;
  try {
    toplevel = realpathSync(gitLine(git, ["rev-parse", "--show-toplevel"]));
  } catch (error) {
    if (error instanceof CheckError) throw error;
    fail("wrapper must belong to this checkout");
  }
  if (toplevel !== base) fail("wrapper must belong to this checkout");
  if (git(["diff", "--quiet", "HEAD", "--"]).status !== 0) fail("tracked checkout is dirty");
  for (const name of COMMITTED_OUTPUTS) {
    const entry = gitLine(git, ["ls-tree", "HEAD", "--", name]);
    const match = /^100644 blob ([0-9a-f]{40}|[0-9a-f]{64})\t(.*)$/su.exec(entry);
    if (!match || match[2] !== name) fail(`${name} must be a tracked regular output at HEAD`);
    const oid = match[1] ?? "";
    if (gitLine(git, ["ls-files", "--stage", "--", name]) !== `100644 ${oid} 0\t${name}`) {
      fail(`${name} index differs from HEAD`);
    }
    const path = join(base, name);
    if (!isPhysicalFile(path)) fail(`${name} must be a physical regular output`);
    let content: Uint8Array;
    try {
      content = readFileSync(path);
    } catch (error) {
      fail(`${name} cannot be read: ${errorText(error)}`);
    }
    if (content.length === 0 || !bytesEqual(content, gitBytes(git, ["cat-file", "blob", oid]))) {
      fail(`${name} physical bytes differ from HEAD`);
    }
  }
  return sha;
}

/** The specs of one groups.ts shard-jsonl line, in order. */
export function planSpecs(line: string): string[] {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch {
    fail(`malformed shard plan line: ${line}`);
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    fail(`shard plan line is not an object: ${line}`);
  }
  const keys = Object.keys(value);
  if (keys.length !== 1 || keys[0] !== "specs")
    fail(`shard plan line must hold only specs: ${line}`);
  const specs: unknown = (value as { specs: unknown }).specs;
  if (!Array.isArray(specs) || specs.length === 0) fail(`shard plan group has no specs: ${line}`);
  return specs.map((spec: unknown) => {
    if (typeof spec !== "string") fail(`shard plan spec is not a string: ${line}`);
    try {
      validateSpecRelpath(spec);
    } catch (error) {
      fail(errorText(error));
    }
    return spec;
  });
}

// Linux's limit on symlinks followed while resolving one path.
const MAX_SYMLINKS = 40;

/** The parts of a lexical POSIX path, as pathlib.PurePosixPath keeps them. */
function lexicalParts(path: string): { root: string; parts: string[] } {
  const root =
    path.startsWith("//") && !path.startsWith("///") ? "//" : path.startsWith("/") ? "/" : "";
  return { root, parts: path.split("/").filter((part) => part !== "" && part !== ".") };
}

/** pathlib.PurePosixPath(path) as a string: repeated slashes and "." dropped, ".." kept. */
export function normalizePath(path: string): string {
  const { root, parts } = lexicalParts(path);
  return root + parts.join("/") || ".";
}

/**
 * pathlib.Path(path).resolve(): absolute, every existing symlink followed
 * before a later "..", missing or unreadable components kept as written.
 * Unlike Python, a symlink loop is refused (as `realpath -m` does).
 */
export function resolvePath(path: string, cwd: string = process.cwd()): string {
  if (path === "") fail("cannot resolve an empty path");
  const pending = lexicalParts(path.startsWith("/") ? path : `${cwd}/${path}`).parts.reverse();
  let resolved = "";
  let links = 0;
  for (let name = pending.pop(); name !== undefined; name = pending.pop()) {
    if (name === "..") {
      resolved = resolved.slice(0, resolved.lastIndexOf("/"));
      continue;
    }
    const next = `${resolved}/${name}`;
    let target: string | undefined;
    try {
      target = lstatSync(next).isSymbolicLink() ? readlinkSync(next) : undefined;
    } catch {
      target = undefined;
    }
    if (target === undefined) {
      resolved = next;
      continue;
    }
    links += 1;
    if (links > MAX_SYMLINKS) fail(`too many levels of symbolic links: ${path}`);
    if (target.startsWith("/")) resolved = "";
    pending.push(...lexicalParts(target).parts.reverse());
  }
  return resolved || "/";
}

/** Refuse unless CHILD, resolved, is PARENT (resolved) or lies beneath it by whole components. */
export function requirePathWithin(child: string, parent: string, cwd?: string): void {
  const inner = resolvePath(child, cwd);
  const outer = resolvePath(parent, cwd);
  if (inner !== outer && !inner.startsWith(outer === "/" ? "/" : `${outer}/`)) {
    fail(`${child} (${inner}) is not within ${parent} (${outer})`);
  }
}

const SAFE_DIAGNOSTICS = "fvoci-selected-diagnostics";

function requirePrivateDirectory(prefix: string, label: string): void {
  if (!isPrivateDirectory(prefix))
    fail(`${label} ${prefix} must be a 0700 directory owned by this user`);
}

/**
 * Create the runner-owned diagnostics directory PREFIX, which must be
 * $RUNNER_TEMP (resolved)/fvoci-selected-diagnostics. An occupied, symlinked
 * or foreign destination is refused. Publishes the path to GITHUB_OUTPUT.
 */
export function createSafeDiagnostics(
  prefix: string,
  env: Record<string, string | undefined>,
): void {
  const runnerTemp = env.RUNNER_TEMP;
  if (!runnerTemp) fail("RUNNER_TEMP is required for selected safe diagnostics");
  const expected = join(resolvePath(runnerTemp), SAFE_DIAGNOSTICS);
  // Compared as pathlib paths: repeated slashes and "." do not matter, ".." does.
  const directory = normalizePath(prefix);
  if (directory !== expected) {
    fail(`selected safe diagnostics must be ${expected}, not ${prefix}`);
  }
  if (/[\n\r]/.test(directory)) fail("selected safe diagnostics path holds a line break");
  try {
    mkdirSync(directory, { mode: 0o700 });
  } catch (error) {
    fail(`cannot create selected safe diagnostics: ${errorText(error)}`);
  }
  requirePrivateDirectory(directory, "selected safe diagnostics");
  if (env.GITHUB_OUTPUT) {
    try {
      appendFileSync(env.GITHUB_OUTPUT, `selected-safe-diagnostics=${directory}\n`);
    } catch (error) {
      fail(`cannot publish selected safe diagnostics: ${errorText(error)}`);
    }
  }
}

const RECEIPT_KEYS = [
  "actual_launcher_exit",
  "ownership_return_exit",
  "selected_final_exit",
  "pending_exit",
  "config_list_exit",
] as const;

/** A shell exit status (0..255) or `not-run` (null). */
export function stageExit(value: string): number | null {
  if (value === "not-run") return null;
  if (!/^(?:0|[1-9][0-9]{0,2})$/.test(value) || Number(value) > 255) {
    fail(`invalid stage exit status: ${JSON.stringify(value)}`);
  }
  return Number(value);
}

/** The launcher-stage.json bytes (json.dump layout kept for existing readers). */
export function launcherReceipt(values: readonly string[]): string {
  if (values.length !== RECEIPT_KEYS.length) {
    fail(`expected ${String(RECEIPT_KEYS.length)} stage exit statuses`);
  }
  const fields = RECEIPT_KEYS.map((key, index) => {
    const exit = stageExit(values[index] ?? "");
    return `"${key}": ${exit === null ? "null" : String(exit)}`;
  });
  return `{${fields.join(", ")}}\n`;
}

/** Write PREFIX/launcher-stage.json exclusively, mode 0600, after validating every value. */
export function writeLauncherReceipt(prefix: string, values: readonly string[]): void {
  const text = launcherReceipt(values);
  requirePrivateDirectory(prefix, "selected safe diagnostics");
  let fd: number;
  try {
    fd = openSync(join(prefix, "launcher-stage.json"), "wx", 0o600);
  } catch (error) {
    fail(`cannot create launcher stage receipt: ${errorText(error)}`);
  }
  try {
    fchmodSync(fd, 0o600);
    writeFileSync(fd, text);
  } catch (error) {
    fail(`cannot write launcher stage receipt: ${errorText(error)}`);
  } finally {
    closeSync(fd);
  }
}

const USAGE = [
  "usage: run-web-e2e.ts committed-api ROOT SHARD true|false ''|prepare|consume",
  "       run-web-e2e.ts plan-specs JSON_LINE",
  "       run-web-e2e.ts path-within CHILD PARENT",
  "       run-web-e2e.ts safe-diagnostics PREFIX",
  "       run-web-e2e.ts launcher-receipt PREFIX LAUNCHER OWNERSHIP SELECTED PENDING CONFIG_LIST",
].join("\n");

function arity(rest: string[], count: number): void {
  if (rest.length !== count) {
    throw new UsageError(`expected ${String(count)} arguments, got ${String(rest.length)}`);
  }
}

function run(argv: string[]): string {
  const [command, ...rest] = argv;
  switch (command) {
    case "committed-api": {
      arity(rest, 4);
      const [root = "", shard = "", selected = "", browserPhase = ""] = rest;
      if (selected !== "true" && selected !== "false") {
        throw new UsageError(`selected must be true or false: ${selected}`);
      }
      if (browserPhase !== "" && browserPhase !== "prepare" && browserPhase !== "consume") {
        throw new UsageError(`unknown browser phase: ${browserPhase}`);
      }
      const request = { shard, selected: selected === "true", browserPhase } as const;
      try {
        const sha = qualifyCommittedApi(root, request, process.env);
        return `committed API outputs match tested checkout ${sha}\n`;
      } catch (error) {
        if (error instanceof CheckError) {
          throw new CheckError(`committed API qualification failed: ${error.message}`);
        }
        throw error;
      }
    }
    case "plan-specs":
      arity(rest, 1);
      return planSpecs(rest[0] ?? "")
        .map((spec) => `${spec}\n`)
        .join("");
    case "path-within":
      arity(rest, 2);
      requirePathWithin(rest[0] ?? "", rest[1] ?? "");
      return "";
    case "safe-diagnostics":
      arity(rest, 1);
      createSafeDiagnostics(rest[0] ?? "", process.env);
      return "";
    case "launcher-receipt":
      arity(rest, 1 + RECEIPT_KEYS.length);
      writeLauncherReceipt(rest[0] ?? "", rest.slice(1));
      return "";
    default:
      throw new UsageError(
        command === undefined ? "a command is required" : `unknown command: ${command}`,
      );
  }
}

if (import.meta.main) {
  try {
    process.stdout.write(run(process.argv.slice(2)));
  } catch (error) {
    if (error instanceof UsageError) {
      process.stderr.write(`${USAGE}\nrun-web-e2e.ts: error: ${error.message}\n`);
      process.exit(2);
    }
    if (error instanceof CheckError) {
      process.stderr.write(`${error.message}\n`);
      process.exit(1);
    }
    throw error;
  }
}
