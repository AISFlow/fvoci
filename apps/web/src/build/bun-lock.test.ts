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
    assert.equal(found.length, 1, `${name} resolves to ${found.length} versions: ${found.join(", ")}`);
  }
});
