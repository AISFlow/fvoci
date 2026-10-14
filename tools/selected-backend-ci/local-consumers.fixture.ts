// Local consumer admission cases for local-consumers.test.ts only. Run through
// setpriv as the actor under test: `bun local-consumers.fixture.ts <purpose>
// <fault>` builds an isolated checkout, grant and output root owned by that
// actor, calls the real entry once in a child and prints its exit and stderr.
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";
import { keycloakModules, registeredModules, tursoUiModules } from "./admission.ts";
import { digest, gid, root, sha, uid } from "./io.ts";
import type { LocalConsumerGrant } from "./types.ts";

export const faults = [
  "none",
  "purpose",
  "legacy",
  "equal-work",
  "equal-work-selected",
  "other-entry",
  "extra-mode",
  "missing-mode",
  "module",
  "missing-module",
  "extra-module",
  "source",
  "tree",
  "dirty",
  "actor",
  "terminal",
  "owner",
  "expiry",
  "output",
  "manifest",
  "input-digest",
  "public-grant",
  "ci",
  "selected-entry",
  "command",
  "sso-command",
  "sso-changed-command",
  "untracked",
  "runtime",
  "runtime-manifest",
  "runtime-physical",
  "runtime-target",
  "runtime-ca",
  "runtime-output",
  "runtime-binary",
  "runtime-cross-purpose",
  "runtime-observer-missing",
  "runtime-observer-foreign-directory",
] as const;
export type Fault = (typeof faults)[number];
export const purposes = ["keycloak", "turso-ui"] as const;
export type Purpose = (typeof purposes)[number];

function command(argv: string[], cwd: string): string {
  const result = Bun.spawnSync(argv, { cwd, env: { PATH: process.env.PATH }, stderr: "pipe" });
  if (result.exitCode !== 0) throw new Error("isolated admission fixture command failed");
  return result.stdout.toString().trim();
}

function binding(grant: LocalConsumerGrant): string {
  return digest(
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
  );
}

function fixture(purpose: Purpose, scratch: string) {
  const checkout = mkdtempSync(join(scratch, "checkout-"));
  const output = mkdtempSync(join(scratch, "output-"));
  const modules = purpose === "keycloak" ? keycloakModules : tursoUiModules;
  for (const name of new Set([...modules, ...registeredModules])) {
    const path = join(checkout, name);
    mkdirSync(dirname(path), { recursive: true });
    if (
      [
        "tools/selected-backend-ci/admission.ts",
        "tools/selected-backend-ci/io.ts",
        "tools/selected-backend-ci/types.ts",
      ].includes(name)
    )
      copyFileSync(join(root, name), path);
    else writeFileSync(path, "// isolated hash-only registration witness\n");
  }
  command(["git", "init", "--quiet"], checkout);
  command(["git", "add", "tools", "apps", "scripts"], checkout);
  command(
    [
      "git",
      "-c",
      "user.name=Admission fixture",
      "-c",
      "user.email=fixture@example.invalid",
      "-c",
      "commit.gpgsign=false",
      "commit",
      "--quiet",
      "-m",
      "isolated source",
    ],
    checkout,
  );
  const grant: LocalConsumerGrant = {
    schema: 1,
    status: "GRANTED",
    executionMode: "orca-local",
    exclusiveLocalBatch: true,
    currentDispatchConfirmed: true,
    owner: "orca:admission-fixture",
    runId: "run_ab12",
    taskId: "task_cd34",
    dispatchId: "ctx_ef56",
    workerTerminal: "fixture-worker",
    rootTerminal: "fixture-root",
    worktree: checkout,
    uid: uid(),
    gid: gid(),
    purpose,
    source: command(["git", "rev-parse", "HEAD"], checkout),
    tree: command(["git", "rev-parse", "HEAD^{tree}"], checkout),
    allowedModes: ["record-before", "freeze", "current-build"],
    expiresUtc: "2099-01-01T00:00:00Z",
    outputRoot: output,
    registrationHashes: Object.fromEntries(
      modules.map((name) => [name, sha(join(checkout, name))]),
    ),
    stageCommands: {
      "record-before": [
        process.execPath,
        "--no-env-file",
        join(checkout, purpose === "keycloak" ? "tools/keycloak/run.ts" : "tools/turso/ui.ts"),
        "--record-before",
      ],
    },
    heavy: { path: process.execPath, sha256: sha(process.execPath) },
    inputSha256: "",
  };
  const path = join(output, "grant.json");
  const env = {
    PATH: process.env.PATH ?? "",
    FVOCI_SELECTED_EXECUTION_MODE: "orca-local",
    FVOCI_SELECTED_LOCAL_ALLOCATION: path,
    FVOCI_CI_OWNER: grant.owner,
    FVOCI_ROOT_RUN_OWNER: grant.owner,
    FVOCI_LOCAL_RUN_ID: grant.runId,
    FVOCI_LOCAL_TASK_ID: grant.taskId,
    FVOCI_LOCAL_DISPATCH_ID: grant.dispatchId,
    ORCA_TERMINAL_HANDLE: grant.workerTerminal,
    FVOCI_LOCAL_ROOT_TERMINAL: grant.rootTerminal,
    FVOCI_CI_SELECTED_RUNS: join(output, "runtime"),
    RUNNER_TEMP: output,
    FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256: "",
  };
  return { checkout, grant, path, env };
}

