import { t } from "@fvoci/i18n";
import { renderAsync } from "docx-preview";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import {
  adoptFrameStyles,
  createDocxFrame,
  DOCX_FRAME_BASE_CSS,
  inertElementFactory,
  sanitizeRenderedDocx,
  transferInlineStyles,
} from "./docx-frame";
import { checkDocxPackage, DOCX_MAX_BYTES } from "./docx-limits";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, readCapped, zoomIn, zoomOut } from "./pdf-limits";
import { ViewerErrorPane, ViewerLoadingPane, ViewerZoomToolbar } from "./viewer-shell";

type DocxState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; frame: HTMLIFrameElement; pages: HTMLElement[] };

const PAGE_SELECTOR = ".docx-wrapper > section.docx";

/**
 * Lays out the original DOCX bytes with docx-preview (source `DocxViewer`):
 * one rendered page at a time, page and zoom controls. Read-only — no edit,
 * save or export. Pages follow the document's explicit page and section
 * breaks, as docx-preview produces them.
 */
export function DocxViewer({ downloadUrl }: { downloadUrl: string }): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<DocxState>({ status: "loading" });
  const [page, setPage] = useState(0);
  const [zoom, setZoom] = useState(1);
  const mountRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;
    const controller = new AbortController();
    let alive = true;
    const isAlive = () => alive;
    const fail = (message: string, retry: boolean) => {
      if (alive) setState({ status: "error", message, retry });
    };
    setState({ status: "loading" });
    setPage(0);
    void (async () => {
      try {
        const response = await fetch(downloadUrl, {
          credentials: "include",
          signal: controller.signal,
        });
        if (!response.ok) {
          await response.body?.cancel();
          fail(t("load.failed"), true);
          return;
        }
        const body = await readCapped(response, DOCX_MAX_BYTES);
        if (!alive) return;
        if (body.status === "tooLarge") {
          fail(t("attachment.viewer.previewUnavailable"), false);
          return;
        }
        const check = await checkDocxPackage(body.bytes, isAlive);
        if (!alive) return;
        if (check !== "ok") {
          fail(t("attachment.viewer.previewUnavailable"), false);
          return;
        }

        const scratch = document.implementation.createHTMLDocument("");
        const styleHost = scratch.createElement("div");
        await renderAsync(body.bytes, scratch.body, styleHost, {
          breakPages: true,
          inWrapper: true,
          ignoreWidth: false,
          ignoreHeight: false,
          ignoreFonts: false,
          renderHeaders: true,
          renderFooters: true,
          renderFootnotes: true,
          renderEndnotes: true,
          renderAltChunks: false,
          renderChanges: false,
          renderComments: false,
          useBase64URL: true,
          experimental: false,
          h: inertElementFactory(scratch),
        });
        if (!alive) return;
        sanitizeRenderedDocx(styleHost);
        sanitizeRenderedDocx(scratch.body);

        const { frame, ready } = createDocxFrame(document);
        mount.replaceChildren(frame);
        await ready;
        const doc = frame.contentDocument;
        const win = frame.contentWindow;
        if (!alive || !doc || !win) return;
        adoptFrameStyles(doc, win, [
          DOCX_FRAME_BASE_CSS,
          ...[...styleHost.querySelectorAll("style")].map((style) => style.textContent ?? ""),
        ]);
        for (const child of [...scratch.body.children]) {
          const copy = doc.importNode(child, true);
          doc.body.appendChild(copy);
          transferInlineStyles(child, copy);
        }
        const pages = [...doc.querySelectorAll<HTMLElement>(PAGE_SELECTOR)];
        if (pages.length === 0) {
          fail(t("attachment.viewer.previewUnavailable"), false);
          return;
        }
        setState({ status: "ready", frame, pages });
      } catch (error) {
        if (!alive || (error instanceof Error && error.name === "AbortError")) return;
        fail(t("attachment.viewer.previewUnavailable"), true);
      }
    })();
    return () => {
      alive = false;
      controller.abort();
      mount.replaceChildren();
    };
  }, [downloadUrl, generation]);

  const ready = state.status === "ready" ? state : null;

  useEffect(() => {
    if (!ready) return;
    const doc = ready.frame.contentDocument;
    if (!doc) return;
    ready.pages.forEach((section, index) => {
      section.style.display = index === page ? "" : "none";
    });
    doc.documentElement.style.zoom = String(zoom);
    // The frame never scrolls vertically: it takes the laid-out page height.
    ready.frame.style.height = `${Math.ceil(doc.documentElement.getBoundingClientRect().height * zoom)}px`;
  }, [ready, page, zoom]);

  const retry = () => setGeneration((n) => n + 1);
  const pageCount = ready?.pages.length ?? 0;

  return (
    <div className="attachment-viewer__pane" data-docx-viewer="" data-docx-state={state.status}>
      {state.status === "loading" ? <ViewerLoadingPane /> : null}
      {state.status === "error" ? (
        <ViewerErrorPane
          message={state.message}
          downloadUrl={downloadUrl}
          {...(state.retry ? { onRetry: retry } : {})}
        />
      ) : null}
      {ready ? (
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
            onClick={() => setPage((p) => Math.max(0, p - 1))}
          >
            {t("attachment.viewer.prevPage")}
          </Button>
          <p className="attachment-viewer__page-label">
            {t("attachment.viewer.page", { current: page + 1, total: pageCount })}
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={page + 1 >= pageCount}
            onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
          >
            {t("attachment.viewer.nextPage")}
          </Button>
        </ViewerZoomToolbar>
      ) : null}
      <div
        ref={mountRef}
        className="attachment-viewer__page-wrap attachment-viewer__docx-mount"
        {...(ready ? {} : { style: { display: "none" } })}
      />
    </div>
  );
}
