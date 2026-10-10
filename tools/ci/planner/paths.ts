import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";

// Path classification: which impact family a changed path belongs to. Any
// path that is not positively known to be narrow broadens the plan to full.

export type NarrowFamily = "docs" | "frontend_web_install" | "web_tests";
export type PathKind = NarrowFamily | "broaden" | "unknown";

/** The planner's own checkout; format inputs are read from here, not --repo-root. */
export const PLANNER_ROOT = resolve(import.meta.dir, "../../..");

// Shared build, auth, DB, harness, toolchain, native crates (conservative full).
const BROADEN_PREFIXES = [
  ".github/",
  "migrations/",
  "src/",
  "tests/",
  "scripts/",
  "vendor/",
  "compat/",
  "infra/",
  ".agents/",
  "packages/",
  "crates/",
  // Bun `patchedDependencies` (package.json), applied by every install.
  "patches/",
  "tools/",
  "xtask/",
] as const;

const BROADEN_EXACT: ReadonlySet<string> = new Set([
  "Cargo.toml",
  "Cargo.lock",
  "rust-toolchain.toml",
  "Dockerfile",
  ".dockerignore",
  // The Bun workspace root: apps/web, packages/* and scripts/document-convert.
  "package.json",
  "bun.lock",
  "eslint.config.mjs",
  ".prettierrc.json",
  ".prettierignore",
  "bunfig.toml",
  ".bun-version",
]);

const MANIFEST_MARKERS = [
  "/package.json",
  "/package-lock.json",
  "/bun.lock",
  "/Cargo.toml",
  "/Cargo.lock",
  "/pnpm-lock.yaml",
  "/yarn.lock",
] as const;

// A change set of only `*.md` files is docs wherever it lives, except fixture
// oracles and markdown whose bytes product code, tests or images actually load.
const MD_FIXTURE_PREFIXES = [
  "vendor/markdown/",
  "compat/fixtures/",
  "tests/fixtures/",
  "scripts/fixtures/",
] as const;
export const CONTENT_READ_MARKDOWN: ReadonlySet<string> = new Set([
  "apps/web/NOTICE.md",
  "packages/editor/src/fonts/README.md",
  "scripts/release-notes-template.md",
  "infra/rust/compose.user.INSTALL.md",
  "scripts/testdata/release/compose.user.INSTALL.md",
  // Named by third-party/browser-licenses/manifest.json and bundled into the
  // browser open-source notice.
  "third-party/browser-licenses/supplements/is-emoji-supported-0.0.5-LICENSE.md",
  // Copied into the install image (infra/rust/Dockerfile).
  "vendor/libsql-0.9.30/LICENSE.md",
]);
// Explicit explanatory docs (not build inputs). Exact paths are checked before
// the broaden prefixes so these `.agents/` records stay docs while every other
// `.agents/` path (skills, references) remains full.
export const EXPLICIT_DOCS: ReadonlySet<string> = new Set([
  "README.md",
  "RUNNING.md",
  "docs/rewrite.md",
  "docs/RELEASING.md",
  "docs/collab-engine-comparison.md",
  "AGENTS.md",
  ".agents/environment.md",
]);
const PLANNER_CONTRACT_MARKDOWN = ".agents/skills/fvoci-fast-verify/SKILL.md";
const PARENT_BROADEN_MARKDOWN =
  ".agents/skills/fvoci-standard-implementations/references/candidates.md";

// Generated / contract / config under apps/web (never narrow).
const WEB_BROADEN_PREFIXES = [
  "apps/web/openapi.json",
  "apps/web/src/generated/",
  "apps/web/package.json",
  "apps/web/playwright.config.ts",
] as const;
const FRONTEND_NARROW_PREFIX = "apps/web/src/";

// Browser UI code consumed by Web unit/type checks, production browser builds
// and the install image. Other editor paths keep full validation: schema,
// serialization, CRDT adapters and exports mirror Rust contracts; fonts are
// read by the native export child; i18n/ko.json is include_str! input to a
// Rust test, so the packages/ workspace as a whole never narrows.
const EDITOR_UI_PREFIXES = ["packages/editor/src/react/", "packages/editor/src/vue/"] as const;
const EDITOR_UI_EXACT: ReadonlySet<string> = new Set([
  "packages/editor/src/clipboard.ts",
  "packages/editor/src/gutter-actions.ts",
  "packages/editor/src/menu-roving.ts",
]);
const UI_SUFFIXES = [".ts", ".tsx", ".vue", ".css"] as const;

// The browser suites exercise API/DB/CRDT behaviour with the actual Rust
// server. Only flat specs and reviewed UI helpers narrow; fixtures, server
// lifecycle, wire codecs/oracles, configs and new harness files stay full.
const BROWSER_SPEC_RE = /^apps\/web\/(?:e2e|e2e-pending)\/[^/]+\.spec\.ts$/;
const BROWSER_UI_HELPERS: ReadonlySet<string> = new Set([
  "apps/web/e2e/helpers.ts",
  "apps/web/e2e/mfa-helpers.ts",
  "apps/web/e2e/workspace-wiki-vue-editor.ts",
  "apps/web/e2e-pending/collab-helpers.ts",
  "apps/web/e2e-pending/collab-helpers.test.ts",
]);
const EDITOR_UNIT_TEST_RE = /^packages\/editor\/test\/[^/]+\.test\.ts$/;

