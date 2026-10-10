import { YAML } from "bun";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { extname, join } from "node:path";

// Parsed YAML values. Bun.YAML implements the YAML 1.2 core schema, which is
// how the Actions runner reads workflows: `on` stays a string key, `yes`/`off`
// stay strings and `0755` is decimal. PyYAML (YAML 1.1) differs on exactly
// those spellings; none occurs in the registered workflows and every pinned
// value is compared strictly, so a 1.1-only spelling fails closed here.
export type Value = null | boolean | number | string | Value[] | Mapping;
export type Mapping = { [key: string]: Value };

export type VerifyContext = {
  root: string;
  /** Parsed top-level mappings keyed by file name ("rust.yml"). */
  workflows: Record<string, Mapping>;
  /** Raw text of every discovered workflow file, for literal-order checks. */
  texts: Record<string, string>;
  /** Parse or shape error for a discovered file that has no entry in workflows. */
  loadErrors: Record<string, string>;
  /** Sorted .yml/.yaml file names, or null when .github/workflows is missing. */
  files: string[] | null;
};

export type WorkflowCheck = (ctx: VerifyContext) => string[];

export function isMapping(value: unknown): value is Mapping {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Own-property lookup: a YAML key such as `__proto__` never reaches the prototype. */
export function get(value: unknown, key: string): Value | undefined {
  return isMapping(value) && Object.hasOwn(value, key) ? value[key] : undefined;
}

export function has(value: unknown, key: string): boolean {
  return isMapping(value) && Object.hasOwn(value, key);
}

/** Deep structural equality with strict scalar types (false is not 0). */
export function deepEqual(a: unknown, b: unknown): boolean {
  return Bun.deepEquals(a, b, true);
}

export function sameKeys(value: unknown, keys: readonly string[]): boolean {
  if (!isMapping(value)) return false;
  const actual = Object.keys(value);
  return actual.length === new Set(keys).size && keys.every((key) => Object.hasOwn(value, key));
}

/** The workflow trigger mapping; `on` is a plain string key under YAML 1.2. */
export function triggersOf(data: Mapping): Value | undefined {
  return get(data, "on");
}

/** Python-style repr of a string: the quoting the registry messages use. */
// Python's str.isprintable() is false for these categories (space excepted).
const NON_PRINTABLE = /^[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]$/u;

export function pyRepr(text: string): string {
  const quote = text.includes("'") && !text.includes('"') ? '"' : "'";
  let body = "";
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    if (char === "\\") body += "\\\\";
    else if (char === quote) body += "\\" + char;
    else if (char === "\n") body += "\\n";
    else if (char === "\r") body += "\\r";
    else if (char === "\t") body += "\\t";
    else if (char !== " " && NON_PRINTABLE.test(char)) {
      const hex = code.toString(16);
      body +=
        code <= 0xff
          ? "\\x" + hex.padStart(2, "0")
          : code <= 0xffff
            ? "\\u" + hex.padStart(4, "0")
            : "\\U" + hex.padStart(8, "0");
    } else body += char;
  }
  return quote + body + quote;
}

export function pyReprList(items: readonly string[]): string {
  return "[" + items.map(pyRepr).join(", ") + "]";
}

export type Parsed = { data: Mapping; error?: undefined } | { data?: undefined; error: string };

/** A parse failure or a non-mapping document is an error for that file. */
export function parseWorkflow(name: string, text: string): Parsed {
  let data: unknown;
  try {
    data = YAML.parse(text);
  } catch (error) {
    return {
      error: `${name}: YAML parse failed: ${error instanceof Error ? error.message : String(error)}`,
    };
  }
  if (!isMapping(data)) return { error: `${name}: workflow YAML must be a mapping` };
  return { data };
}

export function listWorkflowFiles(root: string): string[] | null {
  const directory = join(root, ".github", "workflows");
  if (!existsSync(directory) || !statSync(directory).isDirectory()) return null;
  return readdirSync(directory)
    .filter((name) => {
      const ext = extname(name);
      if (ext !== ".yml" && ext !== ".yaml") return false;
      // Like Path.is_file(): a dangling link is not a file.
      return statSync(join(directory, name), { throwIfNoEntry: false })?.isFile() ?? false;
    })
    .sort();
}

/** Context for workflow texts already in memory (file name -> text). */
export function contextFromTexts(
  root: string,
  texts: Readonly<Record<string, string>>,
): VerifyContext {
  const files = Object.keys(texts).sort();
  const ctx: VerifyContext = { root, workflows: {}, texts: { ...texts }, loadErrors: {}, files };
  for (const name of files) {
    const parsed = parseWorkflow(name, texts[name] ?? "");
    if (parsed.data) ctx.workflows[name] = parsed.data;
    else ctx.loadErrors[name] = parsed.error;
  }
  return ctx;
}

export function loadContext(root: string): VerifyContext {
  const files = listWorkflowFiles(root);
  const ctx: VerifyContext = { root, workflows: {}, texts: {}, loadErrors: {}, files };
  for (const name of files ?? []) {
    let text: string;
    try {
      // Strict UTF-8 and universal newlines, like Python's read_text().
      text = new TextDecoder("utf-8", { fatal: true })
        .decode(readFileSync(join(root, ".github", "workflows", name)))
        .replace(/\r\n?/g, "\n");
    } catch (error) {
      ctx.loadErrors[name] =
        `${name}: YAML parse failed: ${error instanceof Error ? error.message : String(error)}`;
      continue;
    }
    ctx.texts[name] = text;
    const parsed = parseWorkflow(name, text);
    if (parsed.data) ctx.workflows[name] = parsed.data;
    else ctx.loadErrors[name] = parsed.error;
  }
  return ctx;
}
