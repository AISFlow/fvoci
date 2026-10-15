// Credential-free recording before the build and the freeze of its current
// outputs; currentBuild re-verifies every frozen identity before any use.
import { deepEquals } from "bun";
import {
  existsSync,
  lstatSync,
  openSync,
  readFileSync,
  readSync,
  closeSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join } from "node:path";
import process from "node:process";
import { buildEnv, inputs } from "../selected-backend-ci/build.ts";
import { pySplitlines } from "../web-e2e/compat.ts";
import {
  call,
  digest,
  env,
  files,
  inventory,
  resolved,
  root as checkout,
  sha,
  write,
} from "../selected-backend-ci/io.ts";
import type { LocalGrant } from "../selected-backend-ci/types.ts";
import { image as finalFixtureImage } from "../selected-backend-ci/drivers/sqlite.ts";
import { systemSudo } from "../keycloak/process-witness.ts";
import type { ProcessObserver } from "../keycloak/run.ts";
import {
  bunIsolation,
  executionMode,
  get,
  list,
  privateRead,
  require,
  root,
  UiError,
  valueDigest,
} from "./ui-common.ts";

export const PINNED_BUN = "1.4.3";
export const BINARIES = ["fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"];
export const FINAL_ENGINES = ["chromium", "firefox", "webkit"] as const;
export type BrowserEngine = (typeof FINAL_ENGINES)[number];

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
  finalBrowsers?: Record<BrowserEngine, { path: string; sha256: string }>;
  finalBrowserRuntime?: { path: string; files: Record<string, string> };
  finalRun?: ReturnType<typeof finalRunIdentity>;
  finalRuntime?: FinalRuntime;
  finalProcessObserver?: ProcessObserver;
}

function prepareFinalProcessObserver(source: SourceInputs): ProcessObserver {
  const run = finalRunIdentity(source),
    sudo = systemSudo();
  const parent = "/var/lib/fvoci-keycloak-observer";
  if (!existsSync(parent))
    call([sudo, "-n", "install", "-d", "-o", "root", "-g", "root", "-m", "0755", parent]);
  const parentStat = lstatSync(parent);
  require(parentStat.isDirectory() &&
    !parentStat.isSymbolicLink() &&
    parentStat.uid === 0 &&
    parentStat.gid === 0 &&
    (parentStat.mode & 0o022) === 0, "UI_FINAL_PROCESS_OBSERVER_PARENT_REFUSED");
  const directory = join(
    parent,
    source.source + "-" + String(run.runId) + "-" + String(run.attempt),
  );
  require(!existsSync(directory), "UI_FINAL_PROCESS_OBSERVER_ALREADY_EXISTS");
  call([sudo, "-n", "mkdir", "--mode=0755", directory]);
  const empty = join(root(), "observer-bunfig.empty.private");
  writeFileSync(empty, "", { flag: "wx", mode: 0o600 });
  for (const [from, name, mode] of [
    [process.execPath, "bun", "0555"],
    [join(checkout, "tools/keycloak/process-witness.ts"), "process-witness.ts", "0444"],
    [empty, "bunfig.toml", "0444"],
  ] as const)
    call([
      sudo,
      "-n",
      "install",
      "-o",
      "root",
      "-g",
      "root",
      "-m",
      mode,
      from,
      join(directory, name),
    ]);
  const reference = (name: string) => ({
    path: join(directory, name),
    sha256: sha(join(directory, name)),
  });
  const observer = {
    directory,
    bun: reference("bun"),
    module: reference("process-witness.ts"),
    config: reference("bunfig.toml"),
  };
  recheckFinalProcessObserver(observer, source, sha(process.execPath));
  return observer;
}

