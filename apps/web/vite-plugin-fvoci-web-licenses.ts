import fs from "node:fs";
import path from "node:path";
import type { Plugin } from "vite";
import {
  type BundledSourceNotice,
  finalizeBrowserOpenSourceNotice,
  type LicenseEntry,
  licenseEntryId,
  parseLicenseJson,
} from "./src/build/web-licenses.ts";

export const VITE_LICENSE_DATA_FILE = "open-source-license-data.json";
export const PUBLIC_LICENSE_FILE = "open-source-licenses.txt";

/**
 * Worker plugin (`worker.plugins`): records the module ids of every chunk of
 * every web-worker bundle. Vite's `build.license` runs only on the main
 * bundle, where a worker is an opaque asset, so its dependencies would be
 * missing from the notice.
 */
export function collectWorkerModuleIds(into: Set<string>): Plugin {
  return {
    name: "fvoci-worker-module-ids",
    apply: "build",
    generateBundle(_options, bundle) {
      for (const output of Object.values(bundle)) {
        if (output.type === "chunk") for (const id of output.moduleIds) into.add(id);
      }
    },
  };
}

const IN_NODE_MODULES = /[/\\]node_modules[/\\]/;
const LICENSE_FILES = [/^license/i, /^licence/i, /^copying/i];

function isNodeModulesPackageRoot(dir: string): boolean {
  const parent = path.dirname(dir);
  if (path.basename(parent) === "node_modules") return !path.basename(dir).startsWith("@");
  return path.basename(parent).startsWith("@") && path.basename(path.dirname(parent)) === "node_modules";
}

type PackageJson = { name?: string; version?: string; license?: string };

/** The package a module belongs to, skipping nested non-root package.json files. */
function mainPackage(from: string): { dir: string; data: PackageJson } | null {
  for (let dir = from; ; ) {
    const file = path.join(dir, "package.json");
    if (fs.existsSync(file)) {
      const data = JSON.parse(fs.readFileSync(file, "utf8")) as PackageJson;
      const nested = IN_NODE_MODULES.test(dir) && !isNodeModulesPackageRoot(dir);
      if (!nested && data.name) return { dir, data };
    }
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

/**
 * License entries for the node_modules packages of `moduleIds`, built the way
 * Vite's `build.license` builds its JSON (package name, version, `license`
 * field and the text of its LICENSE/LICENCE/COPYING file).
 */
export function packageLicenseEntries(moduleIds: Iterable<string>): LicenseEntry[] {
  const entries = new Map<string, LicenseEntry>();
  for (const id of moduleIds) {
    if (id.startsWith("\0") || !IN_NODE_MODULES.test(id)) continue;
    const pkg = mainPackage(path.dirname(id));
    if (!pkg?.data.name) continue;
    const entry: LicenseEntry = { name: pkg.data.name, version: pkg.data.version ?? "0.0.0" };
    if (entries.has(licenseEntryId(entry))) continue;
    if (pkg.data.license) entry.identifier = pkg.data.license.trim();
    const licenseFile = fs.readdirSync(pkg.dir).find((file) => LICENSE_FILES.some((re) => re.test(file)));
    if (licenseFile) entry.text = fs.readFileSync(path.join(pkg.dir, licenseFile), "utf8").trim();
    entries.set(licenseEntryId(entry), entry);
  }
  return [...entries.values()];
}

/** Vite's license JSON plus the worker-only packages, sorted by `name@version` as Vite sorts. */
export function mergeWorkerLicenses(licenseJson: string, workerEntries: LicenseEntry[]): string {
  const merged = new Map(parseLicenseJson(licenseJson).map((entry) => [licenseEntryId(entry), entry]));
  for (const entry of workerEntries) {
    if (!merged.has(licenseEntryId(entry))) merged.set(licenseEntryId(entry), entry);
  }
  return JSON.stringify([...merged.keys()].sort().map((id) => merged.get(id)));
}

export function fvociWebLicenseAdapt(options: {
  repoRoot: string;
  manifestPath: string;
  /** Notices for files copied into the build outside the module graph. */
  assetNotices?: () => BundledSourceNotice[];
  /** Module ids bundled into web workers (see {@link collectWorkerModuleIds}). */
  workerModuleIds?: () => Iterable<string>;
}): Plugin {
  return {
    name: "fvoci-web-license-adapt",
    apply: "build",
    enforce: "post",
    transformIndexHtml() {
      return [
        {
          tag: "link",
          attrs: { rel: "license", href: `/${PUBLIC_LICENSE_FILE}` },
          injectTo: "head",
        },
      ];
    },
    writeBundle: {
      order: "post",
      handler(outputOptions) {
        const outDir = outputOptions.dir;
        if (!outDir) {
          throw new Error("fvoci-web-license-adapt requires build.outDir");
        }
        const dataPath = path.join(outDir, VITE_LICENSE_DATA_FILE);
        if (!fs.existsSync(dataPath)) {
          throw new Error(
            `Missing ${VITE_LICENSE_DATA_FILE}; enable build.license with fileName "${VITE_LICENSE_DATA_FILE}"`,
          );
        }
        const licenseJson = mergeWorkerLicenses(
          fs.readFileSync(dataPath, "utf8"),
          packageLicenseEntries(options.workerModuleIds?.() ?? []),
        );
        const publicText = finalizeBrowserOpenSourceNotice(
          licenseJson,
          options.repoRoot,
          options.manifestPath,
          options.assetNotices?.() ?? [],
        );
        fs.writeFileSync(path.join(outDir, PUBLIC_LICENSE_FILE), publicText, "utf8");
        fs.unlinkSync(dataPath);
      },
    },
  };
}
