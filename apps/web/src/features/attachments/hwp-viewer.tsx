import { t } from "@fvoci/i18n";
import { useEffect, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { HwpClientError, HwpDocumentClient } from "./hwp-client";
import { clampPage, HWP_MAX_BYTES } from "./hwp-page";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, readCapped, zoomIn, zoomOut } from "./pdf-limits";
import { loadRhwpModule } from "./rhwp-init";
import { ViewerErrorPane, ViewerLoadingPane, ViewerZoomToolbar } from "./viewer-shell";
import "./hwp-viewer.css";

type DocState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; client: HwpDocumentClient; pageCount: number };

type PageImage = { url: string; page: number; width: number } | null;

/** A page for one document and search chunk: the chunk's start page or the user's choice. */
type PageChoice = { client: HwpDocumentClient; chunk: number | undefined; page: number } | null;

function unavailable(): DocState {
  return { status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false };
}

/**
 * HWP/HWPX layout viewer (source `HwpViewer`, view part): the original bytes
 * are parsed by the rhwp WASM in a worker of their own (`HwpDocumentClient`)
 * and one page at a time is rendered to SVG there. The worker is terminated
 * on unmount, attachment switch or retry, and when a parse or render runs
 * past its deadline, which is what releases rhwp's memory. The SVG is only
 * ever shown through `<img>` from a blob URL, so document scripts, links and
 * external references stay inert. `chunk` opens the page holding that search
 * chunk. Editing and saving are a separate slice.
 */
export function HwpViewer({ downloadUrl, chunk }: { downloadUrl: string; chunk?: number }): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<DocState>({ status: "loading" });
  const [start, setStart] = useState<PageChoice>(null);
  const [nav, setNav] = useState<PageChoice>(null);
  const [zoom, setZoom] = useState(1);
  const [image, setImage] = useState<PageImage>(null);
  const [renderFailed, setRenderFailed] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    let client: HwpDocumentClient | null = null;
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
          setState(unavailable());
          return;
        }
        const module = await loadRhwpModule();
        if (!alive) return;
        const opened = await HwpDocumentClient.open(body.bytes, module);
        if (!alive) {
          opened.client.close();
          return;
        }
        client = opened.client;
        setState({ status: "ready", client, pageCount: opened.pageCount });
      } catch (error) {
        if (!alive || (error instanceof Error && error.name === "AbortError")) return;
        // A file rhwp will not lay out, or one too costly to, stays download-only;
        // fetching and parsing it again will not help.
        const reason = error instanceof HwpClientError ? error.reason : null;
        if (reason === "tooLarge" || reason === "invalid" || reason === "timeout") {
          setState(unavailable());
          return;
        }
        setState({ status: "error", message: t("load.failed"), retry: true });
      }
    })();
    return () => {
      alive = false;
      controller.abort();
      client?.close();
    };
  }, [downloadUrl, generation]);

  const client = state.status === "ready" ? state.client : null;
  const pageCount = state.status === "ready" ? state.pageCount : 1;

  // A new document or search chunk opens its page; later navigation is the user's.
  useEffect(() => {
    if (!client) return;
    if (chunk === undefined) {
      setStart({ client, chunk, page: 0 });
      return;
    }
    let alive = true;
    client.startPage(chunk).then(
      (page) => {
        if (alive) setStart({ client, chunk, page: clampPage(page, pageCount) });
      },
      () => {
        // The worker is gone; the page render below reports it.
        if (alive) setStart({ client, chunk, page: 0 });
      },
    );
    return () => {
      alive = false;
    };
  }, [client, chunk, pageCount]);

  const matches = (choice: PageChoice) => choice !== null && choice.client === client && choice.chunk === chunk;
  const page = matches(nav) ? nav!.page : matches(start) ? start!.page : null;
  const go = (next: number) => {
    if (client) setNav({ client, chunk, page: clampPage(next, pageCount) });
  };

  useEffect(() => {
    if (!client || page === null) return;
    let alive = true;
    let url: string | null = null;
    setImage(null);
    client.renderPage(page).then(
      (svg) => {
        if (!alive) return;
        url = URL.createObjectURL(svg);
        setImage({ url, page, width: 0 });
        setRenderFailed(false);
      },
      () => {
        if (alive) setRenderFailed(true);
      },
    );
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, page]);

  const retry = () => setGeneration((n) => n + 1);

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
  if (state.status === "loading" || page === null) return <ViewerLoadingPane />;
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
            data-page={image.page}
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
