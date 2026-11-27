import { blake3 } from '@noble/hashes/blake3.js';
import { bytesToHex } from '@noble/hashes/utils.js';
// [fvoci] The PDF import is not built into this editor: rhwp's import session and its worker
// (rhwp-shared/pdf) are not brought into the repository, and `preparePdfForEditor` below
// refuses. The types it named are kept as the shapes this file uses.
type PdfImportOptions = Record<string, unknown>;
type PdfPageProgress = Record<string, unknown>;

export interface PdfImportPolicy { backend: { inputBytes: number }; [key: string]: unknown }

export interface PdfInputSource {
  byteLength: number;
  read(): Promise<Uint8Array>;
}

export interface PreparedPdfDocument {
  input: Uint8Array;
  editable: Uint8Array;
  diagnostics: Record<string, unknown>;
  pageCount: number;
  deadline: number;
  reopen: Readonly<{ inputBytes: number; decodedBytes: number; options: string }>;
}

export function isPdfInput(data: Uint8Array, fileName: string): boolean {
  return /\.pdf$/i.test(fileName)
    || new TextDecoder('ascii').decode(data.subarray(0, 1024)).includes('%PDF-');
}

/** [fvoci] Refuses: this editor opens no PDF (see the top of the file). */
export async function preparePdfForEditor(
  _source: PdfInputSource,
  _options: PdfImportOptions,
  _policy: { backend: { inputBytes: number } },
  _request: { signal: AbortSignal; timeoutMs: number; onProgress?: (phase: string, progress?: PdfPageProgress) => void },
): Promise<PreparedPdfDocument> {
  throw Object.assign(new Error('PDF_IMPORT_DISABLED'), { code: 'PDF_IMPORT_DISABLED' });
}

/** A candidate owns all resources until the single synchronous publication. */
export async function publishPreparedPdf<T>(
  result: PreparedPdfDocument, signal: AbortSignal, isCurrent: () => boolean,
  lifecycle: { create(timeoutMs: number): T; prepare(candidate: T): void;
    publish(candidate: T, digest: string): void; destroy(candidate: T): void },
): Promise<void> {
  const check = () => {
    const code = signal.aborted ? 'PDF_IMPORT_CANCELLED' : !isCurrent() ? 'PDF_IMPORT_SUPERSEDED'
      : performance.now() >= result.deadline ? 'PDF_IMPORT_TIMEOUT' : null;
    if (code) throw Object.assign(new Error(code), { code });
  };
  const yieldOwner = () => new Promise<void>(resolve => setTimeout(resolve, 0));
  let candidate: T | undefined, owned = false;
  try {
    check();
    candidate = lifecycle.create(result.deadline - performance.now()); owned = true;
    check(); lifecycle.prepare(candidate); check();
    const hash = blake3.create();
    for (let offset = 0; offset < result.input.byteLength; offset += 4 * 1024 * 1024) {
      check(); hash.update(result.input.subarray(offset, offset + 4 * 1024 * 1024));
      if (offset > 0) { await yieldOwner(); check(); }
    }
    const digest = `blake3:${bytesToHex(hash.digest())}`;
    // Queued cancel/timeout/another open must be observed after synchronous WASM.
    await yieldOwner(); check();
    lifecycle.publish(candidate, digest); owned = false;
  } finally {
    if (owned) lifecycle.destroy(candidate!);
  }
}
