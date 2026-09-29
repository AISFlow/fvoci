import assert from "node:assert/strict";
import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import test from "node:test";

// The Vue app must not bundle React: a React import anywhere in its module
// graph (a query module that also builds react-query options, say) loads
// React and react-query into the Vue pages. This walks the graph from the
// Vue entry through the web sources and the @fvoci/* workspace packages.

const web = path.resolve(import.meta.dirname, "../..");
const packages = path.resolve(web, "../../packages");
const REACT = /^(react|react-dom|react-router-dom|@tanstack\/react-query|@hocuspocus\/provider-react|@tiptap\/react|@tiptap\/extension-drag-handle-react|@radix-ui\/[^/]+)(\/|$)/;

function packageEntry(name: "editor" | "i18n", subpath: string): string {
  const manifest = JSON.parse(readFileSync(path.join(packages, name, "package.json"), "utf8")) as {
    exports: Record<string, string>;
  };
  const target = manifest.exports[subpath === "" ? "." : `./${subpath}`];
  assert.ok(target, `@fvoci/${name}/${subpath} is not exported`);
  return path.join(packages, name, target);
}

function resolve(spec: string, from: string): string | null {
  let base: string;
  if (spec.startsWith("@/")) base = path.join(web, "src", spec.slice(2));
  else if (spec.startsWith(".")) base = path.resolve(path.dirname(from), spec);
  else {
    const match = /^@fvoci\/(editor|i18n)(?:\/(.+))?$/.exec(spec);
    if (!match) return null;
    const entry = packageEntry(match[1] as "editor" | "i18n", match[2] ?? "");
    return entry.endsWith(".css") ? null : entry;
  }
  base = base.replace(/\.js$/, "");
  for (const candidate of [base, `${base}.ts`, `${base}.tsx`, `${base}.vue`, path.join(base, "index.ts")]) {
    if (statSync(candidate, { throwIfNoEntry: false })?.isFile()) return candidate;
  }
  return null;
}

test("the Vue app's module graph imports no React module", () => {
  const seen = new Set<string>();
  const found: string[] = [];
  const walk = (file: string) => {
    if (seen.has(file)) return;
    seen.add(file);
    assert.equal(file.endsWith(".tsx"), false, `a React component module: ${path.relative(web, file)}`);
    const source = readFileSync(file, "utf8");
    for (const [, spec] of source.matchAll(/(?:from|import)\s*\(?\s*["']([^"']+)["']/g)) {
      if (!spec) continue;
      if (REACT.test(spec)) found.push(`${spec} in ${path.relative(web, file)}`);
      const next = resolve(spec, file);
      if (next && !next.endsWith(".css")) walk(next);
    }
  };
  walk(path.join(web, "src/vue/main.ts"));
  assert.ok(seen.size > 100, `walked ${seen.size} modules`);
  assert.deepEqual(found, []);
});

// What the Vue entry imports statically is loaded on the first load of every
// Vue page; each page is a lazy route chunk (router.ts). The wiki editor
// stack (Tiptap, ProseMirror, Yjs, the collab provider, KaTeX) and its
// stylesheets belong to the wiki page's chunk: the Gantt page must not load
// them. Type-only imports and import() calls are not static loads.
const STATIC_IMPORT = /^\s*(?:import|export)\s+(?!type\s)(?:[^;'"]*?\sfrom\s*)?["']([^"']+)["']/gm;
const EDITOR_STACK = /^(@fvoci\/editor|@tiptap\/|@hocuspocus\/|yjs|y-protocols|y-prosemirror|prosemirror-|katex)(\/|$)/;
const EDITOR_FILES = [path.join(packages, "editor") + path.sep, path.join(web, "src/features/documents") + path.sep];

test("the Vue entry's static graph leaves the wiki editor to the wiki page's chunk", () => {
  const seen = new Set<string>();
  const found: string[] = [];
  const walk = (file: string) => {
    if (seen.has(file)) return;
    seen.add(file);
    const source = readFileSync(file, "utf8");
    for (const [, spec] of source.matchAll(STATIC_IMPORT)) {
      if (!spec) continue;
      if (EDITOR_STACK.test(spec)) found.push(`${spec} in ${path.relative(web, file)}`);
      const next = resolve(spec, file);
      if (!next) continue;
      if (EDITOR_FILES.some((dir) => next.startsWith(dir))) found.push(`${path.relative(web, next)} from ${path.relative(web, file)}`);
      if (!next.endsWith(".css")) walk(next);
    }
  };
  walk(path.join(web, "src/vue/main.ts"));
  assert.ok(seen.has(path.join(web, "src/vue/router.ts")), "the walk reaches the router");
  for (const page of [
    "WikiDocumentPage.vue",
    "ProjectGanttPage.vue",
    "AttachmentViewPage.vue",
    "ShareAttachmentViewPage.vue",
  ]) {
    assert.equal(seen.has(path.join(web, "src/vue/pages", page)), false, `${page} is a lazy route chunk`);
  }
  assert.deepEqual(found, []);
});
