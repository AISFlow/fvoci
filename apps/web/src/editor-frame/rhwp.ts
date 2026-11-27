/**
 * rhwp, the engine, as the editor (rhwp-studio) takes it: the build puts this
 * module where the editor imports rhwp's loader (`@wasm/rhwp.js`,
 * vite.config.ts).
 *
 * The editor would start the engine by fetching its WebAssembly from beside
 * the page; the frame fetches nothing, so the loader is given the bytes the
 * app handed over with the document (`provide`, called before the editor is
 * started), the same bytes the viewer's HWP renderer starts from, their
 * memory held to the same limit.
 *
 * rhwp 0.9.0 can turn a PDF into a document. That is not used here: its
 * entry points are taken off the engine before the editor sees it, so the
 * editor's own check finds no PDF import (`supportsPdfImport`) and refuses a
 * PDF, and nothing left in the editor can call them.
 */
import init, * as rhwp from '@rhwp/core';

export * from '@rhwp/core';

/** The entry points of the PDF import on the document's class. */
export const PDF_IMPORT_STATICS = ['importPdf', 'supportsPdfImport'] as const;

for (const name of PDF_IMPORT_STATICS) Reflect.deleteProperty(rhwp.HwpDocument, name);

/** The classes of the PDF import: none here (these names hide the engine's). */
export const PdfEditorReopen = undefined;
export const PdfImportRequest = undefined;

let bytes: ArrayBuffer | undefined;
let started: ReturnType<typeof init> | undefined;

/** Gives the loader the engine's WebAssembly, before the editor asks it to start. */
export function provide(engine: ArrayBuffer): void {
  bytes = engine;
}

/** Starts the engine from the bytes provided, once. */
export default function start(): ReturnType<typeof init> {
  if (!started) {
    if (!bytes) throw new Error('rhwp: the WebAssembly was not provided');
    started = init({ module_or_path: bytes });
    bytes = undefined;
  }
  return started;
}
