import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  classifyPath,
  CONTENT_READ_MARKDOWN,
  decideFromPaths,
  EXPLICIT_DOCS,
  expandBraces,
  formatWebDefaultTargets,
  loadMarkdownInputs,
  pathMatchesPrettierIgnore,
  PLANNER_ROOT,
  prettierTargetMatches,
  shellWords,
} from "./paths.ts";
import { isOptIn, planAll } from "./test-support.ts";
import type { Workflow } from "./registry.ts";

const AGENT_DOCS = ["AGENTS.md", ".agents/environment.md"];

function expectLanes(
  paths: string[],
  selected: Workflow[],
  mode: "narrow" | "full" = "narrow",
): void {
  for (const [workflow, plan] of Object.entries(planAll(paths)) as [
    Workflow,
    ReturnType<typeof planAll>[Workflow],
  ][]) {
    expect(plan.mode, `${paths.join(",")} ${workflow}`).toBe(mode);
    for (const [job, meta] of Object.entries(plan.jobs)) {
      const want = (mode === "full" || selected.includes(workflow)) && !isOptIn(workflow, job);
      expect(meta.selected, `${paths.join(",")} ${workflow}/${job}`).toBe(want);
    }
  }
}

describe("classifyPath", () => {
  test("ClassifyPathsTest", () => {
    expect(classifyPath("docs/rewrite.md")).toBe("docs");
    expect(classifyPath("docs/other.md")).toBe("docs");
    expect(classifyPath("apps/web/src/foo.ts")).toBe("frontend_web_install");
    expect(classifyPath("apps/web/src/generated/api.ts")).toBe("broaden");
    expect(classifyPath("apps/web/e2e/foo.spec.ts")).toBe("web_tests");
    expect(classifyPath("packages/editor/x.ts")).toBe("broaden");
    expect(classifyPath("crates/collab-engine/src/x.rs")).toBe("broaden");
    expect(classifyPath("compat/fixtures/x")).toBe("broaden");
  });

  test("canonical paths only", () => {
    for (const path of [
      "",
      "a//b",
      "./a",
      "a/../b",
      "a\\b",
      "apps/web/src/../../src/main.rs",
      "/abs",
    ]) {
      expect(classifyPath(path), path).toBe("unknown");
    }
  });

  test("agent records are docs; skills and the planner contract are full", () => {
    for (const path of AGENT_DOCS) expect(classifyPath(path)).toBe("docs");
    expect(classifyPath(".agents/environment.md.bak")).toBe("broaden");
    expect(classifyPath(".agents/environment.mdx")).toBe("broaden");
    for (const path of ["AGENTS.MD", "AGENTS.md.orig", ".agents/"])
      expect(classifyPath(path), path).toBe("unknown");
    expect(classifyPath(".agents/skills/fvoci-fast-verify/SKILL.md")).toBe("broaden");
    expect(
      classifyPath(".agents/skills/fvoci-standard-implementations/references/candidates.md"),
    ).toBe("broaden");
    for (const path of [
      ".agents/other.md",
      ".agents/sub/environment.md",
      "apps/AGENTS.md",
      "scripts/AGENTS.md",
    ]) {
      expect(classifyPath(path), path).toBe("docs");
    }
  });

  test("explicit docs never overlap build inputs", () => {
    for (const path of EXPLICIT_DOCS) {
      expect(path.endsWith("/") || path.includes("*")).toBe(false);
      expect(classifyPath(path), path).toBe("docs");
    }
  });

  // Policy: a markdown-only change is docs, except fixture/oracle markdown
  // and markdown whose bytes are real inputs (full), and prettier inputs
  // (web format lane).
  test("fixture and oracle markdown stays full", () => {
    for (const path of [
      "compat/fixtures/markdown-oracle/01-basic.md",
      "compat/fixtures/HOCUS-WIRE.md",
      "tests/fixtures/search/NOTICE.md",
      "scripts/fixtures/web-e2e/x.md",
      "vendor/markdown/README.md",
      "crates/collab-engine/fixtures/NOTICE.md",
      "crates/document-extract/fixtures/NOTICE.md",
    ]) {
      expect(classifyPath(path), path).toBe("broaden");
    }
  });

  test("markdown consumed as real input stays full", () => {
    for (const path of CONTENT_READ_MARKDOWN) expect(classifyPath(path), path).toBe("broaden");
    // Bundled into the browser open-source notice by its manifest.
    const manifest = JSON.parse(
      readFileSync(join(PLANNER_ROOT, "third-party/browser-licenses/manifest.json"), "utf8"),
    ) as Record<string, { file?: string }>;
    const supplements = Object.values(manifest)
      .map((entry) => entry.file)
      .filter((file): file is string => typeof file === "string" && file.endsWith(".md"))
      .map((file) => `third-party/browser-licenses/${file}`);
    expect(supplements.length).toBeGreaterThan(0);
    for (const path of supplements) expect(classifyPath(path), path).toBe("broaden");
    // Copied into the install image.
    const dockerfile = readFileSync(join(PLANNER_ROOT, "infra/rust/Dockerfile"), "utf8");
    const copied = [...dockerfile.matchAll(/COPY [^\n]*?\/src\/(\S+\.md) /g)].map(
      (m) => m[1] as string,
    );
    expect(copied).toContain("vendor/libsql-0.9.30/LICENSE.md");
    for (const path of copied) expect(classifyPath(path), path).toBe("broaden");
  });

  test("other markdown is docs, prettier-checked markdown uses the web lane", () => {
    for (const path of [
      "README.md",
      "notes.md",
      "third-party/x.md",
      "docs/fixtures/example.md",
      "vendor/libsql-0.9.30/README.md",
    ]) {
      expect(classifyPath(path), path).toBe("docs");
    }
    expect(classifyPath("scripts/WEB_LINT.md")).toBe("frontend_web_install");
    // format-web.sh passes 'scripts/schema-baseline/*.{ts,md}'; prettier expands the braces.
    expect(classifyPath("scripts/schema-baseline/compare-catalogs.md")).toBe(
      "frontend_web_install",
    );
    expect(classifyPath("scripts/schema-baseline/README.md")).toBe("frontend_web_install");
    // .prettierignore excludes these from the format check.
    expect(classifyPath("packages/editor/NOTICE.md")).toBe("docs");
    expect(classifyPath("apps/web/node_modules/x/README.md")).toBe("docs");
    expect(classifyPath(".github/PULL_REQUEST_TEMPLATE.md")).toBe("broaden");
  });
});

