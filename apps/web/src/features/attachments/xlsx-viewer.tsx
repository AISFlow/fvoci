import { t } from "@fvoci/i18n";
import { useEffect, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, readCapped, zoomIn, zoomOut } from "./pdf-limits";
import {
  ViewerDownloadButton,
  ViewerErrorPane,
  ViewerLoadingPane,
  ViewerZoomToolbar,
} from "./viewer-shell";
import { openXlsxInWorker, XlsxWorkerError, type RemoteXlsxBook } from "./xlsx-client";
import { XLSX_MAX_BYTES } from "./xlsx-limits";
import type { XlsxPage } from "./xlsx-workbook";
import "./xlsx-viewer.css";

type BookState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; book: RemoteXlsxBook };

/** The last page the worker returned, and the request it answers. */
type PageState = { sheetIndex: number; rowPage: number; colPage: number; page: XlsxPage | null };

function pageError(error: unknown): BookState {
  const timedOut = error instanceof XlsxWorkerError && error.reason === "timeout";
  return {
    status: "error",
    message: t(timedOut ? "attachment.viewer.previewUnavailable" : "load.failed"),
    retry: !timedOut,
  };
}

/**
 * Worksheet grid of an XLSX attachment (source `XlsxViewer`): sheet, row-page
 * (200) and column-page (64) navigation plus zoom over the cells' display
 * text. A chartsheet or other non-worksheet tab says it cannot be shown and
 * offers the original download; the other tabs stay reachable. The workbook
 * is parsed and paged in a dedicated worker (`xlsx-client.ts`) with a
 * wall-clock bound, and that worker is terminated on unmount or a new load.
 */
