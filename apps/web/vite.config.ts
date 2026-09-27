import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import {
  fvociWebLicenseAdapt,
  VITE_LICENSE_DATA_FILE,
} from "./vite-plugin-fvoci-web-licenses.ts";

const repoRoot = path.resolve(import.meta.dirname, "../..");
const browserLicenseManifest = path.join(
  repoRoot,
  "third-party/browser-licenses/manifest.json",
);

const apiProxyTarget =
  process.env.API_PROXY_TARGET ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    fvociWebLicenseAdapt({ repoRoot, manifestPath: browserLicenseManifest }),
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
