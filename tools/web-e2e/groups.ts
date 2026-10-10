// Discover normal web Playwright groups, shard them, and verify CI coverage.
// Callers: scripts/run-web-e2e.sh (verify, shard-jsonl) and
// scripts/test-web-e2e-groups.sh (verify). Differences from the replaced
// Python CLI are listed in the migration commit message.
import { lstatSync, readdirSync, realpathSync, statSync, type Dirent } from "node:fs";
import { join, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { compareCodePoints, compareSequences } from "./compat.ts";

export const ROOT = resolve(realpathSync(fileURLToPath(import.meta.url)), "..", "..", "..");
export const DEFAULT_E2E_DIR = join(ROOT, "apps", "web", "e2e");
export const PAIR_FIRST = "workspace-flow.spec.ts";
export const PAIR_SECOND = "workspace-wiki-flow.spec.ts";
export const DEFAULT_SHARD_COUNT = 8;
// Scheduling estimates only: completed group wall time (fresh runtime through
// cleanup), rounded up, from Web run 37194529902 at main 52129a1 (2026-10-04).
// Attempts 1/2 supply shard 0; other shards were carried forward, not rerun.
// Keep overrides only for observed groups >= 40s; the median was 17.3s, rounded
// to a 20s fallback for smaller and newly discovered groups. Build/dependency
// preparation is per-shard and excluded. Timer is ONE logical group costing the
// sum of its unchanged four fresh runs (9+7+1+5 cases), not one Playwright run.
// These estimates affect assignment only, never discovery or test selection.
export const DEFAULT_GROUP_SECONDS = 20;
export const OBSERVED_GROUP_SECONDS: ReadonlyMap<string, number> = new Map([
  ["e2e/account-admin-vue-flow.spec.ts", 42],
  ["e2e/collection-calendar-template.spec.ts", 67],
  ["e2e/editor-entities-flow.spec.ts", 46],
  ["e2e/main-alignment-navigation-lifetime.spec.ts", 44],
  ["e2e/main-alignment-task-contract.spec.ts", 47],
  ["e2e/personal-team-transfer.spec.ts", 60],
  ["e2e/tb-a-dev-editor.spec.ts", 84],
  ["e2e/tb-d-document-header-draft.spec.ts", 52],
  ["e2e/v050-editor-modes.spec.ts", 123],
  ["e2e/v050-native-archive.spec.ts", 68],
  ["e2e/v050-task-timer.spec.ts", 230],
  ["e2e/workspace-wiki-vue-flow.spec.ts", 42],
]);
const SPEC_BASENAME_RE = /^[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$/;
const REL_SPEC_RE = /^e2e\/[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$/;
// Playwright default testMatch '**/*.@(spec|test).?(c|m)[jt]s?(x)' (pinned
// playwright/lib/common/index.js). Supported CI grouping is only top-level
// *.spec.ts; every other default-discoverable suite must fail closed. The
// lookahead keeps Python's `$`, which also matched before a final newline.
const PLAYWRIGHT_DEFAULT_SUITE_RE = /\.(?:spec|test)\.[cm]?[jt]sx?(?=\n?$)/iu;

export type Group = string[];
export type Shards = Group[][];

/** A refusal: the CLI prints the message to stderr and exits 1. */
export class PlanError extends Error {}

function fail(message: string): never {
  throw new PlanError(message);
}

function isDirectory(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/** `Path.resolve()`: symlinks resolved first when the path exists. */
function resolvePath(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return resolve(path);
  }
}

function entries(directory: string): Dirent[] {
  return readdirSync(directory, { withFileTypes: true });
}

function relSpec(name: string): string {
  const rel = `e2e/${name}`;
  if (!REL_SPEC_RE.test(rel)) fail(`unsupported spec path (expected e2e/*.spec.ts): ${rel}`);
  return rel;
}

function validateSpecRelpath(rel: string): void {
  if (!REL_SPEC_RE.test(rel)) fail(`invalid spec path in plan: ${JSON.stringify(rel)}`);
}

/** Top-level names matching `*.spec.ts`, whatever their file type (pathlib glob). */
function topLevelSpecNames(directory: string): string[] {
  return entries(directory)
    .map((entry) => entry.name)
    .filter((name) => name.endsWith(".spec.ts"))
    .sort(compareCodePoints);
}

/** Fail closed on Playwright-like files that discovery does not cover. */
export function findUnsupportedPlaywrightPaths(directory: string): string[] {
  const unsupported: string[] = [];
  if (!isDirectory(directory)) return unsupported;
  // pathlib rglob: every entry, without descending into symlinked directories.
  const found: string[][] = [];
  const walk = (parts: string[]): void => {
    for (const entry of entries(join(directory, ...parts))) {
      const child = [...parts, entry.name];
      found.push(child);
      // lstat, not the dirent type: some filesystems report DT_UNKNOWN.
      if (lstatSync(join(directory, ...child)).isDirectory()) walk(child);
    }
  };
  walk([]);
  found.sort(compareSequences);
  for (const parts of found) {
    if (!isFile(join(directory, ...parts))) continue;
    const rel = parts.join("/");
    const name = parts[parts.length - 1] ?? "";
    const matched = PLAYWRIGHT_DEFAULT_SUITE_RE.exec(name);
    if (!matched) continue;
    if (!rel.includes("/") && SPEC_BASENAME_RE.test(name)) continue;
    const suffix = matched[0].toLowerCase();
    if (rel.includes("/") && suffix === ".spec.ts") {
      unsupported.push(`${rel} (nested spec.ts is not supported in normal e2e)`);
    } else if (suffix === ".spec.ts") {
      unsupported.push(`${rel} (unsupported spec.ts basename)`);
    } else {
      unsupported.push(`${rel} (unsupported Playwright pattern ${suffix})`);
    }
  }
  return unsupported;
}

export function discoverGroups(directory: string = DEFAULT_E2E_DIR): Group[] {
  const root = resolvePath(directory);
  if (!isDirectory(root)) fail(`missing e2e directory: ${root}`);

  const unsupported = findUnsupportedPlaywrightPaths(root);
  if (unsupported.length > 0) {
    fail(
      "unsupported Playwright paths under normal e2e; fix or move to e2e-pending:\n" +
        unsupported.map((item) => `  - ${item}`).join("\n"),
    );
  }

  const specs = topLevelSpecNames(root);
  if (specs.length === 0) fail(`no normal e2e specs under ${root}`);

  const seen = new Set<string>();
  const groups: Group[] = [];
  let pairedWiki = false;

  for (const name of specs) {
    if (!SPEC_BASENAME_RE.test(name)) fail(`unsupported spec.ts basename: ${name}`);
    if (name === PAIR_SECOND) {
      if (pairedWiki) continue;
      fail(`${PAIR_SECOND} must be grouped with ${PAIR_FIRST}, not standalone`);
    }
    if (seen.has(name)) fail(`duplicate spec filename in discovery: ${name}`);
    if (name === PAIR_FIRST) {
      const wiki = join(root, PAIR_SECOND);
      if (!isFile(wiki)) fail(`missing paired spec: ${wiki}`);
      groups.push([relSpec(name), relSpec(PAIR_SECOND)]);
      seen.add(PAIR_FIRST);
      seen.add(PAIR_SECOND);
      pairedWiki = true;
      continue;
    }
    groups.push([relSpec(name)]);
    seen.add(name);
  }

  const all = new Set(specs);
  const missing = [...all].filter((name) => !seen.has(name)).sort(compareCodePoints);
  const extra = [...seen].filter((name) => !all.has(name)).sort(compareCodePoints);
  if (missing.length > 0 || extra.length > 0) {
    fail(`discovery mismatch missing=${JSON.stringify(missing)} extra=${JSON.stringify(extra)}`);
  }
  return groups;
}

export function groupSeconds(group: Group): number {
  // A paired group has one fresh runtime. Its fallback is per logical group.
  return Math.max(
    ...group.map((spec) => OBSERVED_GROUP_SECONDS.get(spec) ?? DEFAULT_GROUP_SECONDS),
  );
}

export function assignShards(groups: Group[], shardCount: number): Shards {
  if (shardCount < 1) fail("shard_count must be >= 1");
  const shards: Shards = Array.from({ length: shardCount }, () => []);
  const loads = new Array<number>(shardCount).fill(0);
  // Longest first, with path and shard-index ties for reproducible plans.
  const ordered = [...groups].sort(
    (a, b) => groupSeconds(b) - groupSeconds(a) || compareSequences(a, b),
  );
  for (const group of ordered) {
    let index = 0;
    for (let candidate = 1; candidate < shardCount; candidate += 1) {
      if ((loads[candidate] ?? 0) < (loads[index] ?? 0)) index = candidate;
    }
    shards[index]?.push(group);
    loads[index] = (loads[index] ?? 0) + groupSeconds(group);
  }
  return shards;
}

/** Seams for the fail-closed checks; the CLI always uses the real functions. */
export interface PlanSteps {
  discoverGroups: (directory: string) => Group[];
  assignShards: (groups: Group[], shardCount: number) => Shards;
}
const realSteps: PlanSteps = { discoverGroups, assignShards };

function sameGroups(a: Group[], b: Group[]): boolean {
  const left = a.map((group) => JSON.stringify(group)).sort();
  const right = b.map((group) => JSON.stringify(group)).sort();
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

export interface PlanSummary {
  group_count: number;
  spec_count: number;
  shard_count: number;
  groups_per_shard: number[];
}

/** Validate discovery and sharding for a fixture or production e2e tree. */
export function verifyPlan(
  directory: string = DEFAULT_E2E_DIR,
  shardCount: number,
  steps: PlanSteps = realSteps,
): PlanSummary {
  const root = resolvePath(directory);
  const groups = steps.discoverGroups(root);
  const shards = steps.assignShards(groups, shardCount);

  // Validate the assignment too: discovery alone cannot catch a scheduler
  // dropping, duplicating, splitting or altering a logical group.
  if (!sameGroups(shards.flat(), groups)) {
    fail("shard assignment has missing, duplicate or altered groups");
  }

  const specPaths: string[] = [];
  for (const group of groups) {
    for (const spec of group) validateSpecRelpath(spec);
    specPaths.push(...group);
  }
  if (specPaths.length !== new Set(specPaths).size) fail("duplicate spec membership across groups");

  const expected = topLevelSpecNames(root);
  const discovered = specPaths.map((spec) => spec.slice("e2e/".length)).sort(compareCodePoints);
  if (JSON.stringify(expected) !== JSON.stringify(discovered)) {
    fail(
      `unregistered or missing specs: tree=${JSON.stringify(expected)} groups=${JSON.stringify(discovered)}`,
    );
  }

  const empty = shards.flatMap((shard, index) => (shard.length === 0 ? [index] : []));
  if (empty.length > 0) fail(`shards with no work: ${JSON.stringify(empty)}`);

  const pair = groups.find((group) => group.length === 2);
  if (JSON.stringify(pair) !== JSON.stringify([`e2e/${PAIR_FIRST}`, `e2e/${PAIR_SECOND}`])) {
    fail(`workspace pair integrity failed: ${JSON.stringify(pair ?? null)}`);
  }

  return {
    group_count: groups.length,
    spec_count: specPaths.length,
    shard_count: shardCount,
    groups_per_shard: shards.map((shard) => shard.length),
  };
}

export function shardPlanLines(
  directory: string = DEFAULT_E2E_DIR,
  index: number,
  shardCount: number,
): { specs: Group }[] {
  const shards = assignShards(discoverGroups(directory), shardCount);
  if (index < 0 || index >= shardCount) {
    fail(`shard index ${String(index)} out of range 0..${String(shardCount - 1)}`);
  }
  const shardGroups = shards[index] ?? [];
  if (shardGroups.length === 0) fail(`shard ${String(index)} has no groups`);
  return shardGroups.map((group) => {
    for (const spec of group) validateSpecRelpath(spec);
    return { specs: group };
  });
}

const USAGE =
  "usage: groups.ts {list-groups | shard-jsonl --index N [--shards N] | verify [--shards N]}";

class UsageError extends Error {}

function integerOption(name: string, value: string | undefined, fallback?: number): number {
  if (value === undefined) {
    if (fallback === undefined)
      throw new UsageError(`the following arguments are required: --${name}`);
    return fallback;
  }
  if (!/^[+-]?[0-9]+$/.test(value))
    throw new UsageError(`argument --${name}: invalid int value: ${value}`);
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed))
    throw new UsageError(`argument --${name}: invalid int value: ${value}`);
  return parsed;
}

function run(argv: string[]): string[] {
  const [command, ...rest] = argv;
  const options = {
    index: { type: "string" },
    shards: { type: "string" },
  } as const;
  let values: { index?: string; shards?: string };
  try {
    ({ values } = parseArgs({ args: rest, options, strict: true, allowPositionals: false }));
  } catch (error) {
    throw new UsageError(error instanceof Error ? error.message : String(error));
  }
  switch (command) {
    case "list-groups":
      if (rest.length > 0) throw new UsageError(`unrecognized arguments: ${rest.join(" ")}`);
      return discoverGroups().map((group) => JSON.stringify({ specs: group }));
    case "shard-jsonl": {
      const index = integerOption("index", values.index);
      const shards = integerOption("shards", values.shards, DEFAULT_SHARD_COUNT);
      return shardPlanLines(DEFAULT_E2E_DIR, index, shards).map((line) => JSON.stringify(line));
    }
    case "verify": {
      if (values.index !== undefined) throw new UsageError("unrecognized arguments: --index");
      const shards = integerOption("shards", values.shards, DEFAULT_SHARD_COUNT);
      return [JSON.stringify(verifyPlan(DEFAULT_E2E_DIR, shards))];
    }
    default:
      throw new UsageError(
        command === undefined
          ? "the following arguments are required: command"
          : `invalid command: ${command}`,
      );
  }
}

if (import.meta.main) {
  try {
    const lines = run(process.argv.slice(2));
    process.stdout.write(lines.map((line) => `${line}\n`).join(""));
  } catch (error) {
    if (error instanceof UsageError) {
      process.stderr.write(`${USAGE}\ngroups.ts: error: ${error.message}\n`);
      process.exit(2);
    }
    if (error instanceof PlanError) {
      process.stderr.write(`${error.message}\n`);
      process.exit(1);
    }
    throw error;
  }
}
