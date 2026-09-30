type PdfJs = typeof import("pdfjs-dist");

let pdfJs: Promise<PdfJs> | null = null;

/**
 * pdf.js and its worker load only when a PDF is opened. The worker and the
 * CMap/standard-font/wasm/ICC data it fetches are same-origin build assets.
 * A failed load is not cached, so a retry loads again.
 */
export function loadPdfJs(): Promise<PdfJs> {
  pdfJs ??= Promise.all([import("pdfjs-dist"), import("pdfjs-dist/build/pdf.worker.min.mjs?url")]).then(
    ([mod, worker]) => {
      mod.GlobalWorkerOptions.workerSrc = worker.default;
      return mod;
    },
  );
  pdfJs.catch(() => {
    pdfJs = null;
  });
  return pdfJs;
}
