import { t } from "@fvoci/i18n";
import { HwpDocument } from "@rhwp/core";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { clampPage, decodePageText, HWP_MAX_BYTES, pageOfChunk, visiblePageCount } from "./hwp-page";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, readCapped, zoomIn, zoomOut } from "./pdf-limits";
import { ensureRhwpCore } from "./rhwp-init";
import { ViewerErrorPane, ViewerLoadingPane, ViewerZoomToolbar } from "./viewer-shell";
import "./hwp-viewer.css";

type DocState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; doc: HwpDocument; pageCount: number };

type PageImage = { url: string; width: number } | null;

/** Page the user moved to, valid only for the document and chunk it was chosen on. */
type Nav = { doc: HwpDocument; chunk: number | undefined; page: number } | null;

function startPage(doc: HwpDocument, count: number, chunk: number | undefined): number {
  if (chunk === undefined) return 0;
  try {
    const pages = Array.from({ length: count }, (_, index) => decodePageText(doc.getPageText(index)));
    return clampPage(pageOfChunk(pages, chunk), count);
  } catch {
    return 0;
  }
}

/**
 * HWP/HWPX layout viewer (source `HwpViewer`, view part): the original bytes
 * are parsed by the rhwp WASM and one page at a time is rendered to SVG.
 * The SVG is only ever shown through `<img>` from a blob URL, so document
 * scripts, links and external references stay inert. `chunk` opens the page
 * holding that search chunk. Editing and saving are a separate slice.
 */
export function HwpViewer({ downloadUrl, chunk }: { downloadUrl: string; chunk?: number }): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<DocState>({ status: "loading" });
  const [nav, setNav] = useState<Nav>(null);
  const [zoom, setZoom] = useState(1);
  const [image, setImage] = useState<PageImage>(null);
  const [renderFailed, setRenderFailed] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    let doc: HwpDocument | null = null;
    setState({ status: "loading" });
    setImage(null);
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
        const body = await readCapped(response, HWP_MAX_BYTES);
        if (!alive) return;
        if (body.status === "tooLarge") {
          setState({
            status: "error",
            message: t("attachment.viewer.previewUnavailable"),
            retry: false,
          });
          return;
        }
        await ensureRhwpCore();
        if (!alive) return;
        try {
          doc = new HwpDocument(body.bytes);
        } catch {
          // Not a document rhwp can lay out; fetching it again will not help.
          setState({
            status: "error",
            message: t("attachment.viewer.previewUnavailable"),
            retry: false,
          });
          return;
        }
        setState({ status: "ready", doc, pageCount: visiblePageCount(doc.pageCount()) });
      } catch (error) {
        if (!alive || (error instanceof Error && error.name === "AbortError")) return;
        setState({ status: "error", message: t("load.failed"), retry: true });
      }
    })();
    return () => {
      alive = false;
      controller.abort();
      doc?.free();
    };
  }, [downloadUrl, generation]);

  const doc = state.status === "ready" ? state.doc : null;
  const pageCount = state.status === "ready" ? state.pageCount : 1;

  // A new document or search chunk opens its page; later navigation is the user's.
  const start = useMemo(() => (doc ? startPage(doc, pageCount, chunk) : 0), [doc, pageCount, chunk]);
  const page = nav && nav.doc === doc && nav.chunk === chunk ? nav.page : start;
  const go = (next: number) => {
    if (doc) setNav({ doc, chunk, page: clampPage(next, pageCount) });
  };

  useEffect(() => {
    if (!doc) return;
    let url: string | null = null;
    try {
      const svg = doc.renderPageSvg(page);
      url = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
      setImage({ url, width: 0 });
      setRenderFailed(false);
    } catch {
      setImage(null);
      setRenderFailed(true);
    }
    return () => {
      if (url) URL.revokeObjectURL(url);
    };
  }, [doc, page]);

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
  const label = t("attachment.viewer.page", { current: page + 1, total: pageCount });
  return (
    <div className="attachment-viewer__pane" data-hwp-viewer="">
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
          disabled={page <= 0}
          onClick={() => go(page - 1)}
        >
          {t("attachment.viewer.prevPage")}
        </Button>
        <p className="attachment-viewer__page-label">{label}</p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={page + 1 >= pageCount}
          onClick={() => go(page + 1)}
        >
          {t("attachment.viewer.nextPage")}
        </Button>
      </ViewerZoomToolbar>
      <div className="attachment-viewer__page-wrap">
        {image ? (
          <img
            key={image.url}
            src={image.url}
            alt={label}
            className="hwp-viewer__page"
            data-page={page}
            onLoad={(event) => {
              const width = event.currentTarget.naturalWidth;
              setImage((current) => (current?.url === image.url ? { ...current, width } : current));
            }}
            {...(image.width > 0 ? { style: { width: `${image.width * zoom}px` } } : {})}
          />
        ) : null}
      </div>
    </div>
  );
}