export function recheckFinalProcessObserver(
  observer: ProcessObserver,
  source: SourceInputs,
  bunSha256: string,
): void {
  const parent = "/var/lib/fvoci-keycloak-observer";
  const run = finalRunIdentity(source);
  require(observer.directory ===
    join(
      parent,
      source.source + "-" + String(run.runId) + "-" + String(run.attempt),
    ), "UI_FINAL_PROCESS_OBSERVER_CHANGED");
  for (const path of ["/", "/var", "/var/lib", parent]) {
    const ancestor = lstatSync(path);
    require(ancestor.isDirectory() &&
      !ancestor.isSymbolicLink() &&
      ancestor.uid === 0 &&
      ancestor.gid === 0 &&
      (ancestor.mode & 0o022) === 0, "UI_FINAL_PROCESS_OBSERVER_CHANGED");
  }
  const directory = lstatSync(observer.directory);
  require(directory.isDirectory() &&
    !directory.isSymbolicLink() &&
    directory.uid === 0 &&
    directory.gid === 0 &&
    (directory.mode & 0o777) === 0o755, "UI_FINAL_PROCESS_OBSERVER_CHANGED");
  for (const [name, reference, mode] of [
    ["bun", observer.bun, 0o555],
    ["process-witness.ts", observer.module, 0o444],
    ["bunfig.toml", observer.config, 0o444],
  ] as const) {
    const file = lstatSync(reference.path);
    require(reference.path === join(observer.directory, name) &&
      file.isFile() &&
      !file.isSymbolicLink() &&
      file.nlink === 1 &&
      file.uid === 0 &&
      file.gid === 0 &&
      (file.mode & 0o777) === mode &&
      sha(reference.path) === reference.sha256, "UI_FINAL_PROCESS_OBSERVER_CHANGED");
  }
  require(observer.bun.sha256 === bunSha256 &&
    observer.module.sha256 === source.files["tools/keycloak/process-witness.ts"] &&
    observer.config.sha256 === digest(""), "UI_FINAL_PROCESS_OBSERVER_SOURCE_CHANGED");
}

export interface FinalRuntime {
  reference: string;
  imageId: string;
  architecture: string;
  os: string;
  config: unknown;
  uid: number;
  gid: number;
  abi: { binaries: Record<string, string>; outputSha256: string };
}

/** An image observation, not a grant or a product publication receipt. */
export function finalRuntimeImage(value: unknown): Omit<FinalRuntime, "abi"> {
  const item = get(value, 0);
  const imageId = get(item, "Id"),
    architecture = get(item, "Architecture"),
    os = get(item, "Os");
  const user = process.getuid?.(),
    group = process.getgid?.();
  require(typeof imageId === "string" &&
    /^sha256:[a-f0-9]{64}$/.test(imageId) &&
    os === "linux" &&
    typeof architecture === "string" &&
    architecture === (process.arch === "x64" ? "amd64" : process.arch) &&
    list(item, "RepoDigests").includes(
      "ubuntu@" + finalFixtureImage.slice(finalFixtureImage.indexOf("@") + 1),
    ) &&
    typeof user === "number" &&
    user > 0 &&
    typeof group === "number" &&
    group > 0, "UI_FINAL_RUNTIME_IMAGE_REFUSED");
  const config = get(item, "Config");
  for (const entry of list(config, "Env"))
    require(typeof entry === "string" &&
      !/PASSWORD|TOKEN|SECRET|CREDENTIAL|FVOCI_LIBSQL_|ENCRYPTION_/.test(
        entry.split("=", 1)[0] ?? "",
      ), "UI_FINAL_RUNTIME_IMAGE_ENV_REFUSED");
  return {
    reference: finalFixtureImage,
    imageId,
    architecture,
    os,
    config,
    uid: user,
    gid: group,
  };
}

const observeFinalImage = () =>
  finalRuntimeImage(JSON.parse(call(["docker", "image", "inspect", finalFixtureImage])));

