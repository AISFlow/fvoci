// Shared shapes and pins for the rust.yml verify-workflows checks.
// Workflow documents arrive already parsed; nothing here touches the disk.

export type Mapping = Record<string, unknown>;
export type VerifyContext = { root: string; workflows: Record<string, unknown> };

export const RUST_WORKFLOW_FILE = "rust.yml";
export const CACHE_PIN = "0057852bfaa89a56745cba8c7296529d2fc39830";
export const DOWNLOAD_ARTIFACT_PIN =
  "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093";
// expected_select_if("postgres-build"): the producer shares the postgres selection output.
export const POSTGRES_BUILD_SELECT_IF = "needs.ci-plan.outputs.select_postgres == 'true'";
export const RUST_POSTGRES_BUILD_CACHE_KEY =
  "v3-server-ubuntu-26.04-${{ runner.arch }}-1.98.1-postgres-db-tests-test-nodebug-" +
  "${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml') }}-" +
  "${{ hashFiles('src/**', 'tests/**', 'migrations/**', 'scripts/**', 'vendor/**', 'crates/**', '.cargo/**') }}-" +
  "${{ steps.sqlite.outputs.cache_identity }}";
export const RUST_HELPER_CACHE_KEY =
  "v2-collab-product-helper-ubuntu-26.04-${{ runner.arch }}-1.98.1-worker-dev-nodebug-" +
  "${{ hashFiles('crates/collab-engine/Cargo.toml', 'crates/collab-engine/Cargo.lock', 'rust-toolchain.toml') }}-" +
  "${{ hashFiles('crates/collab-engine/**', 'crates/vendor/**', '.cargo/**') }}";

/** The build cache key for another output identity (first occurrence, no `$` patterns). */
export function buildCacheKey(identity: string): string {
  return RUST_POSTGRES_BUILD_CACHE_KEY.replace("postgres-db-tests-test", () => identity);
}

export function isMapping(value: unknown): value is Mapping {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Structural equality as Python `==` sees parsed YAML/JSON (key order ignored). */
export function same(a: unknown, b: unknown): boolean {
  return Bun.deepEquals(a, b, true);
}

export function indexOfSame(list: readonly unknown[], item: unknown): number {
  return list.findIndex((entry) => same(entry, item));
}

export function has(map: Mapping, key: string): boolean {
  return Object.hasOwn(map, key);
}

export function field(map: unknown, key: string): unknown {
  return isMapping(map) && has(map, key) ? map[key] : undefined;
}

export function str(map: unknown, key: string): string {
  const value = field(map, key);
  return typeof value === "string" ? value : "";
}

export function steps(job: unknown): Mapping[] {
  const value = field(job, "steps");
  return Array.isArray(value) ? value.filter(isMapping) : [];
}

export function rustJobs(ctx: VerifyContext): Mapping | undefined {
  const jobs = field(ctx.workflows[RUST_WORKFLOW_FILE], "jobs");
  return isMapping(jobs) ? jobs : undefined;
}

/**
 * Python reads these jobs with dict/str methods and raises on other shapes, which
 * aborts verify-workflows. Report that shape instead so the check stays fail-closed.
 */
export function jobShapeErrors(jobs: Mapping, names: readonly string[]): string[] {
  const errors: string[] = [];
  for (const name of names) {
    if (!has(jobs, name)) continue;
    const job = jobs[name];
    const list = field(job, "steps");
    const wellFormed =
      isMapping(job) &&
      (list === undefined ||
        (Array.isArray(list) &&
          list.every(
            (step) =>
              isMapping(step) &&
              !(typeof step.name === "object" && step.name !== null) &&
              (!has(step, "run") || typeof step.run === "string") &&
              (!has(step, "with") ||
                (isMapping(step.with) &&
                  (!has(step.with, "path") || typeof step.with.path === "string"))),
          )));
    if (!wellFormed) {
      errors.push(
        `rust: ${name} job must be a mapping whose steps are mappings with string run and with.path and a scalar name`,
      );
    }
  }
  return errors;
}

// Python str.isspace() code points, so split()/strip() and \s agree with the original.
const PY_SPACE =
  "\\t\\n\\v\\f\\r\\x1c-\\x20\\x85\\xa0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
export const PY_WS = `[${PY_SPACE}]`;
export const PY_NON_WS = `[^${PY_SPACE}]`;
const leading = new RegExp(`^${PY_WS}+`);
const trailing = new RegExp(`${PY_WS}+$`);
const runs = new RegExp(`${PY_WS}+`);

export function pyStrip(text: string): string {
  return text.replace(leading, "").replace(trailing, "");
}

export function pySplit(text: string): string[] {
  const trimmed = pyStrip(text);
  return trimmed ? trimmed.split(runs) : [];
}

// str.isprintable() is false for these categories (space excepted), so repr escapes them.
const nonPrintable = /^[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]$/u;
function pyEscape(code: number): string {
  if (code < 0x100) return "\\x" + code.toString(16).padStart(2, "0");
  if (code < 0x10000) return "\\u" + code.toString(16).padStart(4, "0");
  return "\\U" + code.toString(16).padStart(8, "0");
}

/** Python repr() for the scalar values the messages interpolate. */
export function pyRepr(value: unknown): string {
  if (value === undefined || value === null) return "None";
  if (value === true) return "True";
  if (value === false) return "False";
  if (typeof value === "number") return Number.isInteger(value) ? String(value) : pyFloat(value);
  if (typeof value === "string") {
    const quote = value.includes("'") && !value.includes('"') ? '"' : "'";
    let out = "";
    for (const char of value) {
      const code = char.codePointAt(0) ?? 0;
      if (char === "\\") out += "\\\\";
      else if (char === quote) out += "\\" + quote;
      else if (char === "\n") out += "\\n";
      else if (char === "\r") out += "\\r";
      else if (char === "\t") out += "\\t";
      else if (char !== " " && nonPrintable.test(char)) out += pyEscape(code);
      else out += char;
    }
    return quote + out + quote;
  }
  if (Array.isArray(value)) return "[" + value.map(pyRepr).join(", ") + "]";
  if (isMapping(value)) {
    return (
      "{" +
      Object.entries(value)
        .map(([key, item]) => `${pyRepr(key)}: ${pyRepr(item)}`)
        .join(", ") +
      "}"
    );
  }
  // Parsed YAML/JSON holds no other value kinds.
  return Object.prototype.toString.call(value);
}

function pyFloat(value: number): string {
  if (Number.isNaN(value)) return "nan";
  if (!Number.isFinite(value)) return value > 0 ? "inf" : "-inf";
  return String(value);
}