const startsWithAny = (path: string, prefixes: readonly string[]) =>
  prefixes.some((prefix) => path.startsWith(prefix));
const endsWithAny = (path: string, suffixes: readonly string[]) =>
  suffixes.some((suffix) => path.endsWith(suffix));

function isFixtureMarkdown(path: string): boolean {
  if (startsWithAny(path, MD_FIXTURE_PREFIXES)) return true;
  const parts = path.split("/");
  return parts.length >= 4 && parts[0] === "crates" && parts[2] === "fixtures";
}

// ---- Prettier inputs: which markdown the web format check reads ----------

/** Positional paths format-web.sh passes to prettier when no paths are given. */
export function formatWebDefaultTargets(script: string): string[] {
  const marker = "set -- ";
  const start = script.indexOf(marker);
  if (start < 0) throw new Error("format-web.sh has no default prettier target list");
  const end = script.indexOf('"$@"', start);
  if (end < 0) throw new Error("format-web.sh default target list is not closed");
  return shellWords(script.slice(start + marker.length, end).replaceAll("\\\n", " "));
}

/** POSIX word splitting with quotes and backslashes (shlex.split, no comments). */
export function shellWords(text: string): string[] {
  const words: string[] = [];
  let word: string | null = null;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i] as string;
    if (ch === "'") {
      const close = text.indexOf("'", i + 1);
      if (close < 0) throw new Error("No closing quotation");
      word = (word ?? "") + text.slice(i + 1, close);
      i = close;
    } else if (ch === '"') {
      let j = i + 1;
      let out = "";
      for (; j < text.length && text[j] !== '"'; j++) {
        if (text[j] === "\\" && j + 1 < text.length && '\\"'.includes(text[j + 1] as string)) j++;
        out += text.charAt(j);
      }
      if (j >= text.length) throw new Error("No closing quotation");
      word = (word ?? "") + out;
      i = j;
    } else if (ch === "\\") {
      if (i + 1 >= text.length) throw new Error("No escaped character");
      word = (word ?? "") + text.charAt(++i);
    } else if (" \t\r\n".includes(ch)) {
      if (word !== null) words.push(word);
      word = null;
    } else {
      word = (word ?? "") + ch;
    }
  }
  if (word !== null) words.push(word);
  return words;
}

export function prettierIgnorePatterns(text: string): string[] {
  // str.splitlines() boundaries, including the ASCII separator controls.
  return (
    text
      // eslint-disable-next-line no-control-regex
      .split(/\r\n|[\n\r\v\f\x1c\x1d\x1e\x85\u2028\u2029]/)
      .map((line) => line.trim())
      .filter((line) => line !== "" && !line.startsWith("#"))
  );
}

const escapeRe = (ch: string) => ch.replace(/[\\^$.*+?()[\]{}|/-]/g, "\\$&");

/** `**`, `*`, `?` over path segments; every other character is literal. */
function globBody(pattern: string): string {
  const body: string[] = [];
  let i = 0;
  while (i < pattern.length) {
    if (pattern.startsWith("**/", i)) {
      body.push("(?:.*/)?");
      i += 3;
    } else if (pattern.startsWith("**", i)) {
      body.push(".*");
      i += 2;
    } else {
      const ch = pattern[i] as string;
      body.push(ch === "*" ? "[^/]*" : ch === "?" ? "[^/]" : escapeRe(ch));
      i++;
    }
  }
  return body.join("");
}

function gitignoreRegex(pattern: string): RegExp {
  let body = pattern;
  const directoryOnly = body.endsWith("/");
  if (directoryOnly) body = body.slice(0, -1);
  let anchored = body.startsWith("/");
  if (anchored) body = body.slice(1);
  else if (body.includes("/")) anchored = true;
  const suffix = directoryOnly ? "(?:/.*)$" : "(?:/.*)?$";
  return new RegExp((anchored ? "^" : "(?:^|/)") + globBody(body) + suffix);
}

export function pathMatchesPrettierIgnore(path: string, patterns: readonly string[]): boolean {
  let ignored = false;
  for (const pattern of patterns) {
    const negated = pattern.startsWith("!");
    if (gitignoreRegex(negated ? pattern.slice(1) : pattern).test(path)) ignored = !negated;
  }
  return ignored;
}

/** `{a,b}` alternatives, as prettier's glob expansion reads them (no nesting). */
export function expandBraces(pattern: string): string[] {
  const match = /\{([^{}]*,[^{}]*)\}/.exec(pattern);
  if (!match) return [pattern];
  const head = pattern.slice(0, match.index);
  const tail = pattern.slice(match.index + match[0].length);
  return (match[1] ?? "").split(",").flatMap((alt) => expandBraces(head + alt + tail));
}