function freezeFinalRuntime(binaries: Record<string, Binary>): FinalRuntime {
  const before = privateRead(join(root(), "final-image-before.private.json"));
  const current = observeFinalImage();
  require(deepEquals(before, current, true), "UI_FINAL_RUNTIME_IMAGE_CHANGED");
  const directory = dirname((binaries["fvoci-server"] as Binary).path);
  require(Object.values(binaries).every(
    (binary) => dirname(binary.path) === directory,
  ), "UI_FINAL_NATIVE_COHORT_NOT_ADJACENT");
  const output = call([
    "docker",
    "run",
    "--rm",
    "--pull",
    "never",
    "--network",
    "none",
    "--read-only",
    "--cap-drop",
    "ALL",
    "--security-opt",
    "no-new-privileges",
    "--user",
    String(current.uid) + ":" + String(current.gid),
    "--mount",
    "type=bind,src=" + directory + ",dst=/fvoci-current/bin,readonly",
    "--entrypoint",
    "/bin/sh",
    current.imageId,
    "-ec",
    '. /etc/os-release; test "$ID" = ubuntu; test "$VERSION_ID" = 26.04; for binary do ldd "$binary"; done',
    "sh",
    ...BINARIES.map((name) => "/fvoci-current/bin/" + name),
  ]);
  require(!output.includes("not found"), "UI_FINAL_RUNTIME_ABI_MISSING");
  return {
    ...current,
    abi: {
      binaries: Object.fromEntries(
        Object.entries(binaries).map(([name, binary]) => [name, binary.sha256]),
      ),
      outputSha256: valueDigest(output),
    },
  };
}

/** Final-only same-job binding; previous runs and attempts cannot be replayed. */
export function finalRunIdentity(source: SourceInputs, environment = process.env) {
  require(environment.GITHUB_ACTIONS === "true" &&
    environment.CI === "true" &&
    environment.GITHUB_JOB === "turso-ui" &&
    environment.GITHUB_REPOSITORY === "AISFlow/fvoci" &&
    environment.GITHUB_REF === "refs/heads/main" &&
    environment.GITHUB_EVENT_NAME === "workflow_dispatch" &&
    source.source === environment.GITHUB_SHA &&
    /^[a-f0-9]{40}$/.test(source.source) &&
    /^[a-f0-9]{40}$/.test(source.tree) &&
    /^[1-9][0-9]*$/.test(environment.GITHUB_RUN_ID ?? "") &&
    /^[1-9][0-9]*$/.test(environment.GITHUB_RUN_ATTEMPT ?? ""), "UI_FINAL_JOB_BINDING_REQUIRED");
  return {
    source: source.source,
    tree: source.tree,
    runId: environment.GITHUB_RUN_ID,
    attempt: environment.GITHUB_RUN_ATTEMPT,
    job: environment.GITHUB_JOB,
  };
}

/** Every installed runtime input, including shared assets and launch scripts. */
export function sealFinalBrowsers(
  browserRoot: string,
  executables: Record<BrowserEngine, string>,
): Pick<Manifest, "finalBrowsers" | "finalBrowserRuntime"> {
  require(isAbsolute(browserRoot), "UI_FINAL_BROWSER_ROOT_REQUIRED");
  const runtimeFiles = inventory(browserRoot);
  const finalBrowsers = Object.fromEntries(
    FINAL_ENGINES.map((engine) => {
      const path = executables[engine];
      require(isAbsolute(path) &&
        path.startsWith(browserRoot + "/") &&
        isFile(path) &&
        !lstatSync(path).isSymbolicLink() &&
        Object.hasOwn(
          runtimeFiles,
          path.slice(browserRoot.length + 1),
        ), "UI_FINAL_BROWSER_EXECUTABLE_REQUIRED");
      return [engine, { path, sha256: sha(path) }];
    }),
  ) as NonNullable<Manifest["finalBrowsers"]>;
  require(new Set(Object.values(finalBrowsers).map((browser) => browser.path)).size ===
    3, "UI_FINAL_BROWSER_ENGINE_ALIAS_REFUSED");
  return { finalBrowsers, finalBrowserRuntime: { path: browserRoot, files: runtimeFiles } };
}

