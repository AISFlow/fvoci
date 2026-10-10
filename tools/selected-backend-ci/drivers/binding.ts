// The only artifact/allocation binding for the selected lane drivers. No
// build, lifecycle, DB, native or browser operation happens here: the runner
// supplies the compiler/web/ABI receipts after an exclusive allocation and
// every input is re-hashed before a driver touches a resource.
import { deepEquals } from "bun";
import { strict as assert } from "node:assert";
import { lstatSync, statfsSync } from "node:fs";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import process from "node:process";
import { localAllocation } from "../admission.ts";
import { digest, env, jsonInteger, read, root, sha, sourceInputText } from "../io.ts";
import type {
  Abi,
  Artifact,
  Browser,
  Bundle,
  Inputs,
  LocalGrant,
  Reference,
  Web,
} from "../types.ts";
import { command, hex40, inputCheck, lines, readText, treeHashes } from "./common.ts";
import { restartHelper, validateAllocation, type RestartBinding } from "./restart.ts";

export const bindingModule = import.meta.path;
// The feature list cargo emits for the engine stage: crates/collab-engine
// declares `default = []`, and a compiler-artifact lists every activated
// feature including a declared default. A missing default or an extra feature
// (test-hang) is a different, non-current engine.
export const engineFeatures = ["default", "worker"];
export const engineFeaturesMatch = (features: unknown) =>
  Array.isArray(features) && deepEquals([...(features as string[])].sort(), engineFeatures);
// Receipt booleans are JSON values; only a literal true qualifies.
const isTrue = (value: unknown) => value === true;
const wikiSpec = "apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts";
const offSpec = "apps/web/e2e-pending/workspace-off-selected-backend.spec.ts";
const offSpecSha256 = "3345890f670f6462a6b14931a335de49b62b3c8355bdeece699d030b2869cff7";

export type Lane = "install" | "postgres" | "sqlite";
export interface Manifest {
  schema: unknown;
  ready: unknown;
  flow?: unknown;
  source: string;
  tree: string;
  compiledSource: string;
  sourceInputsBefore: Reference;
  sourceInputsAfter: Reference;
  bundle: Reference;
  compileReceipt: Reference;
  webReceipt: Reference;
  abiReceipt: Reference;
  nativeQualification: Reference | null;
  browserInputs: Browser;
  closedInstallReceipt: Reference | null;
  restartAllocation?: Reference;
}
export interface Grant {
  status: unknown;
  owner: unknown;
  executionMode?: string;
  localAuthorizationSha256?: unknown;
  exclusiveCIJob?: unknown;
  currentCIJobConfirmed?: unknown;
  runId: string;
  runAttempt: string;
  flow?: unknown;
  source: unknown;
  tree: unknown;
  compiledSource: unknown;
  lane: unknown;
  backend: unknown;
  runRoot: string;
  driverSha256: unknown;
  bindingSha256: unknown;
  bindingModuleSha256: unknown;
}
interface CompileReceipt {
  source: unknown;
  tree: unknown;
  full_inputs_unchanged: unknown;
  exit_code: unknown;
  stages: { exit_code: unknown; compilerMessages: Reference }[];
}
interface NativeQualification {
  currentSource: unknown;
  currentTree: unknown;
  originalSource: unknown;
  executableSha256: unknown;
  originalFeature: unknown;
  originalTarget: unknown;
  originalNativeInputs: Record<string, string>;
  currentNativeInputs: Record<string, string>;
  originalToolchainInputs: Record<string, string>;
  currentToolchainInputs: Record<string, string>;
  originalAbiInputs: Record<string, string>;
  currentAbiInputs: Record<string, string>;
  [key: string]: unknown;
}
interface ClosedInstall {
  source: unknown;
  tree: unknown;
  final_exit_code: unknown;
  actual_tests: unknown;
  actual_owned_process_receipts: unknown;
  owned_container_absent: unknown;
  actual_binary_inputs: unknown;
}
export interface Current {
  manifest: Manifest;
  manifestPath: string;
  grant: Grant;
  run: string;
  before: Inputs;
  // The input_check result recorded before any resource is touched.
  sourceBefore: Inputs;
  build: Bundle;
  compileReceipt: CompileReceipt;
  assets: Web;
  abi: Abi;
  flow: "on" | "off";
}

