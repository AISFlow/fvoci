import { clampPage, decodePageText, pageOfChunk, visiblePageCount } from "./hwp-page.ts";
import { prepareHwpBytes } from "./hwp-package.ts";

/** The requests one document worker answers, in order. */
export type HwpRequest =
  | { id: number; op: "open"; bytes: Uint8Array; module: WebAssembly.Module }
  | { id: number; op: "startPage"; chunk: number }
  | { id: number; op: "render"; page: number };

/**
 * `tooLarge`/`invalid`: rhwp will not lay this file out (no retry helps);
 * `failed`: the worker or wasm failed and a new worker may succeed.
 */
export type HwpFailure = "tooLarge" | "invalid" | "failed";

export type HwpResponse =
  | { id: number; ok: true; op: "open"; pageCount: number }
  | { id: number; ok: true; op: "startPage"; page: number }
  | { id: number; ok: true; op: "render"; svg: Blob }
  | { id: number; ok: false; error: HwpFailure };

/** The part of `@rhwp/core` the worker uses. */
export type RhwpDocument = {
  pageCount(): number;
  getPageText(page: number): string;
  renderPageSvg(page: number): string;
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
 */
export function createHwpSession(api: RhwpApi): (request: HwpRequest) => Promise<HwpResponse> {
  let doc: RhwpDocument | null = null;
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
    try {
      const svg = doc.renderPageSvg(clampPage(request.page, pageCount));
      return { id, ok: true, op: "render", svg: new Blob([svg], { type: "image/svg+xml" }) };
    } catch {
      return { id, ok: false, error: "failed" };
    }
  };
}
