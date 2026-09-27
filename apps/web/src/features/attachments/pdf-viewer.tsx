import { t } from "@fvoci/i18n";
import type { PDFDocumentLoadingTask, PDFDocumentProxy, RenderTask } from "pdfjs-dist";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import {
  PDF_MAX_BYTES,
  PDF_MAX_IMAGE_PIXELS,
  PDF_ZOOM_MAX,
  PDF_ZOOM_MIN,
  readCapped,
  renderScale,
  zoomIn,
  zoomOut,
} from "./pdf-limits";
import {
  ViewerErrorPane,
  ViewerLoadingPane,
  ViewerZoomToolbar,
} from "./viewer-shell";

type PdfJs = typeof import("pdfjs-dist");

let pdfJs: Promise<PdfJs> | null = null;

/** pdf.js and its worker load only when a PDF is opened; the worker is a same-origin asset. */
function loadPdfJs(): Promise<PdfJs> {
  pdfJs ??= Promise.all([
    import("pdfjs-dist"),
    import("pdfjs-dist/build/pdf.worker.min.mjs?url"),
  ]).then(([mod, worker]) => {
    mod.GlobalWorkerOptions.workerSrc = worker.default;
    return mod;
  });
  pdfJs.catch(() => {
    pdfJs = null;
  });
  return pdfJs;
}

type DocState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; doc: PDFDocumentProxy };

/**
 * Renders one page at a time onto a canvas (source `PdfViewer`): page and
 * zoom controls only. No text layer, annotation layer, links, forms or
 * document scripts — pdf.js core never runs PDF JavaScript and nothing here
 * navigates to URLs from the document.
 */
export function PdfViewer({ downloadUrl }: { downloadUrl: string }): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<DocState>({ status: "loading" });
  const [page, setPage] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [renderFailed, setRenderFailed] = useState(false);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    let task: PDFDocumentLoadingTask | null = null;
    setState({ status: "loading" });
    setPage(1);
    setRenderFailed(false);
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
        const body = await readCapped(response, PDF_MAX_BYTES);
        if (!alive) return;
        if (body.status === "tooLarge") {
          setState({
            status: "error",
            message: t("attachment.viewer.previewUnavailable"),
            retry: false,
          });
          return;
        }
        const pdfjs = await loadPdfJs();
        if (!alive) return;
        task = pdfjs.getDocument({
          data: body.bytes,
          enableXfa: false,
          maxImageSize: PDF_MAX_IMAGE_PIXELS,
        });
        const doc = await task.promise;
        if (alive) setState({ status: "ready", doc });
      } catch (error) {
        if (!alive || (error instanceof Error && error.name === "AbortError")) return;
        setState({ status: "error", message: t("load.failed"), retry: true });
      }
    })();
    return () => {
      alive = false;
      controller.abort();
      void task?.destroy();
    };
  }, [downloadUrl, generation]);

  const doc = state.status === "ready" ? state.doc : null;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!doc || !canvas) return;
    let alive = true;
    let render: RenderTask | null = null;
    setRenderFailed(false);
    void (async () => {
      try {
        const pdfPage = await doc.getPage(page);
        if (!alive) return;
        const base = pdfPage.getViewport({ scale: 1 });
        const scale = renderScale(base.width, base.height, zoom, window.devicePixelRatio);
        const viewport = pdfPage.getViewport({ scale });
        canvas.width = Math.floor(viewport.width);
        canvas.height = Math.floor(viewport.height);
        canvas.style.width = `${Math.floor(base.width * zoom)}px`;
        render = pdfPage.render({ canvas, viewport });
        await render.promise;
      } catch (error) {
        if (alive && !(error instanceof Error && error.name === "RenderingCancelledException")) {
          setRenderFailed(true);
        }
      }
    })();
    return () => {
      alive = false;
      render?.cancel();
    };
  }, [doc, page, zoom]);

  const retry = () => setGeneration((n) => n + 1);

  if (state.status === "loading") return <ViewerLoadingPane />;
  if (state.status === "error") {
    return (
      <ViewerErrorPane
        message={state.message}
        downloadUrl={downloadUrl}
        {...(state.retry ? { onRetry: retry } : {})}
      />
    );
  }
  if (renderFailed) {
    return <ViewerErrorPane message={t("load.failed")} downloadUrl={downloadUrl} onRetry={retry} />;
  }
  const pageCount = state.doc.numPages;
  return (
    <div className="attachment-viewer__pane" data-pdf-viewer="">
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
          disabled={page <= 1}
          onClick={() => setPage((p) => Math.max(1, p - 1))}
        >
          {t("attachment.viewer.prevPage")}
        </Button>
        <p className="attachment-viewer__page-label">
          {t("attachment.viewer.page", { current: page, total: pageCount })}
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={page >= pageCount}
          onClick={() => setPage((p) => Math.min(pageCount, p + 1))}
        >
          {t("attachment.viewer.nextPage")}
        </Button>
      </ViewerZoomToolbar>
      <div className="attachment-viewer__page-wrap">
        <canvas
          ref={canvasRef}
          className="attachment-viewer__pdf-canvas"
          aria-label={t("attachment.viewer.pdf")}
        />
      </div>
    </div>
  );
}