export function referenced(record: Reference): unknown {
  assert.ok(isAbsolute(record.path) && !lstatSync(record.path).isSymbolicLink());
  assert.ok(sha(record.path) === record.sha256);
  return read(record.path);
}

interface OffCase {
  file: string;
  ok: unknown;
  title: string;
  tests: {
    expectedStatus: unknown;
    results: { status: unknown; retry: unknown; errors: unknown }[];
  }[];
}
interface OffSuite {
  specs?: OffCase[];
  suites?: OffSuite[];
}
// All eight registered OFF cases pass once, in declared order.
export function validateOffReport(report: unknown, backend: string): string[] {
  const spec = join(root, offSpec);
  assert.ok(sha(spec) === offSpecSha256);
  const expected = [...readText(spec).matchAll(/ {2}test\("([^"\n]+)"/g)].map((m) => m[1]);
  assert.ok(expected.length === 8 && new Set(expected).size === 8);
  const r = report as {
    config: { workers: number; metadata: { selectedBackend: unknown; selectedFlow: unknown } };
    errors: unknown;
    stats: Record<string, unknown>;
    suites: OffSuite[];
  };
  assert.ok(jsonInteger(r.config, "workers") && r.config.workers === 1 && deepEquals(r.errors, []));
  assert.ok(r.config.metadata.selectedBackend === backend);
  assert.ok(r.config.metadata.selectedFlow === "off");
  assert.ok(r.stats.expected === 8);
  assert.ok(["unexpected", "flaky", "skipped"].every((key) => r.stats[key] === 0));
  const cases: string[] = [];
  const visit = (suites: OffSuite[]) => {
    for (const suite of suites) {
      for (const item of suite.specs ?? []) {
        assert.ok(basename(item.file) === basename(spec) && item.ok === true);
        assert.ok(item.tests.length === 1);
        const test = item.tests[0] as OffCase["tests"][number];
        assert.ok(test.expectedStatus === "passed" && test.results.length === 1);
        const actual = test.results[0] as OffCase["tests"][number]["results"][number];
        assert.ok(
          actual.status === "passed" && actual.retry === 0 && deepEquals(actual.errors, []),
        );
        cases.push(item.title);
      }
      visit(suite.suites ?? []);
    }
  };
  visit(r.suites);
  assert.ok(
    deepEquals(cases, expected),
    "all eight registered cases must pass once in declared order",
  );
  return cases;
}

const git = async (...args: string[]) =>
  (await command(["git", "-c", "safe.directory=" + root, "-C", root, ...args])).stdout;

// Rejects an absent or false allocation before hashing large inputs or any
// resource mutation. `driver` is the lane driver file the grant names.
export async function loadCurrent(lane: Lane, driver: string): Promise<Current> {
  const owner = env("FVOCI_CI_OWNER"),
    runs = env("FVOCI_CI_SELECTED_RUNS");
  const manifestPath = process.env.FVOCI_ROOT_CURRENT_BINDING;
  assert.ok(
    manifestPath,
    "NOT GRANTED: root must supply exact current compile and allocation binding",
  );
  assert.ok(isAbsolute(manifestPath) && !lstatSync(manifestPath).isSymbolicLink());
  const m = read(manifestPath) as Manifest;
  assert.ok(m.schema === 1 && m.ready === true, "NOT GRANTED: preparation template is false");
  const execution = process.env.FVOCI_SELECTED_EXECUTION_MODE ?? "github-ci";
  assert.ok(execution === "github-ci" || execution === "orca-local");
  const local: LocalGrant | null = execution === "orca-local" ? localAllocation("run") : null;
  if (local !== null) assert.ok(m.source === local.source && m.tree === local.tree);
  else {
    assert.ok(env("GITHUB_ACTIONS") === "true" && env("CI") === "true");
    assert.ok(m.source === env("GITHUB_SHA"));
  }
  assert.ok(process.getuid?.() === 1000 && process.getgid?.() === 1000);
  assert.ok(process.env.FVOCI_ROOT_RUN_OWNER === owner);
  const grantPath = env("FVOCI_ROOT_CURRENT_ALLOCATION");
  assert.ok(isAbsolute(grantPath) && !lstatSync(grantPath).isSymbolicLink());
  const grant = read(grantPath) as Grant;
  assert.ok(grant.status === "GRANTED" && grant.owner === owner);
  assert.ok((grant.executionMode ?? "github-ci") === execution);
  if (local !== null) {
    assert.ok(
      grant.executionMode === "orca-local" &&
        grant.localAuthorizationSha256 === env("FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256"),
    );
    assert.ok(grant.runId === local.runId && grant.runAttempt === local.dispatchId);
  } else {
    assert.ok(grant.exclusiveCIJob === true && grant.currentCIJobConfirmed === true);
    assert.ok(grant.runId === env("GITHUB_RUN_ID"));
    assert.ok(grant.runAttempt === env("GITHUB_RUN_ATTEMPT"));
    assert.ok(/^[0-9]+$/.test(grant.runId) && /^[0-9]+$/.test(grant.runAttempt));
  }
  assert.ok(
    m.source === m.compiledSource && m.source === grant.source && m.source === grant.compiledSource,
  );
  assert.ok(m.tree === grant.tree && grant.lane === lane);
  assert.ok(hex40.test(m.source) && hex40.test(m.tree));
  assert.ok(
    m.source !== "d49f1842f612561c318b70bcc22f8227b5ed85a1",
    "historical main cohort cannot qualify changed current Rust",
  );
  assert.ok((await git("rev-parse", "HEAD")).trim() === m.source);
  assert.ok((await git("rev-parse", "HEAD^{tree}")).trim() === m.tree);
  assert.ok(grant.driverSha256 === sha(driver) && grant.bindingSha256 === sha(manifestPath));
  assert.ok(grant.bindingModuleSha256 === sha(bindingModule));
  const run = grant.runRoot;
  assert.ok(
    typeof run === "string" &&
      isAbsolute(run) &&
      resolve(run) === run &&
      dirname(run) === resolve(runs) &&
      /^root-current-(install|postgres|sqlite)-[0-9a-f]{12}$/.test(basename(run)),
  );
  assert.ok(basename(run).startsWith("root-current-" + lane + "-"));
  const disk = statfsSync(runs);
  assert.ok(
    disk.bavail * disk.bsize >= 20_000_000_000,
    "root heavy start floor is20GB; source preparation grants no cleanup",
  );
  const before = referenced(m.sourceInputsBefore) as Inputs;
  const after = referenced(m.sourceInputsAfter);
  assert.ok(deepEquals(before, after) && before.head === m.source && before.tree === m.tree);
  assert.ok(Object.keys(before.tracked).length && Object.keys(before.external).length);
  assert.ok(
    before.tracked[wikiSpec] === "550a196a4563ff74e68dc4339803f4c06c1be1a39698c08a183ce418b155299f",
  );
  const flow: unknown = m.flow ?? "on";
  assert.ok(flow === "on" || flow === "off");
  assert.ok((grant.flow ?? "on") === flow);
  assert.ok((process.env.FVOCI_E2E_SELECTED_FLOW ?? "on") === flow);
  if (lane === "install") assert.ok(flow === "on");
  if (flow === "off") assert.ok(before.tracked[offSpec] === offSpecSha256);
  if (lane === "postgres")
    assert.ok(
      before.tracked["apps/web/e2e-pending/workspace-wiki-selected-auxiliary.ts"] ===
        "c38b23f590e08f68f4a7abf64d71976e9b088f66631982e73e8f26aa8d606f57",
    );
  // The full current closure (status, ls-files set and every tracked, external
  // and untracked hash) is the same check every driver mode repeats later.
  const sourceBefore = await inputCheck(before, m.source, m.tree);
  const build = referenced(m.bundle) as Bundle;
  const compileReceipt = referenced(m.compileReceipt) as CompileReceipt;
  assert.ok(build.source === compileReceipt.source && build.source === m.source);
  assert.ok(build.tree === compileReceipt.tree && build.tree === m.tree);
  assert.ok(
    isTrue(build.full_inputs_unchanged) &&
      compileReceipt.full_inputs_unchanged === true &&
      compileReceipt.exit_code === 0,
  );
  const artifacts: Artifact[] = [];
  for (const stage of compileReceipt.stages) {
    assert.ok(stage.exit_code === 0);
    const raw = stage.compilerMessages.path;
    assert.ok(sha(raw) === stage.compilerMessages.sha256);
    for (const line of lines(readText(raw)))
      if (line.startsWith("{")) {
        const message = JSON.parse(line) as Artifact;
        if (message.reason === "compiler-artifact") artifacts.push(message);
      }
  }
  const binaries = build.binaries;
  const sorted = (values: string[]) => [...values].sort();
  for (const name of [
    "fvoci-server",
    "fvoci-migrate",
    "fvoci-e2e-fixture",
    "selected_install_lifetime",
    "fvoci_server",
  ]) {
    const matches = Object.entries(binaries).filter(([, r]) => r.target.name === name);
    assert.ok(matches.length === 1, "missing coherent current cohort target");
    const [path, r] = matches[0] as [string, Bundle["binaries"][string]];
    assert.ok(r.compiledSource === m.source && r.targetTriple === "x86_64-unknown-linux-gnu");
    assert.ok(deepEquals(sorted(r.features), ["api-schema", "db-tests"]));
    assert.ok(r.profile.opt_level === "0" && r.profile.debug_assertions === true);
    assert.ok(r.profile.test === ["selected_install_lifetime", "fvoci_server"].includes(name));
    assert.ok(sha(path) === r.sha256);
    assert.ok(
      artifacts.some(
        (a) =>
          deepEquals(a.target, r.target, true) &&
          deepEquals(a.profile, r.profile, true) &&
          deepEquals(sorted(a.features), sorted(r.features)) &&
          a.filenames.includes(path),
      ),
      "not emitted by current recorded Cargo command",
    );
  }
  const engine = Object.entries(binaries).find(([, r]) => r.target.name === "collab-engine")?.[0];
  assert.ok(engine !== undefined);
  const engineRecord = binaries[engine] as Bundle["binaries"][string];
  assert.ok(sha(engine) === engineRecord.sha256);
  assert.ok(typeof engineRecord.compiledSource === "string", "engine compile source required");
  if (engineRecord.compiledSource !== m.source) {
    assert.ok(m.nativeQualification !== null);
    const q = referenced(m.nativeQualification) as NativeQualification;
    assert.ok(q.currentSource === m.source && q.currentTree === m.tree);
    assert.ok(q.originalSource === engineRecord.compiledSource);
    assert.ok(q.executableSha256 === engineRecord.sha256);
    assert.ok(
      [
        "fullNativeInputsEqual",
        "featuresEqual",
        "toolchainEqual",
        "abiEqual",
        "actualExecutableHashChecked",
      ].every((key) => q[key] === true),
    );
    assert.ok(q.originalFeature === "worker" && q.originalTarget === "x86_64-unknown-linux-gnu");
    assert.ok(
      deepEquals(q.originalNativeInputs, q.currentNativeInputs) &&
        Object.keys(q.currentNativeInputs).length > 0,
    );
    assert.ok(Object.entries(q.currentNativeInputs).every(([n, h]) => before.tracked[n] === h));
    assert.ok(
      deepEquals(q.originalToolchainInputs, q.currentToolchainInputs) &&
        Object.keys(q.currentToolchainInputs).length > 0,
    );
    assert.ok(Object.entries(q.currentToolchainInputs).every(([n, h]) => before.external[n] === h));
    assert.ok(
      deepEquals(q.originalAbiInputs, q.currentAbiInputs) &&
        Object.keys(q.currentAbiInputs).length > 0,
    );
    assert.ok(Object.entries(q.currentAbiInputs).every(([n, h]) => sha(n) === h));
  } else {
    assert.ok(engineFeaturesMatch(engineRecord.features), "engine features");
    assert.ok(artifacts.some((a) => a.executable === engine && engineFeaturesMatch(a.features)));
  }
  const assets = referenced(m.webReceipt) as Web;
  assert.ok(assets.source === m.source && assets.tree === m.tree);
  assert.ok(
    assets.exit_code === 0 &&
      isTrue(assets.full_inputs_unchanged) &&
      Object.keys(assets.dist_files).length > 0,
  );
  assert.ok(deepEquals(treeHashes(join(root, "apps/web/dist")), assets.dist_files));
  const abi = referenced(m.abiReceipt) as Abi;
  const server = Object.keys(binaries).find((path) => path.endsWith("/fvoci-server"));
  assert.ok(
    server !== undefined &&
      abi.currentSource === m.source &&
      abi.currentServerSha256 === binaries[server]?.sha256,
  );
  assert.ok(isTrue(abi.currentELFDependenciesVerified));
  for (const [path, hash] of Object.entries(abi.host_runtime_files)) assert.ok(sha(path) === hash);
  if (lane !== "install") {
    assert.ok(m.closedInstallReceipt !== null);
    const closed = referenced(m.closedInstallReceipt) as ClosedInstall;
    assert.ok(closed.source === m.source && closed.tree === m.tree && closed.final_exit_code === 0);
    assert.ok(
      closed.actual_tests === 4 &&
        closed.actual_owned_process_receipts === 15 &&
        closed.owned_container_absent === true,
    );
    assert.ok(deepEquals(closed.actual_binary_inputs, binaries, true));
    assert.ok(grant.backend === lane);
    let restart: { binding: RestartBinding } | null = null;
    const artifactHashes = Object.fromEntries(
      Object.entries(binaries).map(([path, record]) => [path, record.sha256]),
    );
    if (flow === "on") {
      assert.ok(m.restartAllocation !== undefined);
      restart = referenced(m.restartAllocation) as { binding: RestartBinding };
      const sourceWritten = Object.fromEntries(
        (["head", "tree", "status", "tracked", "external", "untracked"] as const).map((key) => [
          key,
          before[key],
        ]),
      );
      validateAllocation(restart, {
        runId: grant.runId,
        runAttempt: grant.runAttempt,
        source: m.source,
        tree: m.tree,
        compiledSource: m.source,
        backend: lane,
        runRoot: run,
        parentDriverSha256: sha(driver),
        restartHelperSha256: sha(restartHelper),
        sourceInputsSha256: digest(sourceInputText(sourceWritten)),
        artifactHashes,
        assetHashes: assets.dist_files,
        browserInputs: m.browserInputs,
        abiHashes: abi.host_runtime_files,
      });
    }
    const browser = m.browserInputs;
    for (const key of ["bun", "chromium"] as const)
      assert.ok(sha(browser[key].path) === browser[key].sha256);
    assert.ok(
      deepEquals(treeHashes(dirname(browser.chromium.path)), browser.chromium_directory_files),
    );
    if (restart !== null) {
      assert.ok(restart.binding.parentDriverSha256 === sha(driver));
      assert.ok(restart.binding.restartHelperSha256 === sha(restartHelper));
      assert.ok(deepEquals(restart.binding.artifactHashes, artifactHashes));
      assert.ok(deepEquals(restart.binding.assetHashes, assets.dist_files));
      assert.ok(deepEquals(restart.binding.abiHashes, abi.host_runtime_files));
      assert.ok(env("FVOCI_ROOT_RESTART_GRANT") === m.restartAllocation?.path);
    }
  }
  return {
    manifest: m,
    manifestPath,
    grant,
    run,
    before,
    sourceBefore,
    build,
    compileReceipt,
    assets,
    abi,
    flow,
  };
}
