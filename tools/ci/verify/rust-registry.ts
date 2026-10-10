// Every root db-tests integration target (explicit [[test]] or autodiscovered
// tests/*.rs) maps to exactly one rust.yml execution bucket.
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import {
  get,
  has,
  intersect,
  isMapping,
  pyStrip,
  pySplitlines,
  setEq,
  sortedPy,
  union,
  type Mapping,
} from "./py.ts";
import {
  collaborationScriptInventory,
  verifyCollaborationWorkflowExecution,
} from "./rust-collab.ts";
import { postgresS3Inventory, verifyPostgresIntegrationExecution } from "./rust-exec.ts";
import { selectedInstallInventory } from "./rust-install.ts";
import { verifyNativeArm64Execution } from "./rust-native.ts";
import { schemaBaselineInventory } from "./rust-schema.ts";
import {
  RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS,
  RUST_CAPACITY_PROBE_SCRIPT,
  RUST_DB_TESTS_FEATURE,
  RUST_INTEGRATION_MANUAL_TARGETS,
  RUST_WORKFLOW_FILE,
  isFile,
  notUtf8,
  readUtf8,
  postgresMatrixInventory,
  rustWorkflowJobs,
  rustWorkflowPresent,
  type Result,
  type VerifyContext,
} from "./rust-shared.ts";

// Checks owned by the planner-side rust unit that run at fixed points of this
// registry walk; absent hooks contribute no errors.
export type RustRegistryHooks = {
  verifySelectedLibraryExecution?: (jobs: Mapping) => string[];
  verifyPostgresBudgetMatrix?: (jobs: Mapping) => string[];
};

export type AutotestFile = { stem: string; text: string | null; invalidUtf8?: boolean };

function pyTruthy(value: unknown): boolean {
  if (Array.isArray(value)) return value.length > 0;
  if (isMapping(value)) return Object.keys(value).length > 0;
  return Boolean(value);
}

function crateAttributes(text: string): string[] {
  return pySplitlines(text)
    .map(pyStrip)
    .filter((line) => line.startsWith("#!["));
}

function declaresDbTests(attrs: string[]): boolean {
  for (const attr of attrs) {
    if (attr.includes("extract-native-tests")) return false;
    if (attr.includes('feature = "db-tests"') || attr.includes('feature="db-tests"')) return true;
  }
  return false;
}

// Pure classification over the parsed Cargo.toml and the root tests/*.rs
// files in sorted order (`text: null` is an entry that is not a readable file).
export function classifyDbIntegrationTargets(
  cargo: Mapping,
  autotests: AutotestFile[] | null,
): Result<Set<string>> {
  const targets = new Set<string>();
  const entries = get(cargo, "test");
  if (Array.isArray(entries)) {
    for (const entry of entries) {
      if (!isMapping(entry)) return [null, "rust: Cargo.toml [[test]] entry must be a table"];
      const name = get(entry, "name");
      if (typeof name !== "string" || !name)
        return [null, "rust: Cargo.toml [[test]] missing name"];
      const features = get(entry, "required-features", []) ?? [];
      if (!Array.isArray(features) || !features.every((item) => typeof item === "string")) {
        return [null, `rust: Cargo.toml [[test]] ${name} required-features must be a string list`];
      }
      if (features.includes(RUST_DB_TESTS_FEATURE)) targets.add(name);
    }
  }
  const pkg = get(cargo, "package");
  const autotestsEnabled =
    isMapping(pkg) && has(pkg, "autotests") ? pyTruthy(get(pkg, "autotests")) : true;
  if (autotestsEnabled && autotests !== null) {
    for (const { stem, text, invalidUtf8 } of autotests) {
      const unregistered: Result<Set<string>> = [
        null,
        `rust: tests/${stem}.rs is not registered and has no crate ` +
          '#![cfg(feature = "db-tests")]; add CI inventory or an explicit fast/native exclusion',
      ];
      if (invalidUtf8 === true) return [null, notUtf8(`tests/${stem}.rs`)];
      if (text === null) return unregistered;
      const attrs = crateAttributes(text);
      if (declaresDbTests(attrs)) {
        targets.add(stem);
        continue;
      }
      if (attrs.some((attr) => attr.includes("extract-native-tests"))) continue;
      if (RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS.has(stem)) continue;
      if (attrs.length > 0 || pyStrip(text)) return unregistered;
    }
  }
  return [targets, null];
}

