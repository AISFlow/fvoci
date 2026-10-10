// Credential-free recording before the build and the freeze of its current
// outputs; currentBuild re-verifies every frozen identity before any use.
import { deepEquals } from "bun";
import { lstatSync, openSync, readFileSync, readSync, closeSync, statSync } from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import process from "node:process";
import { buildEnv, inputs } from "../selected-backend-ci/build.ts";
import { pySplitlines } from "../web-e2e/compat.ts";
import {
  call,
  env,
  files,
  inventory,
  resolved,
  root as checkout,
  sha,
  write,
} from "../selected-backend-ci/io.ts";
import type { LocalGrant } from "../selected-backend-ci/types.ts";
import {
  bunIsolation,
  executionMode,
  get,
  list,
  privateRead,
  require,
  root,
  UiError,
} from "./ui-common.ts";

export const PINNED_BUN = "1.4.2";
export const BINARIES = ["fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"];

export interface SourceInputs {
  source: string;
  tree: string;
  files: Record<string, string>;
  executionMode?: string;
  runId?: string;
  dispatchId?: string;
}
export interface Binary {
  path: string;
  sha256: string;
  features: string[];
  profile: Record<string, unknown>;
}
export interface Manifest {
  schema: 1;
  sourceInputs: SourceInputs;
  physicalInputs: { path: string; sha256: string };
  binaries: Record<string, Binary>;
  assets: Record<string, string>;
  abi: Record<string, string>;
  bun: { path: string; sha256: string; version: string };
  chromium: string;
  browserFiles: Record<string, string>;
  sqliteInputs: { path: string; sha256: string };
  rustc: string;
}

/**
 * Same-user ROOT lease of the orca-local mode. The maintained loader is
 * `localAllocation` in tools/selected-backend-ci/admission.ts, which does not
 * yet accept the turso-ui consumer; until it does, the local mode is refused.
 */
export type LocalLease = (mode: string) => LocalGrant & Record<string, unknown>;
export const refusedLocalLease: LocalLease = () => {
  throw new UiError("UI_LOCAL_ALLOCATION_REFUSED");
};
export function loadLocalLease(mode: string, lease: LocalLease = refusedLocalLease) {
  try {
    return lease(mode);
  } catch {
    throw new UiError("UI_LOCAL_ALLOCATION_REFUSED");
  }
}

export function sourceInputs(): SourceInputs {
  const names = call(["git", "ls-files", "-z"]).split("\0").filter(Boolean);
  require(call(["git", "status", "--short"]) === "", "UI_DIRTY_SOURCE");
  const source = call(["git", "rev-parse", "HEAD"]),
    tree = call(["git", "rev-parse", "HEAD^{tree}"]);
  return { source, tree, files: Object.fromEntries(names.map((n) => [n, sha(join(checkout, n))])) };
}

export function hostedIdentity(): SourceInputs {
  require(process.env.GITHUB_ACTIONS === "true" &&
    process.env.CI === "true" &&
    process.env.GITHUB_JOB === "turso-ui", "UI_HOSTED_ALLOCATION_REQUIRED");
  for (const name of ["GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"])
    require(/^[0-9]+$/.test(process.env[name] ?? ""), "UI_HOSTED_ALLOCATION_REQUIRED");
  const source = sourceInputs();
  require(source.source === env("GITHUB_SHA"), "UI_SOURCE_MISMATCH");
  return source;
}

export function sourceIdentity(mode: string, lease?: LocalLease): SourceInputs {
  if (executionMode() === "github-ci") return hostedIdentity();
  require(executionMode() === "orca-local", "UI_EXECUTION_MODE_REFUSED");
  const grant = loadLocalLease(mode, lease);
  const base = sourceInputs();
  require(base.source === grant.source && base.tree === grant.tree, "UI_LOCAL_SOURCE_REFUSED");
  return {
    executionMode: "orca-local",
    runId: grant.runId,
    dispatchId: grant.dispatchId,
    source: base.source,
    tree: base.tree,
    files: base.files,
  };
}

/** The maintained selected-driver collector: credential refusal, physical
 * sysroot/registry/config/compiler/SQLite/LLVM inputs and ABI. */
export const physicalInputs = () => ({ files: inputs(), buildEnvironment: buildEnv() });

export function recheckPhysical(recorded: unknown, collect: () => unknown = physicalInputs): void {
  require(deepEquals(recorded, collect(), true), "UI_PHYSICAL_BUILD_INPUTS_CHANGED");
}

export function recordBefore(lease?: LocalLease): void {
  write(join(root(), "source-before.json"), sourceIdentity("record-before", lease));
  write(join(root(), "physical-before.private.json"), physicalInputs());
}

function elf(path: string): boolean {
  const head = new Uint8Array(4),
    fd = openSync(path, "r");
  try {
    return (
      readSync(fd, head) === 4 && head.every((byte, i) => byte === [0x7f, 0x45, 0x4c, 0x46][i])
    );
  } finally {
    closeSync(fd);
  }
}
const isFile = (path: string) => {
  try {
    return statSync(path).isFile();
  } catch {
    return false;
  }
};

export function compilerArtifacts(path: string): unknown[] {
  return pySplitlines(readFileSync(path, "utf8"))
    .filter((line) => line.startsWith("{"))
    .map((line) => JSON.parse(line) as unknown)
    .filter(
      (item) =>
        typeof item === "object" &&
        item !== null &&
        (item as Record<string, unknown>).reason === "compiler-artifact" &&
        Boolean((item as Record<string, unknown>).executable),
    );
}

