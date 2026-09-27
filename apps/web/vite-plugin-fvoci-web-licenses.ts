import fs from "node:fs";
import path from "node:path";
import type { Plugin } from "vite";
import { finalizeBrowserOpenSourceNotice } from "./src/build/web-licenses.ts";

export const VITE_LICENSE_DATA_FILE = "open-source-license-data.json";
export const PUBLIC_LICENSE_FILE = "open-source-licenses.txt";

export function fvociWebLicenseAdapt(options: {
  repoRoot: string;
  manifestPath: string;
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
        const licenseJson = fs.readFileSync(dataPath, "utf8");
        const publicText = finalizeBrowserOpenSourceNotice(
          licenseJson,
          options.repoRoot,
          options.manifestPath,
        );
        fs.writeFileSync(path.join(outDir, PUBLIC_LICENSE_FILE), publicText, "utf8");
        fs.unlinkSync(dataPath);
      },
    },
  };
}
