/**
 * pdf.js runtime data that the worker fetches by directory URL (`cMapUrl`,
 * `standardFontDataUrl`, `wasmUrl`, `iccUrl`). The build copies these files
 * unchanged from the installed pdfjs-dist into a versioned same-origin
 * directory; the viewer points pdf.js at it. Shared by vite.config.ts.
 */
export const PDFJS_ASSET_DIRS = ["cmaps", "standard_fonts", "wasm", "iccs"] as const;

export type PdfjsAssetDir = (typeof PDFJS_ASSET_DIRS)[number];

/**
 * Which files of a pdfjs-dist data directory are shipped. License files
 * travel with the data they cover. `quickjs-eval.*` (PDF JavaScript
 * scripting) is left out: the viewer never runs document scripts.
 */
export function isPdfjsAsset(dir: PdfjsAssetDir, name: string): boolean {
  if (/^LICENSE/.test(name)) return true;
  switch (dir) {
    case "cmaps":
      return name.endsWith(".bcmap");
    case "standard_fonts":
      return /\.(pfb|ttf)$/.test(name);
    case "wasm":
      return /^(jbig2|openjpeg|qcms_bg)\.wasm$/.test(name) || /^(jbig2|openjpeg)_nowasm_fallback\.js$/.test(name);
    case "iccs":
      return name.endsWith(".icc");
  }
}

/** Build-relative directory (no leading slash, trailing slash) for one pdfjs-dist version. */
export function pdfjsAssetBase(version: string): string {
  if (!/^\d+\.\d+\.\d+$/.test(version)) {
    throw new Error(`unexpected pdfjs-dist version: ${version}`);
  }
  return `assets/pdfjs-dist-${version}/`;
}
