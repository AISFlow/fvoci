// Root Cargo db-tests integration targets that rust.yml must schedule: explicit
// [[test]] entries with the db-tests feature plus autodiscovered tests/*.rs crates.
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { isMapping, pyStrip } from "./rust-common.ts";

export const RUST_DB_TESTS_FEATURE = "db-tests";
// Autodiscovered root tests that run in the fast/native jobs instead of a DB bucket.
export const RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS: ReadonlySet<string> = new Set([
  "collab_wire",
  "markdown_process",
  "docx_export_process",
  "pdf_export_process",
  "pptx_export_process",
  "doctor_conversion",
  "office_extract_process",
  "static_api",
]);

export type AutotestFile = { stem: string; text: string };
export type TargetsResult =
  { targets: Set<string>; error: null } | { targets: null; error: string };

// Python str.splitlines() boundaries, including its file/group/record separators.
// eslint-disable-next-line no-control-regex -- the separators are the point
const lineBreak = new RegExp("\\r\\n|[\\n\\r\\v\\f\\x1c\\x1d\\x1e\\x85\\u2028\\u2029]");

function crateAttributes(text: string): string[] {
  return text
    .split(lineBreak)
    .map(pyStrip)
    .filter((line) => line.startsWith("#!["));
}

function declaresDbTests(attributes: readonly string[]): boolean {
  for (const attribute of attributes) {
    if (attribute.includes("extract-native-tests")) return false;
    if (attribute.includes('feature = "db-tests"') || attribute.includes('feature="db-tests"'))
      return true;
  }
  return false;
}

// Python bool() of a parsed TOML value.
function truthy(value: unknown): boolean {
  if (Array.isArray(value)) return value.length > 0;
  if (isMapping(value)) return Object.keys(value).length > 0;
  return Boolean(value);
}

/** Pure core over the Cargo.toml text and the sorted tests/*.rs files. */
export function dbIntegrationTargets(
  cargoText: string,
  autotests: readonly AutotestFile[] | null,
): TargetsResult {
  let data: unknown;
  try {
    data = Bun.TOML.parse(cargoText);
  } catch (error) {
    return { targets: null, error: `rust: Cargo.toml parse failed: ${(error as Error).message}` };
  }
  const cargo = isMapping(data) ? data : {};
  const targets = new Set<string>();
  const entries = cargo.test;
  if (Array.isArray(entries)) {
    for (const entry of entries) {
      if (!isMapping(entry))
        return { targets: null, error: "rust: Cargo.toml [[test]] entry must be a table" };
      const name = entry.name;
      if (typeof name !== "string" || !name) {
        return { targets: null, error: "rust: Cargo.toml [[test]] missing name" };
      }
      const features = Object.hasOwn(entry, "required-features") ? entry["required-features"] : [];
      if (!Array.isArray(features) || !features.every((item) => typeof item === "string")) {
        return {
          targets: null,
          error: `rust: Cargo.toml [[test]] ${name} required-features must be a string list`,
        };
      }
      if (features.includes(RUST_DB_TESTS_FEATURE)) targets.add(name);
    }
  }
  const pkg = cargo.package;
  const autotestsEnabled =
    isMapping(pkg) && Object.hasOwn(pkg, "autotests") ? truthy(pkg.autotests) : true;
  if (autotestsEnabled && autotests) {
    for (const file of autotests) {
      const { stem } = file;
      const text = file.text;
      const attributes = crateAttributes(text);
      if (declaresDbTests(attributes)) {
        targets.add(stem);
        continue;
      }
      if (attributes.some((attribute) => attribute.includes("extract-native-tests"))) continue;
      if (RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS.has(stem)) continue;
      if (attributes.length || pyStrip(text)) {
        return {
          targets: null,
          error:
            `rust: tests/${stem}.rs is not registered and has no crate ` +
            '#![cfg(feature = "db-tests")]; add CI inventory or an explicit fast/native exclusion',
        };
      }
    }
  }
  return { targets, error: null };
}

class UnreadableFile extends Error {}
const utf8 = new TextDecoder("utf-8", { fatal: true });

// Python read_text(encoding="utf-8") raises on invalid bytes; keep that fail-closed.
function readUtf8(path: string, label: string): string {
  try {
    return utf8.decode(readFileSync(path));
  } catch {
    throw new UnreadableFile(`rust: ${label} must be a readable UTF-8 file`);
  }
}

/**
 * Reads <root>/Cargo.toml, then each tests/*.rs only when the scan reaches it, in
 * the order Python does (the tests list is null when tests/ is absent).
 */
export function rootDbIntegrationRegistryTargets(root: string): TargetsResult {
  const cargoPath = join(root, "Cargo.toml");
  if (!isFile(cargoPath)) return { targets: null, error: "rust: missing root Cargo.toml" };
  const testsDir = join(root, "tests");
  const autotests = isDirectory(testsDir)
    ? readdirSync(testsDir)
        .filter((name) => name.endsWith(".rs"))
        .sort()
        .map((name) => ({
          stem: name === ".rs" ? name : name.slice(0, -3),
          get text() {
            return readUtf8(join(testsDir, name), "tests/" + name);
          },
        }))
    : null;
  try {
    return dbIntegrationTargets(readUtf8(cargoPath, "Cargo.toml"), autotests);
  } catch (error) {
    if (error instanceof UnreadableFile) return { targets: null, error: error.message };
    throw error;
  }
}

function isFile(path: string): boolean {
  return statSync(path, { throwIfNoEntry: false })?.isFile() ?? false;
}

function isDirectory(path: string): boolean {
  return statSync(path, { throwIfNoEntry: false })?.isDirectory() ?? false;
}
