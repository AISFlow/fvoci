import { deepEquals, spawnSync } from "bun";
import { strict as assert } from "node:assert";
import { constants, existsSync, lstatSync, readdirSync, realpathSync, statSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import process from "node:process";
import {
  accessible,
  below,
  call,
  env,
  gid,
  groups,
  inventory,
  observedExit,
  physical,
  read,
  root,
  sha,
  tool,
  uid,
} from "./io.ts";
import type { AccessReceipt, Browser, Bundle, Inputs, LocalGrant, Web } from "./types.ts";

const localModes = ["record-before", "stage", "record-after", "run"];
export function allocationExpiry(value: string): number {
  assert.ok(
    /^[0-9]{4}-[0-9]{2}-[0-9]{2}T(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](?:\.[0-9]{1,6})?(?:Z|[+-](?:[01][0-9]|2[0-3]):[0-5][0-9])$/.test(
      value,
    ),
    "timezone ISO allocation expiry required",
  );
  const day = value.slice(0, 10),
    calendar = new Date(day + "T00:00:00Z"),
    expiry = Date.parse(value);
  assert.ok(
    Number(day.slice(0, 4)) >= 1 &&
      Number.isFinite(calendar.getTime()) &&
      calendar.toISOString().slice(0, 10) === day &&
      Number.isFinite(expiry),
    "invalid ISO allocation expiry",
  );
  return expiry;
}
export function localAllocation(mode: string): LocalGrant {
  assert.equal(process.env.FVOCI_SELECTED_EXECUTION_MODE, "orca-local");
  assert.ok(!process.env.CI && !Object.keys(process.env).some((key) => key.startsWith("GITHUB_")));
  const path = env("FVOCI_SELECTED_LOCAL_ALLOCATION");
  assert.ok(isAbsolute(path) && !lstatSync(path).isSymbolicLink());
  const facts = statSync(path);
  assert.ok(facts.uid === uid() && (facts.mode & 0o777) === 0o600);
  assert.equal(sha(path), env("FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256"));
  const grant = read(path) as LocalGrant;
  assert.ok(
    grant.schema === 1 && grant.status === "GRANTED" && grant.executionMode === "orca-local",
  );
  assert.equal(grant.exclusiveLocalBatch, true);
  assert.equal(grant.currentDispatchConfirmed, true);
  assert.ok(grant.owner === env("FVOCI_CI_OWNER") && grant.owner === env("FVOCI_ROOT_RUN_OWNER"));
  for (const [key, name, pattern] of [
    ["runId", "FVOCI_LOCAL_RUN_ID", /^run_[0-9a-f]+$/],
    ["dispatchId", "FVOCI_LOCAL_DISPATCH_ID", /^ctx_[0-9a-f]+$/],
    ["taskId", "FVOCI_LOCAL_TASK_ID", /^task_[0-9a-f]+$/],
  ] as const)
    assert.ok(grant[key] === env(name) && pattern.test(grant[key]));
  assert.equal(grant.workerTerminal, env("ORCA_TERMINAL_HANDLE"));
  assert.equal(grant.rootTerminal, env("FVOCI_LOCAL_ROOT_TERMINAL"));
  assert.notEqual(grant.workerTerminal, grant.rootTerminal);
  assert.equal(grant.worktree, root);
  assert.notEqual(uid(), 0);
  assert.equal(grant.uid, uid());
  assert.equal(grant.gid, gid());
  assert.ok(/^[0-9a-f]{40}$/.test(grant.source) && /^[0-9a-f]{40}$/.test(grant.tree));
  assert.equal(call(["git", "rev-parse", "HEAD"]), grant.source);
  assert.equal(call(["git", "rev-parse", "HEAD^{tree}"]), grant.tree);
  assert.equal(
    observedExit(
      spawnSync(["git", "-c", "safe.directory=" + root, "diff", "--quiet", "HEAD"], { cwd: root }),
    ),
    0,
  );
  assert.ok(
    grant.allowedModes.includes(mode) &&
      grant.allowedModes.every((value) => localModes.includes(value)),
  );
  assert.ok(Date.now() < allocationExpiry(grant.expiresUtc), "expired local allocation");
  assert.ok(
    isAbsolute(grant.outputRoot) &&
      resolve(grant.outputRoot, "runtime") === env("FVOCI_CI_SELECTED_RUNS"),
  );
  for (const name of [
    "run-selected-backend-e2e.py",
    "selected-backend-ci/current_binding.py",
    "selected-backend-ci/restart_checkpoint.py",
    "selected-backend-ci/current-install-driver.py",
    "selected-backend-ci/current-postgres-driver.py",
    "selected-backend-ci/current-sqlite-driver.py",
  ]) {
    assert.equal(grant.registrationHashes[name], sha(join(root, "scripts", name)));
  }
  return grant;
}

export function identity(mode = "handoff", output?: string): string {
  const execution = process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci";
  assert.ok(execution === "github-ci" || execution === "orca-local");
  if (execution === "orca-local") {
    const grant = localAllocation(mode);
    assert.equal(output, grant.outputRoot);
    return grant.owner;
  }
  assert.ok(
    process.env.CI === "true" && process.env.GITHUB_ACTIONS === "true",
    "allocated GitHub CI job only",
  );
  assert.equal(call(["git", "rev-parse", "HEAD"]), env("GITHUB_SHA"));
  assert.equal(
    observedExit(
      spawnSync(["git", "-c", "safe.directory=" + root, "diff", "--quiet", "HEAD"], { cwd: root }),
    ),
    0,
    "current tracked source must equal tested SHA",
  );
  assert.ok(/^[0-9]+$/.test(env("GITHUB_RUN_ID")) && /^[0-9]+$/.test(env("GITHUB_RUN_ATTEMPT")));
  if (process.env.GITHUB_JOB === "collaboration-build") {
    assert.ok(["handoff", "record-before", "stage", "record-after"].includes(mode));
    assert.equal(process.env.FVOCI_WEB_BUILD_PHASE, "prepare");
  } else {
    assert.ok(
      [
        "collaboration-flow",
        "collaboration-install-on",
        "collaboration-postgres-on",
        "collaboration-sqlite-on",
        "collaboration-postgres-off",
        "collaboration-sqlite-off",
      ].includes(env("GITHUB_JOB")),
    );
    assert.ok(
      process.env.FVOCI_WEB_BUILD_PHASE === undefined ||
        process.env.FVOCI_WEB_BUILD_PHASE === "consume",
    );
  }
  return (
    "github:" +
    ["GITHUB_REPOSITORY", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB"]
      .map((key) => env(key))
      .join(":")
  );
}

export type BrowserInventory = Record<
  string,
  string | { sha256?: string; directory?: true; mode: number }
>;
export function browserInventory(
  directory: string,
  owner?: readonly [number, number],
  metadata = false,
): BrowserInventory {
  assert.ok(!lstatSync(directory).isSymbolicLink() && statSync(directory).isDirectory());
  const result: BrowserInventory = {};
  function visit(path: string, key: string): void {
    const facts = lstatSync(path);
    assert.ok(facts.isFile() || facts.isDirectory(), "nonregular browser asset");
    if (owner) assert.ok(facts.uid === owner[0] && facts.gid === owner[1] && owner[1] >= 1000);
    if (facts.isFile())
      result[key] = metadata ? { sha256: sha(path), mode: facts.mode & 0o7777 } : sha(path);
    else {
      if (metadata) result[key] = { directory: true, mode: facts.mode & 0o7777 };
      for (const child of readdirSync(path).sort())
        visit(join(path, child), key === "." ? child : key + "/" + child);
    }
  }
  visit(directory, ".");
  assert.ok(Object.keys(result).length, "empty browser closure");
  return result;
}
export interface BrowserStage {
  source: string;
  cache: string;
  chromium: string;
  files: Record<string, BrowserInventory>;
  metadata: Record<string, BrowserInventory>;
}
export function admittedBrowser(
  output: string,
  owner: readonly [number, number] = [1000, 1000],
): string {
  const receipt = read(join(output, "runtime-browser-stage.json")) as BrowserStage;
  assert.equal(receipt.source, env("GITHUB_SHA"));
  const directory = join(output, "browser");
  assert.ok(receipt.cache === directory && process.env.PLAYWRIGHT_BROWSERS_PATH === directory);
  const facts = statSync(directory);
  assert.ok(facts.uid === owner[0] && (facts.mode & 0o777) === 0o700);
  const entries = readdirSync(directory);
  assert.ok(
    deepEquals(
      Object.fromEntries(
        entries.map((name) => [name, browserInventory(join(directory, name), owner)]),
      ),
      receipt.files,
    ),
  );
  assert.ok(
    deepEquals(
      Object.fromEntries(
        entries.map((name) => [name, browserInventory(join(directory, name), owner, true)]),
      ),
      receipt.metadata,
    ),
  );
  assert.ok(below(receipt.chromium, directory) && statSync(receipt.chromium).isFile());
  return receipt.chromium;
}
export function runtimeAccess(files: string[], browser: Browser): void {
  assert.ok(uid() === 1000 && gid() === 1000);
  for (const path of files) assert.ok(statSync(path).isFile() && accessible(path));
  const directory = resolve(browser.chromium.path, "..");
  assert.ok(deepEquals(browserInventory(directory), browser.chromium_directory_files));
  for (const path of [browser.bun.path, browser.chromium.path])
    assert.ok(accessible(path, constants.R_OK | constants.X_OK));
}
export function expectedFiles(bundle: Bundle, output: string): string[] {
  const names = [
    "before.json",
    "after.json",
    "build-env-inputs.json",
    "build-environment.json",
    "compile-receipt.json",
    "bundle.json",
    "web-receipt.json",
    "abi-receipt.json",
  ];
  for (const name of ["main", "lib", "install", "engine"])
    names.push(name + "-stage.json", name + "-compiler.jsonl");
  const core = new Set(
    bundle.compiler_artifacts
      .filter(
        (a) =>
          a.target.name === "fvoci_server" &&
          !a.profile.test &&
          deepEquals(a.features, ["api-schema", "db-tests"]),
      )
      .flatMap((a) => a.filenames.filter((p) => /\.(rlib|rmeta)$/.test(p))),
  );
  assert.ok(
    [...core].some((p) => p.endsWith(".rlib")),
    "missing emitted core library",
  );
  const result = [
    ...names.map((name) => join(output, name)),
    ...Object.keys(bundle.binaries).sort(),
    ...[...core].sort(),
  ];
  assert.equal(result.length, new Set(result).size);
  return result;
}
export interface Consumed {
  source: string;
  tree: string;
  repository: string;
  run: string;
  attempt: string;
  full_current_physical_inputs_equal: boolean;
  fresh_dist_equal: boolean;
  received: Record<string, { sha256: string; inode: number; mode: number }>;
}
export function configListInputs(output: string): {
  before: Inputs;
  browser: Browser;
  modules: Record<string, string>;
} {
  assert.equal(process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci", "github-ci");
  assert.equal(process.env.FVOCI_WEB_BUILD_PHASE, "consume");
  assert.ok(uid() === 1000 && gid() === 1000);
  const owner = identity("config-list", output);
  assert.ok(
    !existsSync(join(output, "runtime")) &&
      !readdirSync(output).some((p) => /-(allocation|binding)\.json$/.test(p)),
  );
  const consumed = read(join(output, "handoff-consumed.json")) as Consumed,
    before = read(join(output, "before.json")) as Inputs;
  assert.ok(consumed.source === before.head && before.head === env("GITHUB_SHA"));
  assert.ok(
    consumed.tree === before.tree && before.tree === call(["git", "rev-parse", "HEAD^{tree}"]),
  );
  assert.ok(
    consumed.repository === env("GITHUB_REPOSITORY") &&
      consumed.run === env("GITHUB_RUN_ID") &&
      consumed.attempt === env("GITHUB_RUN_ATTEMPT"),
  );
  assert.equal(consumed.full_current_physical_inputs_equal, true);
  assert.equal(consumed.fresh_dist_equal, true);
  assert.ok(deepEquals(read(join(output, "after.json")), before));
  assert.equal(call(["git", "status", "--short"]), before.status.trim());
  assert.ok(
    deepEquals(
      new Set(call(["git", "ls-files", "-z"]).split("\0").filter(Boolean)),
      new Set(Object.keys(before.tracked)),
    ),
  );
  for (const [values, base] of [
    [before.tracked, root],
    [before.external, "/"],
    [before.untracked, root],
  ] as const)
    for (const [name, digest] of Object.entries(values))
      assert.equal(sha(resolve(base, name)), digest);
  const bundle = read(join(output, "bundle.json")) as Bundle;
  assert.ok(
    deepEquals(new Set(Object.keys(consumed.received)), new Set(expectedFiles(bundle, output))),
  );
  for (const [path, recorded] of Object.entries(consumed.received)) {
    physical(path);
    assert.ok(lstatSync(path).isFile());
    const facts = statSync(path);
    assert.ok(Number.isSafeInteger(facts.ino), "inode must retain exact integer identity");
    assert.ok(
      sha(path) === recorded.sha256 &&
        facts.ino === recorded.inode &&
        (facts.mode & 0o7777) === recorded.mode,
    );
  }
  assert.ok(bundle.source === before.head && bundle.tree === before.tree);
  const web = read(join(output, "web-receipt.json")) as Web;
  assert.ok(
    web.source === before.head &&
      web.tree === before.tree &&
      deepEquals(web.dist_files, inventory(join(root, "apps/web/dist"))),
  );
  const abi = read(join(output, "abi-receipt.json")) as {
    currentSource: string;
    host_runtime_files: Record<string, string>;
  };
  assert.equal(abi.currentSource, before.head);
  for (const [path, digest] of Object.entries(abi.host_runtime_files))
    assert.equal(sha(path), digest);
  const access = read(join(output, "runtime-access-stage.json")) as AccessReceipt;
  assert.ok(access.source === before.head && access.tree === before.tree && access.owner === owner);
  assert.ok(
    access.runtime_uid === 1000 &&
      access.runtime_gid === 1000 &&
      access.preflight_exit === 0 &&
      access.preflight.missing === 0,
  );
  assert.ok(deepEquals(groups(), access.groups));
  const bun = realpathSync(tool("bun"));
  assert.equal((read(join(output, "build-environment.json")) as { bun: string }).bun, "1.4.2");
  assert.equal(sha(bun), before.external[bun]);
  const chromium = admittedBrowser(output);
  const browser: Browser = {
    bun: { path: bun, sha256: sha(bun) },
    chromium: { path: chromium, sha256: sha(chromium) },
    chromium_directory_files: inventory(resolve(chromium, "..")),
  };
  runtimeAccess(
    [
      ...Object.keys(before.tracked).map((p) => join(root, p)),
      ...Object.keys(before.external),
      ...Object.keys(bundle.binaries),
    ],
    browser,
  );
  for (const path of Object.keys(bundle.binaries))
    assert.ok(accessible(path, constants.R_OK | constants.X_OK));
  const modules: Record<string, string> = {};
  for (const name of ["@playwright/test", "playwright", "playwright-core"]) {
    const path = join(root, "node_modules", name, "package.json"),
      data = read(path) as { version: string; bin?: { playwright: string } };
    assert.equal(data.version, "1.63.0");
    if (name === "playwright") assert.equal(data.bin?.playwright, "cli.js");
    modules[name] = sha(path);
  }
  const cli = join(root, "node_modules/playwright/cli.js");
  assert.ok(!lstatSync(cli).isSymbolicLink() && statSync(cli).isFile() && accessible(cli));
  assert.equal(sha(cli), before.external[cli]);
  modules["playwright/cli.js"] = sha(cli);
  return { before, browser, modules };
}
