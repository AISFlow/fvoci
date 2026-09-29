import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";
import {
  isPdfjsAsset,
  PDFJS_ASSET_DIRS,
  pdfjsAssetBase,
} from "./src/features/attachments/pdf-assets.ts";
import {
  RHWP_THIRD_PARTY_FILE,
  rhwpWasmNoticeTitle,
} from "./src/features/attachments/rhwp-notice.ts";
import {
  collectWorkerModuleIds,
  fvociWebLicenseAdapt,
  VITE_LICENSE_DATA_FILE,
} from "./vite-plugin-fvoci-web-licenses.ts";

const repoRoot = path.resolve(import.meta.dirname, "../..");
const browserLicenseManifest = path.join(
  repoRoot,
  "third-party/browser-licenses/manifest.json",
);

const pdfjsDir = path.dirname(
  createRequire(import.meta.url).resolve("pdfjs-dist/package.json"),
);
const pdfjsVersion = (
  JSON.parse(fs.readFileSync(path.join(pdfjsDir, "package.json"), "utf8")) as {
    version: string;
  }
).version;
const pdfjsBase = pdfjsAssetBase(pdfjsVersion);
// Sorted: readdir order varies by filesystem and installer, and the license
// notices built from this list go into the public notice in this order.
const pdfjsFiles = PDFJS_ASSET_DIRS.flatMap((dir) =>
  fs
    .readdirSync(path.join(pdfjsDir, dir))
    .sort()
    .filter((name) => isPdfjsAsset(dir, name))
    .map((name) => `${dir}/${name}`),
);

/** License texts shipped with the copied pdf.js data, for the public notice. */
function pdfjsAssetNotices() {
  return pdfjsFiles
    .filter((rel) => /\/LICENSE/.test(rel))
    .map((rel) => ({
      title: `pdfjs-dist ${pdfjsVersion} runtime data: ${rel}`,
      text: fs.readFileSync(path.join(pdfjsDir, rel), "utf8").trim(),
    }));
}

/**
 * Serves (dev) and emits (build) the installed pdfjs-dist cmaps, standard
 * fonts, wasm decoders and ICC profile under a versioned same-origin path.
 */
function pdfjsAssets(): Plugin {
  const shipped = new Set(pdfjsFiles);
  return {
    name: "fvoci-pdfjs-assets",
    configureServer(server) {
      server.middlewares.use(`/${pdfjsBase}`, (req, res, next) => {
        let rel: string;
        try {
          rel = decodeURIComponent((req.url ?? "").split("?")[0]!.replace(/^\//, ""));
        } catch {
          return next();
        }
        if (!shipped.has(rel)) return next();
        res.setHeader("content-type", rel.endsWith(".wasm") ? "application/wasm" : rel.endsWith(".js") ? "text/javascript" : "application/octet-stream");
        res.end(fs.readFileSync(path.join(pdfjsDir, rel)));
      });
    },
    generateBundle() {
      for (const rel of pdfjsFiles) {
        this.emitFile({
          type: "asset",
          fileName: `${pdfjsBase}${rel}`,
          source: fs.readFileSync(path.join(pdfjsDir, rel)),
        });
      }
    },
  };
}

/** Pinned full license texts for the Rust dependencies of @rhwp/core's wasm. */
function rhwpWasmNotice() {
  const coreDir = path.dirname(createRequire(import.meta.url).resolve("@rhwp/core"));
  const { version } = JSON.parse(
    fs.readFileSync(path.join(coreDir, "package.json"), "utf8"),
  ) as { version: string };
  return {
    title: rhwpWasmNoticeTitle(version),
    text: fs
      .readFileSync(
        path.join(repoRoot, "third-party/browser-licenses", RHWP_THIRD_PARTY_FILE),
        "utf8",
      )
      .trim(),
  };
}

/**
 * `@office-kit/xlsx` ships THIRD_PARTY_NOTICES.md (the openpyxl MIT notice
 * for its derived code) next to its LICENSE; build.license takes only the
 * LICENSE, so the sidecar is added here.
 */
function officeKitXlsxNotice() {
  const dir = installedPackageDir("@office-kit/xlsx");
  const { version } = JSON.parse(
    fs.readFileSync(path.join(dir, "package.json"), "utf8"),
  ) as { version: string };
  return {
    title: `@office-kit/xlsx ${version}: THIRD_PARTY_NOTICES.md`,
    text: fs.readFileSync(path.join(dir, "THIRD_PARTY_NOTICES.md"), "utf8").trim(),
  };
}

/**
 * The directory `name` is installed in for this app: the first
 * node_modules/`name` up from apps/web, where the bundle resolves it from. For
 * packages that export no ./package.json (createRequire cannot resolve them);
 * import.meta.resolve is avoided because Vite 8.3's default config loader
 * serves it through Node module hooks that Bun 1.4 lacks (oven-sh/bun#27369).
 */
function installedPackageDir(name: string): string {
  for (let dir = import.meta.dirname; ; dir = path.dirname(dir)) {
    const candidate = path.join(dir, "node_modules", name);
    if (fs.existsSync(path.join(candidate, "package.json"))) return candidate;
    if (path.dirname(dir) === dir) throw new Error(`${name} is not installed`);
  }
}

/** Modules of every web-worker bundle, for the license notice. */
const workerModuleIds = new Set<string>();

const apiProxyTarget =
  process.env.API_PROXY_TARGET ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    fvociWebLicenseAdapt({
      repoRoot,
      manifestPath: browserLicenseManifest,
      assetNotices: () => [...pdfjsAssetNotices(), officeKitXlsxNotice(), rhwpWasmNotice()],
      workerModuleIds: () => workerModuleIds,
    }),
    pdfjsAssets(),
  ],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
    },
    dedupe: [
      "react",
      "react-dom",
      "yjs",
      "@hocuspocus/provider",
      "@hocuspocus/provider-react",
    ],
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": apiProxyTarget,
      "/collab": {
        target: apiProxyTarget,
        ws: true,
      },
    },
  },
  worker: {
    plugins: () => [collectWorkerModuleIds(workerModuleIds)],
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    license: { fileName: VITE_LICENSE_DATA_FILE },
  },
});