export function XlsxViewer({ downloadUrl }: { downloadUrl: string }): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<BookState>({ status: "loading" });
  const [sheetIndex, setSheetIndex] = useState(0);
  const [rowPage, setRowPage] = useState(0);
  const [colPage, setColPage] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [shown, setShown] = useState<PageState | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    setState({ status: "loading" });
    setSheetIndex(0);
    setRowPage(0);
    setColPage(0);
    setShown(null);
    let book: RemoteXlsxBook | null = null;
    void (async () => {
      try {
        const response = await fetch(downloadUrl, {
          credentials: "include",
          signal: controller.signal,
        });
        if (!response.ok) {
          await response.body?.cancel();
          if (alive) setState({ status: "error", message: t("load.failed"), retry: true });
          return;
        }
        const body = await readCapped(response, XLSX_MAX_BYTES);
        if (!alive) return;
        if (body.status === "tooLarge") {
          setState({ status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false });
          return;
        }
        const opened = await openXlsxInWorker(body.bytes, { signal: controller.signal });
        if (opened.status === "ok") book = opened.book;
        if (!alive) {
          book?.close();
          return;
        }
        if (opened.status === "ok") {
          // The first page arrives with the book, so the grid never flashes empty.
          let first: PageState | null = null;
          if (opened.book.sheets[0]?.kind === "worksheet") {
            try {
              first = { sheetIndex: 0, rowPage: 0, colPage: 0, page: await opened.book.page(0, 0, 0) };
            } catch (error) {
              if (alive) setState(pageError(error));
              return;
            }
            if (!alive) return;
          }
          setShown(first);
          setState({ status: "ready", book: opened.book });
        } else if (opened.status === "tooLarge") {
          setState({ status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false });
        } else {
          setState({ status: "error", message: t("load.failed"), retry: true });
        }
      } catch (error) {
        if (!alive || (error instanceof Error && error.name === "AbortError")) return;
        setState({ status: "error", message: t("load.failed"), retry: true });
      }
    })();
    return () => {
      alive = false;
      controller.abort();
      book?.close();
    };
  }, [downloadUrl, generation]);

  const book = state.status === "ready" ? state.book : null;
  const sheet = book?.sheets[sheetIndex];

  const current =
    shown !== null && shown.sheetIndex === sheetIndex && shown.rowPage === rowPage && shown.colPage === colPage;

  useEffect(() => {
    if (!book || sheet?.kind !== "worksheet" || current) return;
    let alive = true;
    book.page(sheetIndex, rowPage, colPage).then(
      (next) => {
        if (alive) setShown({ sheetIndex, rowPage, colPage, page: next });
      },
      (error: unknown) => {
        if (alive) setState(pageError(error));
      },
    );
    return () => {
      alive = false;
    };
  }, [book, sheet, sheetIndex, rowPage, colPage, current]);

  // Until the requested page arrives, the previous page of the same sheet stays on screen.
  const pending = shown?.sheetIndex !== sheetIndex;
  const page = pending ? null : shown.page;

  if (state.status === "loading") return <ViewerLoadingPane />;
  if (state.status === "error") {
    return (
      <ViewerErrorPane
        message={state.message}
        downloadUrl={downloadUrl}
        {...(state.retry ? { onRetry: () => setGeneration((n) => n + 1) } : {})}
      />
    );
  }
  const sheets = state.book.sheets;
  if (sheets.length === 0 || !sheet) {
    return (
      <ViewerErrorPane message={t("attachment.viewer.previewUnavailable")} downloadUrl={downloadUrl} />
    );
  }

  const showSheet = (index: number) => {
    setSheetIndex(index);
    setRowPage(0);
    setColPage(0);
  };

  return (
    <div className="attachment-viewer__pane" data-testid="xlsx-viewer">
      <ViewerZoomToolbar
        zoom={zoom}
        canZoomOut={zoom > PDF_ZOOM_MIN}
        canZoomIn={zoom < PDF_ZOOM_MAX}
        onZoomIn={() => setZoom(zoomIn)}
        onZoomOut={() => setZoom(zoomOut)}
        onReset={() => setZoom(1)}
      >
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={sheetIndex <= 0}
          onClick={() => showSheet(sheetIndex - 1)}
        >
          {t("attachment.viewer.prevSheet")}
        </Button>
        <p className="attachment-viewer__page-label" aria-live="polite">
          {t("attachment.viewer.sheet")}: {sheet.name} ({sheetIndex + 1}/{sheets.length})
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={sheetIndex + 1 >= sheets.length}
          onClick={() => showSheet(sheetIndex + 1)}
        >
          {t("attachment.viewer.nextSheet")}
        </Button>
        {page && page.rowPages > 1 ? (
          <>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={page.rowPage <= 0}
              onClick={() => setRowPage(page.rowPage - 1)}
            >
              {t("attachment.viewer.prevPage")}
            </Button>
            <p className="attachment-viewer__page-label" data-xlsx-row-page="">
              {t("attachment.viewer.page", { current: page.rowPage + 1, total: page.rowPages })}
            </p>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={page.rowPage + 1 >= page.rowPages}
              onClick={() => setRowPage(page.rowPage + 1)}
            >
              {t("attachment.viewer.nextPage")}
            </Button>
          </>
        ) : null}
        {page && page.colPages > 1 ? (
          <>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={page.colPage <= 0}
              onClick={() => setColPage(page.colPage - 1)}
            >
              {t("attachment.viewer.prevColumns")}
            </Button>
            <p className="attachment-viewer__page-label" data-xlsx-col-page="">
              {t("attachment.viewer.columns", { current: page.colPage + 1, total: page.colPages })}
            </p>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={page.colPage + 1 >= page.colPages}
              onClick={() => setColPage(page.colPage + 1)}
            >
              {t("attachment.viewer.nextColumns")}
            </Button>
          </>
        ) : null}
      </ViewerZoomToolbar>
      {sheet.kind === "unsupported" ? (
        <div className="attachment-viewer__pane attachment-viewer__pane--center" data-xlsx-unsupported="">
          <p className="attachment-viewer__status">{t("attachment.viewer.previewUnavailable")}</p>
          <ViewerDownloadButton href={downloadUrl} />
        </div>
      ) : (
        <div className="attachment-viewer__page-wrap attachment-viewer__xlsx-body">
          {pending ? (
            <p className="attachment-viewer__status">{t("attachment.preview.loading")}</p>
          ) : page === null || page.rows.length === 0 ? (
            <p className="attachment-viewer__status">—</p>
          ) : (
            <table className="attachment-viewer__xlsx-table" style={{ zoom }}>
              <tbody>
                {page.rows.map((cells, rowIndex) => (
                  <tr key={page.box.minRow + rowIndex}>
                    {cells.map((text, colIndex) => (
                      <td key={page.box.minCol + colIndex}>{text}</td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}