describe("prettier inputs", () => {
  test("format-web.sh default targets are read word for word", () => {
    const targets = loadMarkdownInputs().targets;
    expect(targets).toContain("scripts/WEB_LINT.md");
    expect(targets).toContain("tools/web-e2e/**/*.ts");
    expect(targets).toContain("scripts/schema-baseline/*.{ts,md}");
    expect(targets).not.toContain('"$@"');
    expect(() => formatWebDefaultTargets("echo none")).toThrow("no default prettier target list");
    expect(() => formatWebDefaultTargets("set -- a b")).toThrow("not closed");
  });

  test("shell words", () => {
    expect(shellWords(` a 'b c' "d\\"e" f\\ g 'x'"y" `)).toEqual(["a", "b c", 'd"e', "f g", "xy"]);
    expect(() => shellWords("'open")).toThrow("No closing quotation");
  });

  test("glob and ignore matching", () => {
    expect(expandBraces("a/*.{ts,md}")).toEqual(["a/*.ts", "a/*.md"]);
    expect(prettierTargetMatches("tools/web-e2e/**/*.ts", "tools/web-e2e/a/b.ts")).toBe(true);
    expect(prettierTargetMatches("tools/web-e2e/**/*.ts", "tools/web-e2e/b.ts")).toBe(true);
    expect(prettierTargetMatches("apps/web", "apps/web/x.md")).toBe(true);
    expect(prettierTargetMatches("apps/web", "apps/webx/x.md")).toBe(false);
    expect(prettierTargetMatches("scripts/WEB_LINT.md", "scripts/WEB_LINT.md.bak")).toBe(false);
    expect(pathMatchesPrettierIgnore("a/node_modules/b.md", ["**/node_modules/**"])).toBe(true);
    expect(pathMatchesPrettierIgnore("x.md", ["*.md", "!x.md"])).toBe(false);
    expect(pathMatchesPrettierIgnore("d/x.md", ["/x.md"])).toBe(false);
    expect(pathMatchesPrettierIgnore("d/x.md", ["x.md"])).toBe(true);
  });
});