export function prettierTargetMatches(target: string, path: string): boolean {
  if (/[*?[{]/.test(target)) {
    return expandBraces(target).some((alt) => new RegExp(`^${globBody(alt)}$`).test(path));
  }
  const name = target.slice(target.lastIndexOf("/") + 1);
  if (name.includes(".")) return path === target;
  return path === target || path.startsWith(target + "/");
}

const STRICT_UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
/** A text input read as strict UTF-8: undecodable bytes are an error, never U+FFFD. */
export function readUtf8(path: string): string {
  return STRICT_UTF8.decode(readFileSync(path));
}

export type MarkdownInputs = { targets: readonly string[]; ignore: readonly string[] };

export function prettierChecksMarkdown(path: string, inputs: MarkdownInputs): boolean {
  if (!path.endsWith(".md") || pathMatchesPrettierIgnore(path, inputs.ignore)) return false;
  return inputs.targets.some((target) => prettierTargetMatches(target, path));
}

let plannerMarkdownInputs: MarkdownInputs | undefined;
/** format-web.sh targets and .prettierignore of the planner checkout, read once. */
export function loadMarkdownInputs(root = PLANNER_ROOT): MarkdownInputs {
  if (root === PLANNER_ROOT && plannerMarkdownInputs) return plannerMarkdownInputs;
  const inputs = {
    targets: formatWebDefaultTargets(readUtf8(join(root, "scripts/format-web.sh"))),
    ignore: prettierIgnorePatterns(readUtf8(join(root, ".prettierignore"))),
  };
  if (root === PLANNER_ROOT) plannerMarkdownInputs = inputs;
  return inputs;
}

function markdownLane(path: string, inputs: MarkdownInputs | undefined): PathKind {
  if (isFixtureMarkdown(path) || CONTENT_READ_MARKDOWN.has(path)) return "broaden";
  if (path.startsWith(".github/")) return "broaden";
  if (path === PLANNER_CONTRACT_MARKDOWN || path === PARENT_BROADEN_MARKDOWN) return "broaden";
  // Read the format inputs only when a markdown path needs them.
  if (prettierChecksMarkdown(path, inputs ?? loadMarkdownInputs())) return "frontend_web_install";
  return "docs";
}

export function classifyPath(path: string, inputs?: MarkdownInputs): PathKind {
  // Git paths are relative and canonical. Reject unexpected separators or
  // traversal before any allowlist/prefix match, including synthetic inputs.
  if (
    !path ||
    path.includes("\\") ||
    path.split("/").some((p) => p === "" || p === "." || p === "..")
  ) {
    return "unknown";
  }
  if (path.endsWith(".md")) return markdownLane(path, inputs);
  if (EXPLICIT_DOCS.has(path)) return "docs";
  if (BROWSER_SPEC_RE.test(path) || BROWSER_UI_HELPERS.has(path)) return "web_tests";
  if (path === "packages/editor/src/react/schema.tsx") return "broaden";
  if (
    EDITOR_UI_EXACT.has(path) ||
    (startsWithAny(path, EDITOR_UI_PREFIXES) && endsWithAny(path, UI_SUFFIXES))
  ) {
    return "frontend_web_install";
  }
  if (EDITOR_UNIT_TEST_RE.test(path)) return "web_tests";
  if (BROADEN_EXACT.has(path)) return "broaden";
  if (startsWithAny(path, BROADEN_PREFIXES)) return "broaden";
  if (MANIFEST_MARKERS.some((marker) => path.includes(marker))) return "broaden";
  if (path.startsWith("docs/")) return "broaden";
  if (startsWithAny(path, WEB_BROADEN_PREFIXES)) return "broaden";
  if (path.startsWith(FRONTEND_NARROW_PREFIX)) {
    if (!endsWithAny(path, UI_SUFFIXES)) return "broaden";
    if (path.endsWith(".test.ts") || path.endsWith(".test.tsx")) return "web_tests";
    return "frontend_web_install";
  }
  if (path.startsWith("apps/web/")) return "broaden";
  return "unknown";
}

export type Mode = "full" | "narrow";
export type SelectionDecision = {
  mode: Mode;
  reasonCode: string;
  families: ReadonlySet<NarrowFamily>;
};

export function decideFromPaths(
  paths: readonly string[],
  inputs?: MarkdownInputs,
): SelectionDecision {
  if (paths.length === 0)
    return { mode: "full", reasonCode: "FULL_EMPTY_DIFF", families: new Set() };
  const families = new Set<NarrowFamily>();
  for (const path of paths) {
    const kind = classifyPath(path, inputs);
    if (kind === "broaden")
      return { mode: "full", reasonCode: "FULL_PATH_BROADEN", families: new Set() };
    if (kind === "unknown")
      return { mode: "full", reasonCode: "FULL_UNKNOWN_PATH", families: new Set() };
    families.add(kind);
  }
  // Known impact families compose by union; explanatory docs add no jobs.
  const reasonCode = families.has("frontend_web_install")
    ? "NARROW_FRONTEND_WEB_INSTALL"
    : families.has("web_tests")
      ? "NARROW_WEB_TESTS"
      : "NARROW_DOCS";
  return { mode: "narrow", reasonCode, families };
}