function isDirectory(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

function readAutotests(root: string): AutotestFile[] | null {
  const dir = join(root, "tests");
  if (!isDirectory(dir)) return null;
  const names = sortedPy(readdirSync(dir).filter((name) => name.endsWith(".rs")));
  return names.map((name) => {
    const path = join(dir, name);
    const stem = name === ".rs" ? name : name.slice(0, -3);
    if (!isFile(path)) return { stem, text: null };
    const text = readUtf8(path);
    return text === null ? { stem, text, invalidUtf8: true } : { stem, text };
  });
}

export function rootDbIntegrationRegistryTargets(root: string): Result<Set<string>> {
  const cargoPath = join(root, "Cargo.toml");
  if (!isFile(cargoPath)) return [null, "rust: missing root Cargo.toml"];
  let cargo: unknown;
  try {
    const text = readUtf8(cargoPath);
    if (text === null) return [null, notUtf8("Cargo.toml")];
    cargo = Bun.TOML.parse(text);
  } catch (error) {
    return [
      null,
      `rust: Cargo.toml parse failed: ${error instanceof Error ? error.message : String(error)}`,
    ];
  }
  if (!isMapping(cargo)) return [null, "rust: Cargo.toml parse failed: not a table"];
  return classifyDbIntegrationTargets(cargo, readAutotests(root));
}

export function verifyRustSuiteRegistry(
  ctx: VerifyContext,
  hooks: RustRegistryHooks = {},
): string[] {
  const errors: string[] = [];
  if (!rustWorkflowPresent(ctx)) return [`rust: missing workflow file ${RUST_WORKFLOW_FILE}`];
  if (!isFile(join(ctx.root, "Cargo.toml"))) return ["rust: missing root Cargo.toml"];
  const [cargoTargets, cargoErr] = rootDbIntegrationRegistryTargets(ctx.root);
  if (cargoErr !== null) return [cargoErr];
  const [jobs, jobsErr] = rustWorkflowJobs(ctx);
  if (jobsErr !== null) return [jobsErr];

  errors.push(...(hooks.verifySelectedLibraryExecution?.(jobs) ?? []));
  errors.push(...verifyNativeArm64Execution(jobs));
  errors.push(...(hooks.verifyPostgresBudgetMatrix?.(jobs) ?? []));
  errors.push(...verifyPostgresIntegrationExecution(jobs));

  const [perArch, matrixErr] = postgresMatrixInventory(jobs);
  if (matrixErr !== null) return [...errors, matrixErr];
  const [s3Tests, s3Err] = postgresS3Inventory(jobs);
  if (s3Err !== null) return [...errors, s3Err];
  errors.push(...verifyCollaborationWorkflowExecution(jobs));
  const [collabTests, collabErr] = collaborationScriptInventory(ctx.root);
  if (collabErr !== null) return [...errors, collabErr];

  if (!setEq(perArch.x64, perArch.arm64)) {
    const onlyX64 = sortedPy([...perArch.x64].filter((name) => !perArch.arm64.has(name)));
    const onlyArm = sortedPy([...perArch.arm64].filter((name) => !perArch.x64.has(name)));
    if (onlyX64.length > 0)
      errors.push("rust: postgres matrix missing on arm64: " + onlyX64.join(", "));
    if (onlyArm.length > 0)
      errors.push("rust: postgres matrix missing on x64: " + onlyArm.join(", "));
  }

  const [installTests, installErr] = selectedInstallInventory(jobs);
  if (installErr !== null) return [...errors, installErr];
  const [schemaTests, schemaErr] = schemaBaselineInventory(jobs);
  if (schemaErr !== null) return [...errors, schemaErr];

  const postgres = perArch.x64;
  const overlap = union(
    intersect(postgres, collabTests),
    intersect(postgres, s3Tests),
    intersect(collabTests, s3Tests),
    intersect(installTests, union(postgres, collabTests, s3Tests)),
    intersect(schemaTests, union(postgres, collabTests, s3Tests, installTests)),
  );
  if (overlap.size > 0) {
    errors.push(
      "rust: integration target assigned to multiple CI buckets: " + sortedPy(overlap).join(", "),
    );
  }

  if (intersect(RUST_INTEGRATION_MANUAL_TARGETS, cargoTargets).size > 0) {
    if (!isFile(join(ctx.root, RUST_CAPACITY_PROBE_SCRIPT))) {
      errors.push(`rust: missing manual probe script ${RUST_CAPACITY_PROBE_SCRIPT}`);
    }
  }

  const assigned = union(
    postgres,
    collabTests,
    s3Tests,
    installTests,
    schemaTests,
    RUST_INTEGRATION_MANUAL_TARGETS,
  );
  const missing = sortedPy(
    [...cargoTargets].filter(
      (name) => !RUST_INTEGRATION_MANUAL_TARGETS.has(name) && !assigned.has(name),
    ),
  );
  if (missing.length > 0) {
    errors.push(
      "rust: Cargo.toml db-tests integration targets missing from rust.yml inventory: " +
        missing.join(", "),
    );
  }
  return errors;
}