export function recheckFinalBrowsers(
  manifest: Pick<Manifest, "finalBrowsers" | "finalBrowserRuntime">,
): void {
  require(manifest.finalBrowsers !== undefined &&
    manifest.finalBrowserRuntime !== undefined, "UI_FINAL_BROWSER_SEAL_REQUIRED");
  const runtime = manifest.finalBrowserRuntime;
  require(deepEquals(Object.keys(manifest.finalBrowsers).sort(), [...FINAL_ENGINES].sort()) &&
    deepEquals(inventory(runtime.path), runtime.files, true), "UI_FINAL_BROWSER_RUNTIME_CHANGED");
  for (const engine of FINAL_ENGINES) {
    const browser = manifest.finalBrowsers[engine];
    require(browser.path.startsWith(runtime.path + "/") &&
      sha(browser.path) === browser.sha256 &&
      runtime.files[browser.path.slice(runtime.path.length + 1)] ===
        browser.sha256, "UI_FINAL_BROWSER_EXECUTABLE_CHANGED");
  }
}

/**
 * Same-user ROOT lease of the orca-local mode. The maintained loader is
 * `localTursoUiAllocation` in tools/selected-backend-ci/admission.ts. Without
 * that controller allocation the local mode is refused.
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
  const source = sourceIdentity("record-before", lease);
  if (process.env.FVOCI_TURSO_UI_FINAL === "1") {
    finalRunIdentity(source);
    call(["docker", "pull", finalFixtureImage]);
    write(join(root(), "final-image-before.private.json"), observeFinalImage());
    write(join(root(), "final-observer-before.private.json"), prepareFinalProcessObserver(source));
  }
  write(join(root(), "source-before.json"), source);
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
  if (process.env.FVOCI_TURSO_UI_FINAL === "1") {
    manifest.finalRun = finalRunIdentity(manifest.sourceInputs);
    manifest.finalRuntime = freezeFinalRuntime(binaries);
    manifest.finalProcessObserver = privateRead(
      join(root(), "final-observer-before.private.json"),
    ) as ProcessObserver;
    recheckFinalProcessObserver(
      manifest.finalProcessObserver,
      manifest.sourceInputs,
      manifest.bun.sha256,
    );
    const selected = JSON.parse(
      call([
        bun,
        ...bunIsolation(),
        "--eval",
        'import {chromium,firefox,webkit} from "@playwright/test";console.log(JSON.stringify({chromium:chromium.executablePath(),firefox:firefox.executablePath(),webkit:webkit.executablePath()}))',
      ]),
    ) as Record<BrowserEngine, string>;
    Object.assign(manifest, sealFinalBrowsers(env("PLAYWRIGHT_BROWSERS_PATH"), selected));
  }
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
  if (process.env.FVOCI_TURSO_UI_FINAL === "1" || manifest.finalBrowsers !== undefined) {
    recheckFinalBrowsers(manifest);
    require(deepEquals(
      manifest.finalRun,
      finalRunIdentity(manifest.sourceInputs),
      true,
    ), "UI_FINAL_JOB_BINDING_CHANGED");
    require(manifest.finalRuntime !== undefined, "UI_FINAL_RUNTIME_IMAGE_REQUIRED");
    const { abi, ...image } = manifest.finalRuntime;
    require(deepEquals(image, observeFinalImage(), true) &&
      deepEquals(
        abi.binaries,
        Object.fromEntries(
          Object.entries(manifest.binaries).map(([name, binary]) => [name, binary.sha256]),
        ),
        true,
      ), "UI_FINAL_RUNTIME_IMAGE_CHANGED");
    require(manifest.finalProcessObserver !== undefined, "UI_FINAL_PROCESS_OBSERVER_REQUIRED");
    recheckFinalProcessObserver(
      manifest.finalProcessObserver,
      manifest.sourceInputs,
      manifest.bun.sha256,
    );
  }
  return manifest;
}
