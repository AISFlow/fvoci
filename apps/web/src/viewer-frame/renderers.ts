/**
 * The renderer of each kind of file, fetched the first time a file of that
 * kind is opened. Each is a chunk of its own with its library, so opening a
 * PDF fetches none of the other renderers.
 *
 * Adding a kind: its line in `FORMATS` of `$lib/file-viewer/formats.ts` and
 * its extensions in `$lib/file-viewer/extensions.ts`, a module here that
 * makes its `Render`, its line in this table, and its line in
 * `CHECKED_IN_WORKER` if its worker checks the file itself.
 */
import type { FormatId } from '$lib/file-viewer/formats.ts';

import type { Render } from './renderer.ts';

/** `engine`: rhwp's WebAssembly, which comes with the formats it reads (the app fetches it). */
export const RENDERERS: Record<FormatId, (engine: ArrayBuffer | null) => Promise<Render>> = {
  pdf: async () => (await import('./pdf.ts')).render,
  docx: async () => (await import('./docx.ts')).render,
  xlsx: async () => (await import('./xlsx.ts')).render,
  pptx: async () => (await import('./pptx.ts')).render,
  hwpx: async (engine) => (await import('./hwp.ts')).renderer('hwpx', engine),
  hwp: async (engine) => (await import('./hwp.ts')).renderer('hwp', engine),
};

/**
 * The kinds whose renderer runs in a worker of its own and checks the file
 * there (an archive unpacked and counted, a compound file's chains): the
 * frame hands it the bytes as they came.
 */
export const CHECKED_IN_WORKER: ReadonlySet<FormatId> = new Set(['pptx', 'hwpx', 'hwp']);
