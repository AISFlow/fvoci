#!/usr/bin/env bun
// Credential-free, network-free admission fixtures of the manual Turso
// workflow: the guard, the UI consumer and the turso-test.yml literal checks.
// They are not Turso runtime tests; no credential, Environment, network or
// compiled binary is used, and no dependency install is needed.
import { statSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { root as repository, type Environment } from "../selected-backend-ci/io.ts";
import { bunIsolation } from "./ui-common.ts";

/**
 * Every suite the admission step runs, relative to the repository root.
 * bun test passes when one path among several does not exist, so each one
 * is checked before the run.
 */
export const FIXTURE_SUITES = [
  "tools/turso/fixtures.test.ts",
  "tools/turso/guard-io.test.ts",
  "tools/turso/guard-policy.test.ts",
  "tools/turso/guard-receipts.test.ts",
  "tools/turso/guard.test.ts",
  "tools/turso/ui-audit.test.ts",
  "tools/turso/ui-cli.test.ts",
  "tools/turso/ui-container.test.ts",
  "tools/turso/ui-flow.test.ts",
  "tools/turso/ui-native.test.ts",
  "tools/turso/ui-processes.test.ts",
  "tools/turso/ui-start.test.ts",
  "tools/ci/verify/registry.test.ts",
] as const;

/** Only what the suites need: nothing else of the calling step reaches them. */
export function fixtureEnv(env: Environment): Record<string, string> {
  const child: Record<string, string> = { PATH: env.PATH ?? "" };
  if (env.TMPDIR !== undefined) child.TMPDIR = env.TMPDIR;
  return child;
}

export function fixtureArgv(bun: string, root: string): string[] {
  return [bun, ...bunIsolation(root), "test", ...FIXTURE_SUITES.map((suite) => "./" + suite)];
}

function isFile(path: string): boolean {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

/** Runs the pinned suites once and returns their exit status. */
export async function main(
  env: Environment,
  root: string = repository,
  bun: string = process.execPath,
): Promise<number> {
  const missing = FIXTURE_SUITES.filter((suite) => !isFile(join(root, suite)));
  if (missing.length) {
    process.stderr.write("TURSO_FIXTURE_SUITE_MISSING " + missing.join(" ") + "\n");
    return 1;
  }
  const child = Bun.spawn(fixtureArgv(bun, root), {
    cwd: root,
    env: fixtureEnv(env),
    stdio: ["ignore", "inherit", "inherit"],
  });
  await child.exited;
  // A signal is a failure too; only an exit status of 0 passes.
  return child.exitCode ?? 1;
}

if (import.meta.main) {
  process.exitCode = await main(process.env);
}