export function qualifyBinaries(artifacts: unknown[]): Record<string, Binary> {
  const binaries: Record<string, Binary> = {};
  for (const name of BINARIES) {
    const matches = artifacts.filter(
      (a) => get(a, "target", "name") === name && !get(a, "profile", "test"),
    );
    require(matches.length === 1, "UI_CURRENT_ARTIFACT_MISSING");
    const a = matches[0];
    const expected = name === "collab-engine" ? ["default", "worker"] : ["api-schema", "db-tests"];
    const features = list(a, "features");
    require(deepEquals([...features].sort(), expected) &&
      get(a, "profile", "opt_level") === "0" &&
      Boolean(get(a, "profile", "debug_assertions")), "UI_ARTIFACT_FEATURES_MISMATCH");
    const path = get(a, "executable") as string;
    require(typeof path === "string" &&
      isAbsolute(path) &&
      !lstatSync(path).isSymbolicLink() &&
      elf(path), "UI_ARTIFACT_REFUSED");
    binaries[name] = {
      path,
      sha256: sha(path),
      features: features as string[],
      profile: get(a, "profile") as Record<string, unknown>,
    };
  }
  return binaries;
}

export function runtimeAbi(binaries: Record<string, Binary>): Record<string, string> {
  const abi: Record<string, string> = {};
  for (const artifact of Object.values(binaries)) {
    const output = call(["ldd", artifact.path]);
    require(!output.includes("not found"), "UI_RUNTIME_ABI_MISSING");
    for (const match of output.matchAll(/(\/[\w./+-]+)/g)) {
      const path = resolved(match[1] as string);
      require(isFile(path), "UI_RUNTIME_ABI_MISSING");
      abi[path] = sha(path);
    }
  }
  return abi;
}

const dist = () => join(checkout, "apps/web/dist");

export function freeze(lease?: LocalLease): void {
  const before = privateRead(join(root(), "source-before.json"), 4 * 1024 * 1024);
  require(deepEquals(before, sourceIdentity("freeze", lease), true), "UI_BUILD_INPUTS_CHANGED");
  const physicalPath = join(root(), "physical-before.private.json");
  recheckPhysical(privateRead(physicalPath, 64 * 1024 * 1024));
  const artifacts = ["ui-compile.json", "ui-engine-compile.json"].flatMap((name) =>
    compilerArtifacts(join(env("RUNNER_TEMP"), name)),
  );
  const binaries = qualifyBinaries(artifacts);
  const assets = inventory(dist());
  require(Object.keys(assets).length > 0 &&
    Object.hasOwn(assets, "index.html"), "UI_FRESH_DIST_MISSING");
  const abi = runtimeAbi(binaries);
  const bun = call(["which", "bun"]);
  const chromium = call([
    bun,
    ...bunIsolation(),
    "--eval",
    'import {chromium} from "@playwright/test";console.log(chromium.executablePath())',
  ]);
  require(isFile(chromium), "UI_PINNED_BROWSER_MISSING");
  const browserFiles = Object.fromEntries(files(dirname(chromium)).map((p) => [p, sha(p)]));
  const sqlite = join(env("RUNNER_TEMP"), "fvoci-sqlite/consumer-inputs.json");
  const manifest: Manifest = {
    schema: 1,
    sourceInputs: before as SourceInputs,
    physicalInputs: { path: physicalPath, sha256: sha(physicalPath) },
    binaries,
    assets,
    abi,
    bun: { path: bun, sha256: sha(bun), version: call([bun, "--version"]) },
    chromium,
    browserFiles,
    sqliteInputs: { path: sqlite, sha256: sha(sqlite) },
    rustc: call(["rustc", "-vV"]),
  };
  require(manifest.bun.version === PINNED_BUN, "UI_BUN_PIN_MISMATCH");
  write(join(root(), "current-build.json"), manifest);
}

export function currentBuild(lease?: LocalLease): Manifest {
  const manifest = privateRead(join(root(), "current-build.json"), 8 * 1024 * 1024) as Manifest;
  require(deepEquals(
    manifest.sourceInputs,
    sourceIdentity("current-build", lease),
    true,
  ), "UI_SOURCE_CHANGED");
  const physical = manifest.physicalInputs;
  require(sha(physical.path) === physical.sha256, "UI_PHYSICAL_RECEIPT_CHANGED");
  recheckPhysical(privateRead(physical.path, 64 * 1024 * 1024));
  for (const binary of Object.values(manifest.binaries))
    require(sha(binary.path) === binary.sha256, "UI_BINARY_CHANGED");
  for (const [path, digest] of Object.entries(manifest.abi))
    require(sha(path) === digest, "UI_ABI_CHANGED");
  for (const [path, digest] of Object.entries(manifest.browserFiles))
    require(sha(path) === digest, "UI_BROWSER_CHANGED");
  require(sha(manifest.bun.path) === manifest.bun.sha256, "UI_BUN_CHANGED");
  require(sha(manifest.sqliteInputs.path) ===
    manifest.sqliteInputs.sha256, "UI_SQLITE_INPUTS_CHANGED");
  require(deepEquals(inventory(dist()), manifest.assets, true), "UI_ASSETS_CHANGED");
  return manifest;
}
