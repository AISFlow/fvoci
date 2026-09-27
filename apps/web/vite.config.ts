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
const pdfjsFiles = PDFJS_ASSET_DIRS.flatMap((dir) =>
  fs
    .readdirSync(path.join(pdfjsDir, dir))
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

const apiProxyTarget =
  process.env.API_PROXY_TARGET ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    fvociWebLicenseAdapt({
      repoRoot,
      manifestPath: browserLicenseManifest,
      assetNotices: pdfjsAssetNotices,
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
  build: {
    outDir: "dist",
    emptyOutDir: true,
    license: { fileName: VITE_LICENSE_DATA_FILE },
  },
});