function admit(purpose: Purpose, fault: Fault, scratch: string) {
  const f = fixture(purpose, scratch);
  let mode = "record-before";
  let entry = purpose === "keycloak" ? "localKeycloakAllocation" : "localTursoUiAllocation";
  if (fault === "purpose") f.grant.purpose = purpose === "keycloak" ? "turso-ui" : "keycloak";
  if (fault === "legacy") Reflect.deleteProperty(f.grant, "purpose");
  if (fault === "equal-work" || fault === "equal-work-selected")
    Reflect.set(f.grant, "purpose", "equal-work");
  if (fault === "other-entry")
    entry = purpose === "keycloak" ? "localTursoUiAllocation" : "localKeycloakAllocation";
  if (fault === "extra-mode") f.grant.allowedModes.push("run-anything");
  if (fault === "missing-mode") f.grant.allowedModes = ["freeze"];
  if (fault === "module") f.grant.registrationHashes["tools/turso/ui-record.ts"] = "0".repeat(64);
  if (fault === "missing-module")
    Reflect.deleteProperty(f.grant.registrationHashes, "tools/turso/ui-record.ts");
  if (fault === "extra-module") f.grant.registrationHashes["unregistered.ts"] = "0".repeat(64);
  if (fault === "source") f.grant.source = "0".repeat(40);
  if (fault === "tree") f.grant.tree = "0".repeat(40);
  if (fault === "dirty")
    writeFileSync(join(f.checkout, "tools/turso/ui-record.ts"), "// dirty source\n");
  if (fault === "actor") f.grant.uid = 0;
  if (fault === "terminal") f.grant.workerTerminal = f.grant.rootTerminal;
  if (fault === "owner") f.grant.owner = "foreign-owner";
  if (fault === "expiry") f.grant.expiresUtc = "2000-01-01T00:00:00Z";
  if (fault === "output") f.env.RUNNER_TEMP = f.checkout;
  if (fault === "manifest") mode = "current-build";
  if (fault === "selected-entry") {
    entry = "localAllocation";
    mode = "run";
  }
  if (fault === "equal-work-selected") {
    entry = "localAllocation";
    mode = "run";
    f.grant.allowedModes = ["run"];
    f.grant.registrationHashes = Object.fromEntries(
      registeredModules.map((name) => [name, sha(join(f.checkout, name))]),
    );
  }
  if (fault === "command") f.grant.stageCommands["record-before"]?.push("--unregistered");
  if (fault === "sso-command" || fault === "sso-changed-command") {
    f.grant.stageCommands.sso = [
      f.grant.heavy.path,
      "cargo",
      "test",
      "--locked",
      "--offline",
      "--features",
      "db-tests",
      "--test",
      "identity_integration",
      "--no-run",
      "--message-format=json",
    ];
    if (fault === "sso-changed-command") f.grant.stageCommands.sso.push("--unregistered");
  }
  if (fault === "untracked") writeFileSync(join(f.checkout, "unregistered.ts"), "// untracked\n");
  if (fault.startsWith("runtime")) {
    mode = purpose === "keycloak" ? "keycloak" : "server";
    f.grant.allowedModes.push(mode);
    const build = join(f.grant.outputRoot, "turso-ui");
    mkdirSync(build, { mode: 0o700 });
    const reference = (name: string) => {
      const path = join(build, name);
      writeFileSync(path, "{}\n", { mode: 0o600 });
      return { path, sha256: sha(path) };
    };
    f.grant.build = {
      manifest: reference("current-build.json"),
      physical: reference("physical-before.private.json"),
    };
    f.grant.localLibsql = {
      binary: f.grant.heavy,
      directory: join(f.grant.outputRoot, "runtime"),
      label: "fvoci-admission-fixture",
      host: "127.0.0.1",
      port: 0,
      tls: "generated-private-san-ip-loopback",
      childOnlyTrust: true,
    };
    if (purpose === "keycloak") {
      f.grant.keycloak = {
        backends: ["libsql-remote"],
        sqld: f.grant.heavy,
        heavy: process.execPath,
      };
      f.grant.stageCommands.run = [
        process.execPath,
        "--no-env-file",
        join(f.checkout, "tools/keycloak/run.ts"),
        "--backend=libsql-remote",
      ];
    } else {
      f.grant.canonicalRuntime = {
        imageId: "sha256:" + "a".repeat(64),
        networkAuthorized: true,
        shellProof: reference("shell-proof.json"),
      };
      f.grant.stageCommands.run = [
        process.execPath,
        "--no-env-file",
        join(f.checkout, "tools/turso/ui.ts"),
        "--local-restore",
      ];
    }
    if (fault === "runtime-manifest") f.grant.build.manifest.sha256 = "0".repeat(64);
    if (fault === "runtime-physical") f.grant.build.physical.sha256 = "0".repeat(64);
    if (fault === "runtime-target") Reflect.deleteProperty(f.grant, "localLibsql");
    if (fault === "runtime-ca") Reflect.set(f.grant.localLibsql, "childOnlyTrust", false);
    if (fault === "runtime-output") f.grant.localLibsql.directory = f.checkout;
    if (fault === "runtime-binary")
      f.grant.localLibsql.binary = { path: process.execPath, sha256: "0".repeat(64) };
    if (fault === "runtime-cross-purpose") {
      if (purpose === "keycloak")
        f.grant.canonicalRuntime = {
          imageId: "sha256:" + "a".repeat(64),
          networkAuthorized: true,
          shellProof: reference("shell-proof.json"),
        };
      else f.grant.keycloak = { backends: ["sqlite"], heavy: process.execPath };
    }
    // The privileged witness is provided only by a controller allocation.
    // A unit fixture never manufactures a root-owned witness or authority.
    if (fault === "runtime" && purpose === "keycloak") mode = "current-build";
    if (fault === "runtime-observer-foreign-directory" && f.grant.keycloak) {
      f.grant.keycloak.processObserver = {
        directory: f.grant.outputRoot,
        bun: f.grant.heavy,
        module: f.grant.heavy,
        config: f.grant.heavy,
      };
    }
  }
  f.grant.inputSha256 = fault === "input-digest" ? "0".repeat(64) : binding(f.grant);
  writeFileSync(f.path, JSON.stringify(f.grant), { mode: 0o600 });
  if (fault === "public-grant") chmodSync(f.path, 0o644);
  f.env.FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256 = sha(f.path);
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "--eval",
      `import { ${entry} } from ${JSON.stringify(join(f.checkout, "tools/selected-backend-ci/admission.ts"))}; try { ${entry}(${JSON.stringify(mode)}); process.exit(0); } catch (error) { console.error(String(error)); process.exit(23); }`,
    ],
    {
      cwd: f.checkout,
      env: { ...f.env, ...(fault === "ci" ? { CI: "true" } : {}) },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  return { exitCode: result.exitCode, stderr: result.stderr.toString() };
}

if (import.meta.main) {
  const [purpose, fault] = process.argv.slice(2);
  if (
    !purposes.includes(purpose as Purpose) ||
    !faults.includes(fault as Fault) ||
    (purpose === "turso-ui" && fault?.startsWith("runtime-observer"))
  )
    throw new Error("unknown local consumer admission case");
  const scratch = mkdtempSync(join(tmpdir(), "fvoci-local-consumers-"));
  try {
    console.log(JSON.stringify(admit(purpose as Purpose, fault as Fault, scratch)));
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
