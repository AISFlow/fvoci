// Test-side workflow loading and the trimmed registry tree the original
// suite uses (real workflows and scripts, a one-target Cargo.toml).
import { YAML } from "bun";
import {
  copyFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import type { Mapping } from "./py.ts";
import {
  RUST_CAPACITY_PROBE_SCRIPT,
  RUST_COLLAB_CI_SCRIPT,
  type VerifyContext,
} from "./rust-shared.ts";

export const ROOT = resolve(import.meta.dir, "../../..");

export function loadWorkflows(root: string): Record<string, unknown> {
  const dir = join(root, ".github", "workflows");
  const workflows: Record<string, unknown> = {};
  for (const name of readdirSync(dir).sort()) {
    if (!name.endsWith(".yml") && !name.endsWith(".yaml")) continue;
    try {
      workflows[name] = YAML.parse(readFileSync(join(dir, name), "utf8"));
    } catch (error) {
      workflows[name] = error instanceof Error ? error : new Error(String(error));
    }
  }
  return workflows;
}

export function loadContext(root: string): VerifyContext {
  return { root, workflows: loadWorkflows(root) };
}

export function realJobs(file: "rust.yml" | "web.yml"): Mapping {
  const data = YAML.parse(
    readFileSync(join(ROOT, ".github", "workflows", file), "utf8"),
  ) as Mapping;
  return data["jobs"] as Mapping;
}

export function writeMinimalCargo(root: string, extraTargets: string[] = []): void {
  const lines = ["[features]", "db-tests = []", ""];
  for (const name of ["db_integration", ...extraTargets]) {
    lines.push(
      "[[test]]",
      `name = "${name}"`,
      `path = "tests/${name}.rs"`,
      'required-features = ["db-tests"]',
      "",
    );
  }
  writeFileSync(join(root, "Cargo.toml"), lines.join("\n"));
}

export class RegistryTree {
  readonly root = mkdtempSync(join(tmpdir(), "rust2-"));
  constructor(extraTargets: string[] | null = []) {
    cpSync(join(ROOT, ".github", "workflows"), join(this.root, ".github", "workflows"), {
      recursive: true,
    });
    for (const rel of [RUST_COLLAB_CI_SCRIPT, RUST_CAPACITY_PROBE_SCRIPT]) {
      mkdirSync(dirname(join(this.root, rel)), { recursive: true });
      copyFileSync(join(ROOT, rel), join(this.root, rel));
    }
    if (extraTargets !== null) writeMinimalCargo(this.root, extraTargets);
  }
  write(rel: string, text: string): void {
    mkdirSync(dirname(join(this.root, rel)), { recursive: true });
    writeFileSync(join(this.root, rel), text);
  }
  read(rel: string): string {
    return readFileSync(join(this.root, rel), "utf8");
  }
  context(mutateRust?: (jobs: Mapping) => void): VerifyContext {
    const ctx = loadContext(this.root);
    if (mutateRust) mutateRust((ctx.workflows["rust.yml"] as Mapping)["jobs"] as Mapping);
    return ctx;
  }
  [Symbol.dispose](): void {
    rmSync(this.root, { recursive: true, force: true });
  }
}

export function catalogRows(job: Mapping): Mapping[] {
  return JSON.parse(
    (job["env"] as Mapping)["FVOCI_POSTGRES_MATRIX_CATALOG"] as string,
  ) as Mapping[];
}

export function storeCatalog(job: Mapping, rows: Mapping[]): void {
  (job["env"] as Mapping)["FVOCI_POSTGRES_MATRIX_CATALOG"] = JSON.stringify(rows);
}

export function namedStep(job: Mapping, name: string): Mapping {
  const step = (job["steps"] as Mapping[]).find((item) => item["name"] === name);
  if (!step) throw new Error(`missing step ${name}`);
  return step;
}
