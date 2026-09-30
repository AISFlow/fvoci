/**
 * Dedicated worker that owns the parsed workbook (`xlsx-client.ts` talks to
 * it). Parsing runs here so a workbook that is slow to inflate or parse never
 * blocks the page; the client terminates this worker on timeout.
 */
import type { XlsxWorkerRequest, XlsxWorkerResponse } from "./xlsx-client.ts";
import { openXlsx, type XlsxBook } from "./xlsx-workbook.ts";

type WorkerScope = {
  onmessage: ((event: MessageEvent<XlsxWorkerRequest>) => void) | null;
  postMessage(message: XlsxWorkerResponse): void;
};

const scope = self as unknown as WorkerScope;
let book: XlsxBook | null = null;

scope.onmessage = ({ data }) => {
  void handle(data).then(
    (response) => {
      scope.postMessage(response);
    },
    () => {
      scope.postMessage({ type: "failed" });
    },
  );
};

async function handle(request: XlsxWorkerRequest): Promise<XlsxWorkerResponse> {
  if (request.type === "open") {
    const opened = await openXlsx(request.bytes);
    if (opened.status !== "ok") return { type: "opened", status: opened.status };
    book = opened.book;
    return { type: "opened", status: "ok", sheets: opened.book.sheets };
  }
  return {
    type: "page",
    id: request.id,
    page: book?.page(request.index, request.rowPage, request.colPage) ?? null,
  };
}
