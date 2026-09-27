import type { HwpExportFormat } from "./hwp-edit.ts";
import { clampPage, decodePageText, HWP_MAX_BYTES, pageOfChunk, visiblePageCount } from "./hwp-page.ts";
import { prepareHwpBytes } from "./hwp-package.ts";
import { parseRhwpMutation, rhwpMutationChanged } from "./rhwp-mutation.ts";

/** The requests one document worker answers, in order. */
export type HwpRequest =
  | { id: number; op: "open"; bytes: Uint8Array; module: WebAssembly.Module }
  | { id: number; op: "startPage"; chunk: number }
  | { id: number; op: "render"; page: number }
  | { id: number; op: "replace"; find: string; replacement: string; all: boolean }
  | { id: number; op: "revert" }
  | { id: number; op: "export"; format: HwpExportFormat };

/**
 * `tooLarge`/`invalid`: rhwp will not lay this file out (no retry helps);
 * `failed`: the worker or wasm failed and a new worker may succeed.
 */
export type HwpFailure = "tooLarge" | "invalid" | "failed";

/**
 * `changed`: the document was edited; `unchanged`: rhwp replaced nothing (a
 * `replaceAll` count of 0); `rejected`: rhwp refused or answered something
 * unreadable, which also leaves the document as it was.
 */
export type HwpReplaceOutcome = "changed" | "unchanged" | "rejected";

export type HwpResponse =
  | { id: number; ok: true; op: "open"; pageCount: number }
  | { id: number; ok: true; op: "startPage"; page: number }
  | { id: number; ok: true; op: "render"; svg: Blob }
  | { id: number; ok: true; op: "replace"; outcome: HwpReplaceOutcome; pageCount: number }
  | { id: number; ok: true; op: "revert"; pageCount: number }
  | { id: number; ok: true; op: "export"; bytes: Uint8Array }
  | { id: number; ok: false; error: HwpFailure };

/** The part of `@rhwp/core` the worker uses. */
export type RhwpDocument = {
  pageCount(): number;
  getPageText(page: number): string;
  renderPageSvg(page: number): string;
  replaceOne(query: string, replacement: string, caseSensitive: boolean): string;
  replaceAll(query: string, replacement: string, caseSensitive: boolean): string;
  exportHwp(): Uint8Array;
  exportHwpx(): Uint8Array;
  free(): void;
};

export type RhwpApi = {
  init(module: WebAssembly.Module): Promise<void>;
  open(bytes: Uint8Array): RhwpDocument;
};

/**
 * One document's request handler (the worker body, kept free of worker
 * globals so it runs under Node with the real wasm). The document lives until
 * the worker is terminated, which is what releases rhwp's memory.
 *
 * Edits (source `HwpViewer` 간단 편집) change only this in-memory document:
 * `replace` runs rhwp's case-sensitive find/replace, `revert` re-parses the
 * checked original bytes kept here, and `export` writes the current document
 * as HWP or HWPX. An export larger than the viewer's own download budget is
 * refused, since the copy could not be opened again.
 */
export function createHwpSession(api: RhwpApi): (request: HwpRequest) => Promise<HwpResponse> {
  let doc: RhwpDocument | null = null;
  let original: Uint8Array | null = null;
  let pageCount = 1;
  return async (request) => {
    const { id } = request;
    if (request.op === "open") {
      if (doc) return { id, ok: false, error: "failed" };
      const checked = await prepareHwpBytes(request.bytes, () => true);
      if (checked.status !== "ok") return { id, ok: false, error: checked.status };
      try {
        await api.init(request.module);
      } catch {
        return { id, ok: false, error: "failed" };
      }
      try {
        doc = api.open(checked.bytes);
        pageCount = visiblePageCount(doc.pageCount());
      } catch {
        // Not a document rhwp can lay out (including a wasm trap).
        return { id, ok: false, error: "invalid" };
      }
      original = checked.bytes;
      return { id, ok: true, op: "open", pageCount };
    }
    if (!doc) return { id, ok: false, error: "failed" };
    if (request.op === "startPage") {
      let page = 0;
      try {
        const pages = Array.from({ length: pageCount }, (_, index) => decodePageText(doc!.getPageText(index)));
        page = clampPage(pageOfChunk(pages, request.chunk), pageCount);
      } catch {
        // No page text: open the first page.
      }
      return { id, ok: true, op: "startPage", page };
    }
    if (request.op === "replace") {
      if (request.find.length === 0) return { id, ok: true, op: "replace", outcome: "unchanged", pageCount };
      try {
        const raw = request.all
          ? doc.replaceAll(request.find, request.replacement, false)
          : doc.replaceOne(request.find, request.replacement, false);
        const result = parseRhwpMutation(raw);
        if (!result.ok) return { id, ok: true, op: "replace", outcome: "rejected", pageCount };
        if (!rhwpMutationChanged(result)) return { id, ok: true, op: "replace", outcome: "unchanged", pageCount };
        pageCount = visiblePageCount(doc.pageCount());
        return { id, ok: true, op: "replace", outcome: "changed", pageCount };
      } catch {
        return { id, ok: false, error: "failed" };
      }
    }
    if (request.op === "revert") {
      if (!original) return { id, ok: false, error: "failed" };
      let fresh: RhwpDocument;
      try {
        fresh = api.open(original);
        pageCount = visiblePageCount(fresh.pageCount());
      } catch {
        return { id, ok: false, error: "failed" };
      }
      doc.free();
      doc = fresh;
      return { id, ok: true, op: "revert", pageCount };
    }
    if (request.op === "export") {
      let bytes: Uint8Array;
      try {
        bytes = request.format === "hwpx" ? doc.exportHwpx() : doc.exportHwp();
      } catch {
        return { id, ok: false, error: "failed" };
      }
      if (bytes.byteLength === 0) return { id, ok: false, error: "failed" };
      if (bytes.byteLength > HWP_MAX_BYTES) return { id, ok: false, error: "tooLarge" };
      return { id, ok: true, op: "export", bytes };
    }
    try {
      const svg = doc.renderPageSvg(clampPage(request.page, pageCount));
      return { id, ok: true, op: "render", svg: new Blob([svg], { type: "image/svg+xml" }) };
    } catch {
      return { id, ok: false, error: "failed" };
    }
  };
}
