// Discover normal web Playwright groups, shard them, and verify CI coverage.
// CLI: list-groups | shard-jsonl --index N [--shards 8] | verify [--shards 8]

import { readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { Fail } from "./proc.ts";

export const repoRoot = join(import.meta.dir, "../..");
export const defaultE2eDir = join(repoRoot, "apps/web/e2e");
export const PAIR_FIRST = "workspace-flow.spec.ts";
export const PAIR_SECOND = "workspace-wiki-flow.spec.ts";
export const DEFAULT_SHARD_COUNT = 8;
export const DEFAULT_GROUP_SECONDS = 20;
export const OBSERVED_GROUP_SECONDS: Record<string, number> = {
  "e2e/account-admin-vue-flow.spec.ts": 42,
  "e2e/collection-calendar-template.spec.ts": 67,
  "e2e/editor-entities-flow.spec.ts": 46,
  "e2e/main-alignment-navigation-lifetime.spec.ts": 44,
  "e2e/main-alignment-task-contract.spec.ts": 47,
  "e2e/personal-team-transfer.spec.ts": 60,
  "e2e/tb-a-dev-editor.spec.ts": 84,
  "e2e/tb-d-document-header-draft.spec.ts": 52,
  "e2e/v050-editor-modes.spec.ts": 123,
  "e2e/v050-native-archive.spec.ts": 68,
  "e2e/v050-task-timer.spec.ts": 230,
  "e2e/workspace-wiki-vue-flow.spec.ts": 42,
};
const SPEC_BASENAME = /^[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$/;
const REL_SPEC = /^e2e\/[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$/;
const PLAYWRIGHT_DEFAULT_SUITE = /\.(?:spec|test)\.(?:[cm])?[jt]sx?$/i;

export function relSpec(name: string): string {
  const rel = `e2e/${name}`;
  if (!REL_SPEC.test(rel)) throw new Fail(`unsupported spec path (expected e2e/*.spec.ts): ${rel}`);
  return rel;
}

export function validateSpecRelpath(rel: string): void {
  if (!REL_SPEC.test(rel)) throw new Fail(`invalid spec path in plan: ${JSON.stringify(rel)}`);
}

function walkFiles(directory: string): string[] {
  const found: string[] = [];
  const visit = (current: string) => {
    for (const name of readdirSync(current).sort()) {
      const path = join(current, name);
      if (statSync(path).isDirectory()) visit(path);
      else found.push(path);
    }
  };
  visit(directory);
  return found.sort();
}

export function findUnsupportedPlaywrightPaths(directory: string): string[] {
  const unsupported: string[] = [];
  let entries: string[];
  try {
    entries = walkFiles(directory);
  } catch {
    return unsupported;
  }
  for (const path of entries) {
    const rel = path
      .slice(directory.length + 1)
      .split("\\")
      .join("/");
    const name = rel.slice(rel.lastIndexOf("/") + 1);
    const matched = PLAYWRIGHT_DEFAULT_SUITE.exec(name);
    if (!matched) continue;
    if (!rel.includes("/") && SPEC_BASENAME.test(name)) continue;
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

export function discoverGroups(directory = defaultE2eDir): string[][] {
  const root = resolve(directory);
  let stat;
  try {
    stat = statSync(root);
  } catch {
    throw new Fail(`missing e2e directory: ${root}`);
  }
  if (!stat.isDirectory()) throw new Fail(`missing e2e directory: ${root}`);
  const unsupported = findUnsupportedPlaywrightPaths(root);
  if (unsupported.length) {
    throw new Fail(
      "unsupported Playwright paths under normal e2e; fix or move to e2e-pending:\n" +
        unsupported.map((item) => `  - ${item}`).join("\n"),
    );
  }
  const specs = readdirSync(root)
    .filter((name) => name.endsWith(".spec.ts") && statSync(join(root, name)).isFile())
    .sort();
  if (!specs.length) throw new Fail(`no normal e2e specs under ${root}`);
  const seen = new Set<string>();
  const groups: string[][] = [];
  let pairedWiki = false;
  for (const name of specs) {
    if (!SPEC_BASENAME.test(name)) throw new Fail(`unsupported spec.ts basename: ${name}`);
    if (name === PAIR_SECOND) {
      if (pairedWiki) continue;
      throw new Fail(`${PAIR_SECOND} must be grouped with ${PAIR_FIRST}, not standalone`);
    }
    if (seen.has(name)) throw new Fail(`duplicate spec filename in discovery: ${name}`);
    if (name === PAIR_FIRST) {
      const wiki = join(root, PAIR_SECOND);
      try {
        if (!statSync(wiki).isFile()) throw new Error("missing");
      } catch {
        throw new Fail(`missing paired spec: ${wiki}`);
      }
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
  if (all.size !== seen.size || [...all].some((name) => !seen.has(name))) {
    const missing = [...all].filter((name) => !seen.has(name)).sort();
    const extra = [...seen].filter((name) => !all.has(name)).sort();
    throw new Fail(
      `discovery mismatch missing=${JSON.stringify(missing)} extra=${JSON.stringify(extra)}`,
    );
  }
  return groups;
}

export function groupSeconds(group: string[]): number {
  return Math.max(...group.map((spec) => OBSERVED_GROUP_SECONDS[spec] ?? DEFAULT_GROUP_SECONDS));
}

function compareGroups(a: string[], b: string[]): number {
  const seconds = groupSeconds(b) - groupSeconds(a);
  if (seconds !== 0) return seconds;
  const length = Math.max(a.length, b.length);
  for (let index = 0; index < length; index += 1) {
    const left = a[index];
    const right = b[index];
    if (left === right) continue;
    if (left === undefined) return -1;
    if (right === undefined) return 1;
    if (left < right) return -1;
    if (left > right) return 1;
  }
  return 0;
}

export function assignShards(groups: string[][], shardCount: number): string[][][] {
  if (shardCount < 1) throw new Fail("shard_count must be >= 1");
  const shards: string[][][] = Array.from({ length: shardCount }, () => []);
  const loads = Array.from({ length: shardCount }, () => 0);
  for (const group of [...groups].sort(compareGroups)) {
    let index = 0;
    for (let cursor = 1; cursor < shardCount; cursor += 1) {
      const load = loads[cursor] ?? 0;
      const best = loads[index] ?? 0;
      if (load < best || (load === best && cursor < index)) index = cursor;
    }
    shards[index]?.push(group);
    loads[index] = (loads[index] ?? 0) + groupSeconds(group);
  }
  return shards;
}

function sameGroups(left: string[][], right: string[][]): boolean {
  const key = (groups: string[][]) =>
    [...groups]
      .map((group) => group.join("\0"))
      .sort()
      .join("\n");
  return key(left) === key(right);
}

export function verifyKnownGroups(
  directory: string,
  shardCount: number,
  groups: string[][],
  shards = assignShards(groups, shardCount),
): { group_count: number; spec_count: number; shard_count: number; groups_per_shard: number[] } {
  const root = resolve(directory);
  const assigned = shards.flat();
  if (!sameGroups(assigned, groups)) {
    throw new Fail("shard assignment has missing, duplicate or altered groups");
  }
  const specPaths: string[] = [];
  for (const group of groups) {
    for (const spec of group) validateSpecRelpath(spec);
    specPaths.push(...group);
  }
  if (new Set(specPaths).size !== specPaths.length) {
    throw new Fail("duplicate spec membership across groups");
  }
  const expected = readdirSync(root)
    .filter((name) => name.endsWith(".spec.ts") && statSync(join(root, name)).isFile())
    .sort();
  const discovered = specPaths.map((spec) => spec.slice("e2e/".length)).sort();
  if (expected.join("\0") !== discovered.join("\0")) {
    throw new Fail(
      `unregistered or missing specs: tree=${JSON.stringify(expected)} groups=${JSON.stringify(discovered)}`,
    );
  }
  const empty = shards.flatMap((shard, index) => (shard.length ? [] : [index]));
  if (empty.length) throw new Fail(`shards with no work: ${JSON.stringify(empty)}`);
  const pair = groups.find((group) => group.length === 2);
  if (!pair || pair[0] !== `e2e/${PAIR_FIRST}` || pair[1] !== `e2e/${PAIR_SECOND}`) {
    throw new Fail(`workspace pair integrity failed: ${JSON.stringify(pair)}`);
  }
  return {
    group_count: groups.length,
    spec_count: specPaths.length,
    shard_count: shardCount,
    groups_per_shard: shards.map((shard) => shard.length),
  };
}

export function verifyPlan(directory: string | undefined, shardCount: number) {
  const root = resolve(directory ?? defaultE2eDir);
  return verifyKnownGroups(root, shardCount, discoverGroups(root));
}

export function shardPlanLines(directory: string | undefined, index: number, shardCount: number) {
  const groups = discoverGroups(directory);
  const shards = assignShards(groups, shardCount);
  if (index < 0 || index >= shardCount) {
    throw new Fail(`shard index ${index} out of range 0..${shardCount - 1}`);
  }
  const shardGroups = shards[index] ?? [];
  if (!shardGroups.length) throw new Fail(`shard ${index} has no groups`);
  return shardGroups.map((group) => {
    for (const spec of group) validateSpecRelpath(spec);
    return { specs: group };
  });
}

function printLines(lines: { specs: string[] }[]) {
  for (const line of lines) console.log(JSON.stringify(line));
}

function usage(): never {
  console.error(
    "usage: groups.ts list-groups | shard-jsonl --index N [--shards 8] | verify [--shards 8]",
  );
  process.exit(2);
}

function integerFlag(argv: string[], name: string, fallback?: number): number {
  const index = argv.indexOf(name);
  if (index < 0) {
    if (fallback === undefined) usage();
    return fallback;
  }
  const raw = argv[index + 1];
  if (raw === undefined || !/^-?\d+$/.test(raw)) usage();
  return Number(raw);
}

if (import.meta.main) {
  const argv = process.argv.slice(2);
  const commandName = argv[0];
  try {
    if (commandName === "list-groups") printLines(discoverGroups().map((specs) => ({ specs })));
    else if (commandName === "shard-jsonl") {
      printLines(
        shardPlanLines(
          undefined,
          integerFlag(argv, "--index"),
          integerFlag(argv, "--shards", DEFAULT_SHARD_COUNT),
        ),
      );
    } else if (commandName === "verify") {
      console.log(
        JSON.stringify(verifyPlan(undefined, integerFlag(argv, "--shards", DEFAULT_SHARD_COUNT))),
      );
    } else usage();
  } catch (error) {
    if (error instanceof Fail) {
      console.error(error.message);
      process.exit(error.code);
    }
    throw error;
  }
}