describe("impact union", () => {
  test("browser and unit tests run web without install", () => {
    for (const path of [
      "apps/web/e2e/new-flow.spec.ts",
      "apps/web/e2e-pending/workspace-wiki-vue-collab.spec.ts",
      "apps/web/e2e/helpers.ts",
      "apps/web/e2e/mfa-helpers.ts",
      "apps/web/e2e/workspace-wiki-vue-editor.ts",
      "apps/web/e2e-pending/collab-helpers.ts",
      "apps/web/e2e-pending/collab-helpers.test.ts",
      "apps/web/src/vue/router.test.ts",
      "packages/editor/test/vue-menu-selection.test.ts",
    ]) {
      expectLanes([path], ["web"]);
      expectLanes(["docs/rewrite.md", path], ["web"]);
    }
  });

  test("editor UI keeps browser and install", () => {
    for (const path of [
      "packages/editor/src/vue/FvociEditor.vue",
      "packages/editor/src/react/block-menu.tsx",
      "packages/editor/src/react/editor.css",
      "packages/editor/src/clipboard.ts",
      "packages/editor/src/gutter-actions.ts",
      "packages/editor/src/menu-roving.ts",
    ]) {
      expectLanes([path], ["web", "install"]);
      expectLanes(["README.md", "apps/web/e2e/foo.spec.ts", path], ["web", "install"]);
    }
  });

  test("explanatory docs are exact", () => {
    expectLanes(["docs/RELEASING.md", "docs/collab-engine-comparison.md"], []);
    for (const path of ["docs/fixtures/example.md", "docs/generated/api.md", "docs/other.md"]) {
      expect(decideFromPaths([path]).reasonCode, path).toBe("NARROW_DOCS");
    }
    expect(decideFromPaths(["docs/collab-engine-comparison.md.bak"]).mode).toBe("full");
  });

  test("backend contracts, harness and unknown paths stay full", () => {
    for (const path of [
      "packages/editor/src/tiptap-schema.ts",
      "packages/editor/src/collab-tiptap.ts",
      "packages/editor/src/json.ts",
      "packages/editor/src/export/pdf.tsx",
      "packages/editor/src/fonts/NotoSansKR.ttf",
      "packages/editor/src/react/schema.tsx",
      "packages/editor/src/vue/new.wasm",
      "packages/editor/test/schema-dump.ts",
      "packages/editor/test/setup/vue-sfc.ts",
      "packages/editor/tsconfig.json",
      "packages/i18n/src/locales/ko.json",
      "apps/web/e2e/fixtures/markdown-import.zip",
      "apps/web/e2e/nested/foo.spec.ts",
      "apps/web/e2e/new-harness.ts",
      "apps/web/e2e-pending/collab-restart.ts",
      "apps/web/e2e-pending/collab-wire.ts",
      "apps/web/e2e-pending/collab-attachment-oracle.ts",
      "apps/web/e2e-pending/collab-playwright.config.ts",
      "apps/web/src/generated/api.test.ts",
      "apps/web/src/fixtures/backend.sql",
      "apps/web/src/new-contract.json",
      "scripts/run-web-e2e.sh",
      "scripts/ci_selection.py",
      "tools/ci/plan.ts",
      "tools/ci/planner/paths.ts",
      "src/auth.rs",
      "migrations/045.sql",
      "new-unknown-file.ts",
      "apps/web/src/../../src/main.rs",
      "apps/web//src/test.ts",
    ]) {
      expectLanes(["README.md", "apps/web/e2e/foo.spec.ts", path], [], "full");
    }
  });

  test("agent docs union with other changes", () => {
    for (const extra of [
      "src/lib.rs",
      ".github/workflows/rust.yml",
      "scripts/test-ci-selection.sh",
      ".agents/skills/fvoci-fast-verify/SKILL.md",
      "Cargo.toml",
      ".dockerignore",
      "infra/rust/Dockerfile",
      "apps/web/package.json",
      "crates/collab-engine/Cargo.toml",
      "package.json",
      "bun.lock",
      ".bun-version",
      "patches/@volar%2Ftypescript@2.4.28.patch",
      "scripts/document-convert/package.json",
      "compat/fixtures/x.json",
    ]) {
      expect(decideFromPaths([...AGENT_DOCS, extra]).reasonCode, extra).toBe("FULL_PATH_BROADEN");
    }
    for (const extra of [".gitignore", "LICENSE"]) {
      expect(decideFromPaths([...AGENT_DOCS, extra]).reasonCode, extra).toBe("FULL_UNKNOWN_PATH");
    }
    expectLanes([...AGENT_DOCS, "docs/other.md", "third-party/x.md", "notes.md"], []);
    expectLanes([...AGENT_DOCS, "apps/web/src/x.ts"], ["web", "install"]);
  });

  test("frozen candidate inventories", () => {
    // Regression evidence only; production paths come from verified git.
    const snapshot = JSON.parse(
      readFileSync(join(PLANNER_ROOT, "scripts/fixtures/ci-selection/candidates.json"), "utf8"),
    ) as {
      candidates: { number: number; head_sha: string; paths: string[] }[];
    };
    expect(new Set(snapshot.candidates.map((c) => c.number))).toEqual(
      new Set([265, 267, 269, 270, 271, 263, 280]),
    );
    for (const item of snapshot.candidates) {
      expectLanes(item.paths, item.number === 263 || item.number === 280 ? [] : ["web", "install"]);
    }
  });

  test("empty diff is full", () => {
    expect(decideFromPaths([])).toEqual({
      mode: "full",
      reasonCode: "FULL_EMPTY_DIFF",
      families: new Set(),
    });
  });
});
