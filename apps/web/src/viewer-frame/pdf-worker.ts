/**
 * pdf.js's worker, built as a classic script whose text pdf.ts carries and
 * starts from a `blob:` (`?worker&inline`). Loaded in a worker, the module
 * sets itself up to answer pdf.js.
 */
import 'pdfjs-dist/build/pdf.worker.mjs';
