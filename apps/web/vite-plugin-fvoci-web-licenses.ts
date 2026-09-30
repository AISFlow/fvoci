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
  return (
    path.basename(parent).startsWith("@") && path.basename(path.dirname(parent)) === "node_modules"
  );
}

type PackageJson = { name?: string; version?: string; license?: string };

/** The package a module belongs to, skipping nested non-root package.json files. */
function mainPackage(from: string): { dir: string; data: PackageJson } | null {
  for (let dir = from; ;) {
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
    const licenseFile = fs
      .readdirSync(pkg.dir)
      .find((file) => LICENSE_FILES.some((re) => re.test(file)));
    if (licenseFile) entry.text = fs.readFileSync(path.join(pkg.dir, licenseFile), "utf8").trim();
    entries.set(licenseEntryId(entry), entry);
  }
  return [...entries.values()];
}

/**
 * Vite's license JSON plus entries it cannot see (worker-only packages,
 * bundled icon sets), sorted by `name@version` as Vite sorts.
 */
export function mergeLicenseEntries(licenseJson: string, extraEntries: LicenseEntry[]): string {
  const merged = new Map(
    parseLicenseJson(licenseJson).map((entry) => [licenseEntryId(entry), entry]),
  );
  for (const entry of extraEntries) {
    if (!merged.has(licenseEntryId(entry))) merged.set(licenseEntryId(entry), entry);
  }
  return JSON.stringify([...merged.keys()].sort().map((id) => merged.get(id)));
}

/**
 * The virtual module Nuxt UI's icon plugin generates (icon.clientBundle): the
 * data of every bundled icon, copied out of the installed @iconify-json/*
 * sets. Being virtual, neither Vite's build.license nor
 * {@link packageLicenseEntries} sees those packages.
 */
export const NUXT_UI_ICONS_MODULE_ID = "virtual:nuxt-ui-icons";

/**
 * The icon set prefixes an icon module inlines. @nuxt/icon's
 * generateClientBundleCode writes either `export function init() {}` (no
 * icons) or `const collections = JSON.parse("<[{prefix, icons}] as JSON>")`.
 * Any other shape throws, so a changed generator cannot drop icon sets from
 * the notice unnoticed.
 */
export function bundledIconPrefixes(code: string): string[] {
  const literals = [...code.matchAll(/JSON\.parse\(("(?:[^"\\]|\\.)*")\)/g)];
  if (literals.length === 0) {
    if (/^\s*export\s+function\s+init\s*\(\s*\)\s*\{\s*\}\s*;?\s*$/.test(code)) return [];
    throw new Error(`${NUXT_UI_ICONS_MODULE_ID}: unrecognised module; update bundledIconPrefixes`);
  }
  const prefixes = new Set<string>();
  for (const [, literal] of literals) {
    const collections: unknown = JSON.parse(JSON.parse(literal!) as string);
    if (!Array.isArray(collections)) {
      throw new Error(`${NUXT_UI_ICONS_MODULE_ID}: icon collections are not an array`);
    }
    for (const collection of collections) {
      const prefix = (collection as { prefix?: unknown } | null)?.prefix;
      if (typeof prefix !== "string" || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(prefix)) {
        throw new Error(`${NUXT_UI_ICONS_MODULE_ID}: icon collection without a valid prefix`);
      }
      prefixes.add(prefix);
    }
  }
  return [...prefixes].sort();
}

/**
 * License entries for bundled icon sets, from the installed
 * `@iconify-json/<prefix>` package the icon plugin read them from (the first
 * node_modules up from `resolveFrom`, the Vite root). These packages ship no
 * LICENSE file, so each one needs a pinned supplement in the browser license
 * manifest, or the notice check fails the build.
 */
export function iconSetLicenseEntries(
  prefixes: Iterable<string>,
  resolveFrom: string,
): LicenseEntry[] {
  const entries: LicenseEntry[] = [];
  for (const prefix of prefixes) {
    const name = `@iconify-json/${prefix}`;
    let dir: string | undefined;
    for (let at = resolveFrom; ; at = path.dirname(at)) {
      const candidate = path.join(at, "node_modules", name);
      if (fs.existsSync(path.join(candidate, "package.json"))) {
        dir = candidate;
        break;
      }
      if (path.dirname(at) === at) break;
    }
    if (!dir) {
      throw new Error(
        `icon set "${prefix}" is bundled but ${name} is not installed; install it so its license is known`,
      );
    }
    const data = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8")) as PackageJson;
    const entry: LicenseEntry = { name, version: data.version ?? "0.0.0" };
    if (data.license) entry.identifier = data.license.trim();
    const licenseFile = fs
      .readdirSync(dir)
      .find((file) => LICENSE_FILES.some((re) => re.test(file)));
    if (licenseFile) entry.text = fs.readFileSync(path.join(dir, licenseFile), "utf8").trim();
    entries.push(entry);
  }
  return entries;
}

export function fvociWebLicenseAdapt(options: {
  repoRoot: string;
  manifestPath: string;
  /** Notices for files copied into the build outside the module graph. */
  assetNotices?: () => BundledSourceNotice[];
  /** Module ids bundled into web workers (see {@link collectWorkerModuleIds}). */
  workerModuleIds?: () => Iterable<string>;
  /** Where bundled icon sets are resolved from (the Vite root). */
  iconSetRoot: string;
}): Plugin {
  // Code of the icon module as transformed, and whether any chunk kept it.
  let iconModuleCode: string | undefined;
  let iconPrefixes: string[] = [];
  return {
    name: "fvoci-web-license-adapt",
    apply: "build",
    enforce: "post",
    transform(code, id) {
      if (id === NUXT_UI_ICONS_MODULE_ID) iconModuleCode = code;
      return null;
    },
    generateBundle(_outputOptions, bundle) {
      const bundled = Object.values(bundle).some(
        (output) => output.type === "chunk" && output.moduleIds.includes(NUXT_UI_ICONS_MODULE_ID),
      );
      if (!bundled) {
        iconPrefixes = [];
        return;
      }
      if (iconModuleCode === undefined) {
        throw new Error(`${NUXT_UI_ICONS_MODULE_ID} is bundled but its code was not seen`);
      }
      iconPrefixes = bundledIconPrefixes(iconModuleCode);
    },
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
        const licenseJson = mergeLicenseEntries(fs.readFileSync(dataPath, "utf8"), [
          ...packageLicenseEntries(options.workerModuleIds?.() ?? []),
          ...iconSetLicenseEntries(iconPrefixes, options.iconSetRoot),
        ]);
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
