import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";

const repoRoot = path.resolve(import.meta.dirname, "../../../../");
const lock = readFileSync(path.join(repoRoot, "bun.lock"), "utf8");

/** Every `name@version` Bun resolved, keyed by package name. */
function resolvedVersions(text: string): Map<string, Set<string>> {
  const versions = new Map<string, Set<string>>();
  // Package entries: `    "<key>": ["<name>@<version>", ...` (the key is the
  // name, or `<parent>/<name>` for a nested copy).
  for (const m of text.matchAll(/^ {4}"[^"]+": \["((?:@[^@/"]+\/)?[^@"]+)@([^"]+)"/gm)) {
    const [, name, version] = m;
    const set = versions.get(name!) ?? new Set<string>();
    set.add(version!);
    versions.set(name!, set);
  }
  return versions;
}

test("bun.lock pulls nothing from Tiptap's paid registry", () => {
  // Tiptap Pro packages live under @tiptap-pro/ on registry.tiptap.dev and
  // need a subscription token; FVOCI ships MIT packages only.
  assert.equal(lock.includes("@tiptap-pro/"), false, "bun.lock names a @tiptap-pro/ package");
  assert.equal(lock.includes("registry.tiptap.dev"), false, "bun.lock names registry.tiptap.dev");
});

test("bun.lock resolves one copy of each shared editor, collab, Vue and query runtime", () => {
  const versions = resolvedVersions(lock);
  assert.ok(versions.size > 100, "bun.lock package entries were not recognised");
  const single = [
    "vue",
    "yjs",
    "y-protocols",
    "@hocuspocus/provider",
    "@tanstack/query-core",
    ...[...versions.keys()].filter((name) => name.startsWith("@tiptap/")),
  ];
  for (const name of single) {
    const found = [...(versions.get(name) ?? [])];
    assert.equal(
      found.length,
      1,
      `${name} resolves to ${found.length} versions: ${found.join(", ")}`,
    );
  }
});

test("every Tiptap package bun.lock resolves is pinned by a root override", () => {
  // @fvoci/editor pins Tiptap exactly while Nuxt UI asks for ^ ranges; without
  // an override a lock refresh could give Nuxt UI a second, newer copy.
  const { overrides = {} } = JSON.parse(
    readFileSync(path.join(repoRoot, "package.json"), "utf8"),
  ) as {
    overrides?: Record<string, string>;
  };
  const versions = resolvedVersions(lock);
  const tiptap = [...versions.keys()].filter((name) => name.startsWith("@tiptap/"));
  assert.ok(tiptap.length > 10, "bun.lock Tiptap entries were not recognised");
  for (const name of tiptap) {
    assert.deepEqual(
      [...versions.get(name)!],
      [overrides[name]],
      `${name}: root override must pin the resolved version`,
    );
  }
});
