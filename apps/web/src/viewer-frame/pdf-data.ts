/**
 * The data files pdf.js asks for while it reads a document, from the bundle
 * instead of the network.
 *
 * pdf.js fetches the character maps of CJK fonts that are not embedded (a
 * Korean PDF made without its fonts needs them to be read at all), and the
 * WebAssembly decoders of JBIG2 and JPEG 2000 images and of ICC colour. The
 * frame may not fetch: each file is built into a module of its own that holds
 * its bytes in Base64, and a module is loaded like the frame's
 * other scripts, only when pdf.js asks for that file (`virtual:pdfjs-data`,
 * tools/build/pdfjs-data.ts).
 *
 * The standard fonts (Helvetica and the like when not embedded) are left out:
 * without them pdf.js draws those fonts with the system's.
 */

import { cMaps, decoders } from 'virtual:pdfjs-data';

type Loader = () => Promise<string>;

/** The loaders by the kind pdf.js names (the option the file's address would be given by). */
const FILES: Record<string, Readonly<Record<string, Loader>>> = {
  cMapUrl: cMaps,
  wasmUrl: decoders,
};

/** The bytes of Base64 text. */
function decode(base64: string): Uint8Array {
  const text = atob(base64);
  const bytes = new Uint8Array(text.length);
  for (let index = 0; index < text.length; index += 1) bytes[index] = text.charCodeAt(index);
  return bytes;
}

/**
 * Takes the place of pdf.js's `DOMBinaryDataFactory` (the `BinaryDataFactory`
 * option of `getDocument`). The addresses given to `getDocument` for these
 * kinds are only names here: what counts is the file name pdf.js asks for.
 */
export class BundledData {
  async fetch({ kind, filename }: { kind: string; filename: string }): Promise<Uint8Array> {
    const files = FILES[kind];
    const load = files && Object.hasOwn(files, filename) ? files[filename] : undefined;
    if (!load) throw new Error(`no bundled ${kind} file ${filename}`);
    return decode(await load());
  }
}

/** The options of `getDocument` that send pdf.js's requests for data to `BundledData`. */
export const BUNDLED_DATA_OPTIONS = {
  BinaryDataFactory: BundledData,
  useWorkerFetch: false,
  cMapUrl: 'bundled/',
  cMapPacked: true,
  wasmUrl: 'bundled/',
} as const;
