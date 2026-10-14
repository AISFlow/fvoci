import { deepEquals, spawnSync } from "bun";
import { strict as assert } from "node:assert";
import { constants, existsSync, lstatSync, readdirSync, realpathSync, statSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import process from "node:process";
import {
  accessible,
  below,
  call,
  digest,
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
import type {
  AccessReceipt,
  Browser,
  Bundle,
  Inputs,
  LocalConsumerGrant,
  LocalGrant,
  Reference,
  Web,
} from "./types.ts";

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
export function localAllocation(
  mode: string,
  expected: readonly [number, number] = [1000, 1000],
): LocalGrant {
  const grant = localIdentity(expected);
  assert.ok(
    grant.purpose === undefined || grant.purpose === "equal-work",
    "selected allocation purpose only",
  );
  assert.ok(
    grant.allowedModes.includes(mode) &&
      grant.allowedModes.every((value) => localModes.includes(value)),
  );
  registered(grant, registeredModules);
  return grant;
}

function localIdentity(expected: readonly [number, number]): LocalGrant {
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
  assert.notEqual(uid(), 0, "refusing root process");
  assert.equal(uid(), expected[0], "selected runtime actor must be the fixed handoff uid and gid");
  assert.equal(gid(), expected[1], "selected runtime actor must be the fixed handoff uid and gid");
  assert.notEqual(grant.uid, 0, "refusing root grant");
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
  assert.ok(Date.now() < allocationExpiry(grant.expiresUtc), "expired local allocation");
  assert.ok(
    isAbsolute(grant.outputRoot) &&
      resolve(grant.outputRoot, "runtime") === env("FVOCI_CI_SELECTED_RUNS"),
  );
  return grant;
}

function registered(grant: LocalGrant, modules: readonly string[]): void {
  for (const name of modules)
    assert.ok(
      grant.registrationHashes[name] === sha(join(root, name)),
      "unregistered module " + name,
    );
}
// The runner and lane driver modules a local lease is issued for: the CLI
// entry and every module it loads (they run before this check), and the lane
// drivers the runner starts as separate processes. Repo-relative; the lease
// producer hashes the same files.
export const registeredModules = [
  "scripts/run-selected-backend-e2e.ts",
  "tools/selected-backend-ci/admission.ts",
  "tools/selected-backend-ci/build.ts",
  "tools/selected-backend-ci/config-list.ts",
  "tools/selected-backend-ci/io.ts",
  "tools/selected-backend-ci/runtime.ts",
  "tools/selected-backend-ci/drivers/common.ts",
  "tools/selected-backend-ci/drivers/binding.ts",
  "tools/selected-backend-ci/drivers/restart.ts",
  "tools/selected-backend-ci/drivers/install.ts",
  "tools/selected-backend-ci/drivers/postgres.ts",
  "tools/selected-backend-ci/drivers/sqlite.ts",
] as const;

const consumerCommonModules = [
  "tools/selected-backend-ci/admission.ts",
  "tools/selected-backend-ci/types.ts",
  "tools/selected-backend-ci/build.ts",
  "tools/selected-backend-ci/io.ts",
  "tools/web-e2e/compat.ts",
  "tools/turso/ui-record.ts",
  "tools/turso/ui-common.ts",
  "tools/turso/ui-processes.ts",
  "tools/turso/ui-server.ts",
  "tools/turso/ui-start-diagnostic.ts",
] as const;

export const tursoUiModules = Object.freeze([
  ...consumerCommonModules,
  "tools/turso/ui.ts",
  "tools/turso/ui-flow.ts",
  "tools/turso/ui-native.ts",
  "tools/turso/ui-audit.ts",
  "tools/turso/ui-container.ts",
  "tools/turso/ui-browser-diagnostic.ts",
  "tools/turso/ui-restore.ts",
  "tools/turso/guard.ts",
  "tools/turso/guard-policy.ts",
  "tools/turso/guard-io.ts",
  "tools/turso/guard-receipts.ts",
  "tools/turso/python-compat.ts",
  "tools/keycloak/local-libsql.ts",
  "tools/keycloak/redact.ts",
  "apps/web/e2e-pending/turso-restore-playwright.config.ts",
  "apps/web/e2e-pending/v060-turso-restore.spec.ts",
  "apps/web/e2e-pending/turso-restore-helpers.ts",
  "apps/web/e2e-pending/collab-playwright.config.ts",
  "apps/web/e2e-pending/collab-helpers.ts",
  "apps/web/e2e-pending/collab-restart.ts",
  "apps/web/e2e-pending/collab-wire.ts",
  "apps/web/e2e/helpers.ts",
] as const);

export const keycloakModules = Object.freeze([
  ...consumerCommonModules,
  "tools/keycloak/run.ts",
  "tools/keycloak/local-libsql.ts",
  "tools/keycloak/realm.ts",
  "tools/keycloak/admin.ts",
  "tools/keycloak/redact.ts",
  "apps/web/e2e-keycloak/config.ts",
  "apps/web/e2e-keycloak/observer.ts",
  "apps/web/e2e-keycloak/keycloak.config.ts",
  "apps/web/e2e-keycloak/oidc-keycloak-flow.spec.ts",
  "apps/web/e2e/helpers.ts",
  "tools/web-e2e/database-urls.ts",
  "tools/selected-backend-ci/drivers/postgres.ts",
  "tools/selected-backend-ci/drivers/binding.ts",
  "tools/selected-backend-ci/drivers/common.ts",
  "tools/selected-backend-ci/drivers/restart.ts",
] as const);

const tursoUiModes = ["record-before", "freeze", "current-build", "fixture", "server", "actor"];
const keycloakModes = ["record-before", "freeze", "current-build", "keycloak"];

function ownedDirectory(path: string): void {
  assert.ok(isAbsolute(path) && realpathSync(path) === path, "physical owned directory required");
  const facts = lstatSync(path);
  assert.ok(facts.isDirectory() && facts.uid === uid() && facts.gid === gid());
  assert.equal(facts.mode & 0o777, 0o700, "private owned directory required");
}

function admittedReference(reference: Reference): void {
  assert.ok(isAbsolute(reference.path));
  assert.ok(!lstatSync(reference.path).isSymbolicLink() && statSync(reference.path).isFile());
  assert.match(reference.sha256, /^[0-9a-f]{64}$/);
  assert.equal(sha(reference.path), reference.sha256, "local input changed");
}

function admittedCommands(grant: LocalConsumerGrant): void {
  const entry = join(
    root,
    grant.purpose === "keycloak" ? "tools/keycloak/run.ts" : "tools/turso/ui.ts",
  );
  const expected: Record<string, string[]> = {
    "record-before": [process.execPath, "--no-env-file", entry, "--record-before"],
    freeze: [process.execPath, "--no-env-file", entry, "--freeze"],
    native: [
      grant.heavy.path,
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--features",
      "api-schema,db-tests",
      "--jobs",
      "2",
      "--bin",
      "fvoci-server",
      "--bin",
      "fvoci-migrate",
      "--bin",
      "fvoci-e2e-fixture",
      "--message-format=json",
    ],
    engine: [
      grant.heavy.path,
      "cargo",
      "build",
      "--locked",
      "--offline",
      "--manifest-path",
      "crates/collab-engine/Cargo.toml",
      "--features",
      "worker",
      "--jobs",
      "2",
      "--bin",
      "collab-engine",
      "--message-format=json",
    ],
    web: [process.execPath, "--bun", "run", "--cwd", "apps/web", "build"],
  };
  if (grant.purpose === "turso-ui")
    expected.run = [process.execPath, "--no-env-file", entry, "--local-restore"];
  else if (grant.stageCommands.run) {
    const command = grant.stageCommands.run;
    assert.ok(command, "exact Keycloak runtime command required");
    const backend = command[3]?.replace(/^--backend=/, "");
    assert.ok(backend);
    assert.ok(
      grant.keycloak?.backends.includes(backend as "postgres" | "sqlite" | "libsql-remote"),
    );
    expected.run = [process.execPath, "--no-env-file", entry, "--backend=" + backend];
    if (command.length === 5) {
      assert.ok(backend === "postgres" && grant.keycloak?.sso);
      expected.run.push("--workspace-sso");
    }
  }
  assert.ok(Object.keys(grant.stageCommands).length > 0);
  for (const [stage, argv] of Object.entries(grant.stageCommands))
    assert.ok(
      expected[stage] && deepEquals(argv, expected[stage]),
      "unapproved consumer stage argv",
    );
}

function consumerAllocation(mode: string, purpose: "turso-ui" | "keycloak"): LocalConsumerGrant {
  const grant = localIdentity([1000, 1000]) as LocalConsumerGrant;
  assert.equal(call(["git", "status", "--short"]), "", "current consumer source must be clean");
  assert.equal(grant.purpose, purpose, "local consumer purpose mismatch");
  if (purpose === "turso-ui")
    assert.equal(grant.keycloak, undefined, "Turso allocation cannot grant Keycloak");
  else
    assert.equal(
      grant.canonicalRuntime,
      undefined,
      "Keycloak allocation cannot grant Turso containers",
    );
  const modes = purpose === "turso-ui" ? tursoUiModes : keycloakModes;
  assert.ok(
    grant.allowedModes.includes(mode) && grant.allowedModes.every((v) => modes.includes(v)),
  );
  const modules = purpose === "turso-ui" ? tursoUiModules : keycloakModules;
  assert.ok(deepEquals(Object.keys(grant.registrationHashes).sort(), [...modules].sort()));
  registered(grant, modules);
  ownedDirectory(grant.outputRoot);
  assert.equal(env("RUNNER_TEMP"), grant.outputRoot);
  admittedReference(grant.heavy);
  assert.ok(accessible(grant.heavy.path, constants.R_OK | constants.X_OK));
  admittedCommands(grant);
  if (mode === "record-before" || mode === "freeze")
    assert.ok(grant.stageCommands[mode], "current build stage argv required");
  else if (mode !== "current-build")
    assert.ok(grant.stageCommands.run, "current runtime stage argv required");
  assert.match(grant.inputSha256, /^[0-9a-f]{64}$/);
  assert.equal(
    grant.inputSha256,
    digest(
      JSON.stringify({
        purpose: grant.purpose,
        source: grant.source,
        tree: grant.tree,
        registrationHashes: grant.registrationHashes,
        stageCommands: grant.stageCommands,
        heavy: grant.heavy,
        build: grant.build ?? null,
        localLibsql: grant.localLibsql ?? null,
        canonicalRuntime: grant.canonicalRuntime ?? null,
        keycloak: grant.keycloak ?? null,
      }),
    ),
    "local consumer input binding changed",
  );
  if (mode === "record-before" || mode === "freeze") return grant;
  assert.ok(grant.build, "current build seal required");
  assert.equal(grant.build.manifest.path, join(grant.outputRoot, "turso-ui/current-build.json"));
  assert.equal(
    grant.build.physical.path,
    join(grant.outputRoot, "turso-ui/physical-before.private.json"),
  );
  admittedReference(grant.build.manifest);
  admittedReference(grant.build.physical);
  if (mode === "current-build") return grant;
  if (purpose === "turso-ui" || grant.keycloak?.backends.includes("libsql-remote")) {
    const local = grant.localLibsql;
    assert.ok(local, "owned local libSQL fixture required");
    assert.equal(local.host, "127.0.0.1");
    assert.equal(local.port, 0);
    assert.equal(local.tls, "generated-private-san-ip-loopback");
    assert.equal(local.childOnlyTrust, true);
    assert.match(local.label, /^fvoci-[a-z0-9-]+$/);
    assert.ok(below(local.directory, grant.outputRoot) && local.directory !== grant.outputRoot);
    admittedReference(local.binary);
    // The shared fixture enforces the official binary pin and proves its
    // actual child/socket/certificate before handing its private env over.
    // This is the allowed parent, not a caller-provided existing endpoint.
    // Every run allocates a new child directory below it; the shared fixture
    // observes the actual bound child/socket before releasing its env.
  }
  if (purpose === "turso-ui") {
    assert.ok(grant.canonicalRuntime?.networkAuthorized === true);
    assert.match(grant.canonicalRuntime.imageId, /^sha256:[0-9a-f]{64}$/);
    admittedReference(grant.canonicalRuntime.shellProof);
    assert.equal(grant.keycloak, undefined, "Turso allocation cannot grant Keycloak");
  } else {
    const keycloak = grant.keycloak;
    assert.ok(keycloak && keycloak.backends.length > 0);
    assert.equal(new Set(keycloak.backends).size, keycloak.backends.length);
    assert.ok(keycloak.backends.every((v) => ["postgres", "sqlite", "libsql-remote"].includes(v)));
    assert.equal(keycloak.heavy, grant.heavy.path);
    if (keycloak.sqld) {
      admittedReference(keycloak.sqld);
      assert.ok(deepEquals(keycloak.sqld, grant.localLibsql?.binary));
    }
    if (keycloak.sso) admittedReference(keycloak.sso);
    if (keycloak.chromiumTmp) {
      assert.match(keycloak.chromiumTmp, /^\/tmp\/fvoci-[a-z0-9-]+$/);
      assert.equal(realpathSync(keycloak.chromiumTmp), keycloak.chromiumTmp);
      const facts = statSync(keycloak.chromiumTmp);
      assert.ok(facts.isDirectory() && facts.uid === uid() && (facts.mode & 0o777) === 0o711);
    }
    assert.equal(
      grant.canonicalRuntime,
      undefined,
      "Keycloak allocation cannot grant Turso containers",
    );
  }
  return grant;
}

/** Each entry owns its static policy; callers cannot supply a smaller registry. */
export const localTursoUiAllocation = (mode: string): LocalConsumerGrant =>
  consumerAllocation(mode, "turso-ui");
export const localKeycloakAllocation = (mode: string): LocalConsumerGrant =>
  consumerAllocation(mode, "keycloak");

// The whole-flow job and the five per-lane jobs that start the selected runtime.
export const runtimeJobs = [
  "collaboration-flow",
  "collaboration-install-on",
  "collaboration-postgres-on",
  "collaboration-sqlite-on",
  "collaboration-postgres-off",
  "collaboration-sqlite-off",
] as const;
export function assertRuntimeJob(): void {
  const job = process.env.GITHUB_JOB;
  assert.ok(
    job !== undefined && (runtimeJobs as readonly string[]).includes(job),
    "selected runtime job only",
  );
}

export function identity(mode = "handoff", output?: string, checkout = root): string {
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
  assert.equal(call(["git", "rev-parse", "HEAD"], checkout), env("GITHUB_SHA"));
  assert.equal(
    observedExit(
      spawnSync(["git", "-c", "safe.directory=" + checkout, "diff", "--quiet", "HEAD"], {
        cwd: checkout,
      }),
    ),
    0,
    "current tracked source must equal tested SHA",
  );
  assert.ok(/^[0-9]+$/.test(env("GITHUB_RUN_ID")) && /^[0-9]+$/.test(env("GITHUB_RUN_ATTEMPT")));
  if (process.env.GITHUB_JOB === "collaboration-build") {
    assert.ok(["handoff", "record-before", "stage", "record-after"].includes(mode));
    assert.equal(process.env.FVOCI_WEB_BUILD_PHASE, "prepare");
  } else {
    assertRuntimeJob();
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
    if (owner)
      assert.ok(
        owner[0] !== 0 && owner[1] !== 0 && facts.uid === owner[0] && facts.gid === owner[1],
      );
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
export const collaborationStages = ["main", "lib", "install", "engine"] as const;
export const browserStages = ["fixture", "default", "engine"] as const;
// The packet file list of a collaboration build, or of a browser build when its
// web receipt is given (browser stages, served dist assets, no schema core).
export function expectedFiles(
  bundle: Bundle,
  output: string,
  web?: Web,
  checkout = root,
): string[] {
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
  for (const name of web ? browserStages : collaborationStages)
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
  if (web) assert.equal(core.size, 0, "browser packet must not contain schema-feature core");
  else
    assert.ok(
      [...core].some((p) => p.endsWith(".rlib")),
      "missing emitted core library",
    );
  const result = [
    ...names.map((name) => join(output, name)),
    ...Object.keys(bundle.binaries).sort(),
    ...(web
      ? Object.keys(web.dist_files).map((name) => join(checkout, "apps/web/dist", name))
      : []),
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
export function configListInputs(
  output: string,
  checkout = root,
): {
  before: Inputs;
  browser: Browser;
  modules: Record<string, string>;
} {
  assert.equal(process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci", "github-ci");
  assert.equal(process.env.FVOCI_WEB_BUILD_PHASE, "consume");
  assert.ok(uid() === 1000 && gid() === 1000);
  const owner = identity("config-list", output, checkout);
  assert.ok(
    !existsSync(join(output, "runtime")) &&
      !readdirSync(output).some((p) => /-(allocation|binding)\.json$/.test(p)),
  );
  const consumed = read(join(output, "handoff-consumed.json")) as Consumed,
    before = read(join(output, "before.json")) as Inputs;
  assert.ok(consumed.source === before.head && before.head === env("GITHUB_SHA"));
  assert.ok(
    consumed.tree === before.tree &&
      before.tree === call(["git", "rev-parse", "HEAD^{tree}"], checkout),
  );
  assert.ok(
    consumed.repository === env("GITHUB_REPOSITORY") &&
      consumed.run === env("GITHUB_RUN_ID") &&
      consumed.attempt === env("GITHUB_RUN_ATTEMPT"),
  );
  assert.equal(consumed.full_current_physical_inputs_equal, true);
  assert.equal(consumed.fresh_dist_equal, true);
  assert.ok(deepEquals(read(join(output, "after.json")), before));
  assert.equal(call(["git", "status", "--short"], checkout), before.status.trim());
  assert.ok(
    deepEquals(
      new Set(call(["git", "ls-files", "-z"], checkout).split("\0").filter(Boolean)),
      new Set(Object.keys(before.tracked)),
    ),
  );
  for (const [values, base] of [
    [before.tracked, checkout],
    [before.external, "/"],
    [before.untracked, checkout],
  ] as const)
    for (const [name, digest] of Object.entries(values))
      assert.equal(sha(resolve(base, name)), digest);
  const bundle = read(join(output, "bundle.json")) as Bundle;
  assert.ok(
    deepEquals(
      new Set(Object.keys(consumed.received)),
      new Set(expectedFiles(bundle, output, undefined, checkout)),
    ),
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
      deepEquals(web.dist_files, inventory(join(checkout, "apps/web/dist"))),
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
  assert.equal((read(join(output, "build-environment.json")) as { bun: string }).bun, "1.4.3");
  assert.equal(sha(bun), before.external[bun]);
  const chromium = admittedBrowser(output);
  const browser: Browser = {
    bun: { path: bun, sha256: sha(bun) },
    chromium: { path: chromium, sha256: sha(chromium) },
    chromium_directory_files: inventory(resolve(chromium, "..")),
  };
  runtimeAccess(
    [
      ...Object.keys(before.tracked).map((p) => join(checkout, p)),
      ...Object.keys(before.external),
      ...Object.keys(bundle.binaries),
    ],
    browser,
  );
  for (const path of Object.keys(bundle.binaries))
    assert.ok(accessible(path, constants.R_OK | constants.X_OK));
  const modules: Record<string, string> = {};
  for (const name of ["@playwright/test", "playwright", "playwright-core"]) {
    const path = join(checkout, "node_modules", name, "package.json"),
      data = read(path) as { version: string; bin?: { playwright: string } };
    assert.equal(data.version, "1.64.0");
    if (name === "playwright") assert.equal(data.bin?.playwright, "cli.js");
    modules[name] = sha(path);
  }
  const cli = join(checkout, "node_modules/playwright/cli.js");
  assert.ok(!lstatSync(cli).isSymbolicLink() && statSync(cli).isFile() && accessible(cli));
  assert.equal(sha(cli), before.external[cli]);
  modules["playwright/cli.js"] = sha(cli);
  return { before, browser, modules };
}
