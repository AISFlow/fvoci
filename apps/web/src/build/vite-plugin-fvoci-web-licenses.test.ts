import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  collectWorkerModuleIds,
  mergeWorkerLicenses,
  packageLicenseEntries,
} from "../../vite-plugin-fvoci-web-licenses.ts";
import { finalizeBrowserOpenSourceNotice } from "./web-licenses.ts";

const webRoot = path.resolve(import.meta.dirname, "../..");
const repoRoot = path.resolve(webRoot, "../..");
const manifestPath = path.join(repoRoot, "third-party/browser-licenses/manifest.json");

test("the worker collector records chunk module ids and skips assets", () => {
  const ids = new Set<string>();
  const plugin = collectWorkerModuleIds(ids);
  const generate = plugin.generateBundle as (options: unknown, bundle: Record<string, unknown>) => void;
  generate.call({}, {}, {
    "worker.js": { type: "chunk", moduleIds: ["/a/node_modules/x/index.js", "/src/worker.ts"] },
    "worker.css": { type: "asset" },
  });
  assert.deepEqual([...ids], ["/a/node_modules/x/index.js", "/src/worker.ts"]);
});

test("package entries use the package root, like Vite's build.license", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "fvoci-worker-licenses-"));
  try {
    const pkg = path.join(root, "node_modules", "@scope", "pkg");
    fs.mkdirSync(path.join(pkg, "esm"), { recursive: true });
    fs.mkdirSync(path.join(pkg, "sub"), { recursive: true });
    fs.writeFileSync(path.join(pkg, "package.json"), JSON.stringify({ name: "@scope/pkg", version: "1.2.3", license: " MIT " }));
    fs.writeFileSync(path.join(pkg, "LICENSE.md"), "\nMIT text\n");
    // Nested package.json files (a type marker, a named sub-entry) are not the package.
    fs.writeFileSync(path.join(pkg, "esm", "package.json"), JSON.stringify({ type: "module" }));
    fs.writeFileSync(path.join(pkg, "sub", "package.json"), JSON.stringify({ name: "inner", version: "9.9.9" }));
    const bare = path.join(root, "node_modules", "bare");
    fs.mkdirSync(bare, { recursive: true });
    fs.writeFileSync(path.join(bare, "package.json"), JSON.stringify({ name: "bare" }));
    const entries = packageLicenseEntries([
      path.join(pkg, "esm", "index.js"),
      path.join(pkg, "sub", "index.js"),
      path.join(bare, "index.js"),
      "\0virtual:node_modules/x",
      path.join(webRoot, "src", "features", "attachments", "xlsx-worker.ts"),
    ]);
    assert.deepEqual(entries, [
      { name: "@scope/pkg", version: "1.2.3", identifier: "MIT", text: "MIT text" },
      { name: "bare", version: "0.0.0" },
    ]);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("worker-only packages join Vite's entries sorted by id; Vite's entry wins a duplicate", () => {
  const vite = JSON.stringify([
    { name: "react", version: "19.3.0", identifier: "MIT", text: "from vite" },
    { name: "zod", version: "3.25.76", identifier: "MIT", text: "z" },
  ]);
  const merged = JSON.parse(
    mergeWorkerLicenses(vite, [
      { name: "react", version: "19.3.0", identifier: "MIT", text: "from worker" },
      { name: "fflate", version: "0.8.3", identifier: "MIT", text: "f" },
    ]),
  ) as { name: string; text: string }[];
  assert.deepEqual(
    merged.map((entry) => `${entry.name}:${entry.text}`),
    ["fflate:f", "react:from vite", "zod:z"],
  );
});

/** The directory `name` is installed in for the web app: its own or a workspace ancestor's node_modules. */
function installedPackageDir(name: string): string {
  for (let dir = webRoot; ; dir = path.dirname(dir)) {
    const candidate = path.join(dir, "node_modules", name);
    if (fs.existsSync(path.join(candidate, "package.json"))) return candidate;
    if (path.dirname(dir) === dir) throw new Error(`${name} is not installed`);
  }
}

test("installed XLSX worker packages reach the public notice, with the existing supplements", () => {
  const modules = [
    ["@office-kit/xlsx", "dist/io.mjs"],
    ["fflate", "esm/browser.js"],
    ["saxes", "saxes.js"],
    ["@nodable/entities", "src/index.js"],
  ].map(([name, file]) => path.join(installedPackageDir(name!), file!));
  const entries = packageLicenseEntries(modules);
  assert.deepEqual(
    entries.map((entry) => `${entry.name}@${entry.version}`),
    ["@office-kit/xlsx@0.21.1", "fflate@0.8.3", "saxes@6.0.0", "@nodable/entities@3.0.0"],
  );
  const notice = finalizeBrowserOpenSourceNotice(mergeWorkerLicenses("[]", entries), repoRoot, manifestPath);
  for (const heading of [
    "## @nodable/entities - 3.0.0 (MIT)",
    "## @office-kit/xlsx - 0.21.1 (MIT)",
    "## fflate - 0.8.3 (MIT)",
    "## saxes - 6.0.0 (ISC)",
  ]) {
    assert.ok(notice.includes(heading), heading);
  }
  // saxes and @nodable/entities ship no LICENSE file; the manifest supplements fill them.
  assert.equal(notice.match(/^Supplement source: /gm)?.length, 2);
});
