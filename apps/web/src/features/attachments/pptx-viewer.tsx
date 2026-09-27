import { t } from "@fvoci/i18n";
import { useEffect, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, readCapped, zoomIn, zoomOut } from "./pdf-limits";
import { openPptx, renderSlide, type PptxDeck } from "./pptx-deck";
import { PPTX_MAX_BYTES } from "./pptx-limits";
import { sanitizeSlideSvg } from "./pptx-svg";
import { ViewerErrorPane, ViewerLoadingPane, ViewerZoomToolbar } from "./viewer-shell";
import "./pptx-viewer.css";

type DeckState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; deck: PptxDeck };

/** The rendered slide: a blob URL of the sanitized SVG, or why it cannot be shown. */
type SlideImage =
  | { deck: PptxDeck; index: number; status: "ready"; url: string }
  | { deck: PptxDeck; index: number; status: "unavailable" };

/**
 * PPTX slide viewer (source `PptxViewer`): the original bytes are laid out by
 * `@office-kit/pptx` + `@office-kit/pptx-preview` (browser entry), one slide
 * at a time as SVG, with slide and zoom controls. Read-only — no edit, save
 * or export. The SVG is sanitized (`pptx-svg.ts`) and only ever shown through
 * `<img>` from a blob URL, so deck links, scripts and external references
 * stay inert. A slide whose markup is rejected, too large or fails to decode
 * says so; the other slides stay reachable.
 */
export function PptxViewer({ downloadUrl }: { downloadUrl: string }): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<DeckState>({ status: "loading" });
  const [slide, setSlide] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [image, setImage] = useState<SlideImage | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    const isAlive = () => alive;
    const fail = (message: string, retry: boolean) => {
      if (alive) setState({ status: "error", message, retry });
    };
    setState({ status: "loading" });
    setSlide(0);
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
        const body = await readCapped(response, PPTX_MAX_BYTES);
        if (!alive) return;
        if (body.status === "tooLarge") {
          fail(t("attachment.viewer.previewUnavailable"), false);
          return;
        }
        const opened = await openPptx(body.bytes, isAlive);
        if (!alive) return;
        if (opened.status !== "ok") {
          // Over a cap, or not a deck the renderer can read: fetching again will not help.
          fail(t("attachment.viewer.previewUnavailable"), false);
          return;
        }
        setState({ status: "ready", deck: opened.deck });
      } catch (error) {
        if (!alive || (error instanceof Error && error.name === "AbortError")) return;
        fail(t("load.failed"), true);
      }
    })();
    return () => {
      alive = false;
      controller.abort();
    };
  }, [downloadUrl, generation]);

  const deck = state.status === "ready" ? state.deck : null;
  const slideCount = deck?.slides.length ?? 0;

  useEffect(() => {
    if (!deck) return;
    let url: string | null = null;
    const rendered = renderSlide(deck, slide);
    const svg = rendered.status === "ok" ? sanitizeSlideSvg(rendered.svg) : null;
    if (svg === null) {
      setImage({ deck, index: slide, status: "unavailable" });
    } else {
      url = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
      setImage({ deck, index: slide, status: "ready", url });
    }
    return () => {
      if (url) URL.revokeObjectURL(url);
    };
  }, [deck, slide]);

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
  const current = image && image.deck === state.deck && image.index === slide ? image : null;
  const label = t("attachment.viewer.slide", { current: slide + 1, total: slideCount });
  return (
    <div
      className="attachment-viewer__pane"
      data-pptx-viewer=""
      data-pptx-slide={slide}
      data-pptx-slide-state={current?.status ?? "loading"}
    >
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
          disabled={slide <= 0}
          onClick={() => setSlide((s) => Math.max(0, s - 1))}
        >
          {t("attachment.viewer.prevSlide")}
        </Button>
        <p className="attachment-viewer__page-label">{label}</p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={slide + 1 >= slideCount}
          onClick={() => setSlide((s) => Math.min(slideCount - 1, s + 1))}
        >
          {t("attachment.viewer.nextSlide")}
        </Button>
      </ViewerZoomToolbar>
      <div className="attachment-viewer__page-wrap">
        {current?.status === "ready" ? (
          <img
            key={current.url}
            src={current.url}
            alt={label}
            className="pptx-viewer__slide"
            width={Math.round(state.deck.width * zoom)}
            height={Math.round(state.deck.height * zoom)}
            onError={() =>
              setImage((shown) =>
                shown === current ? { deck: current.deck, index: current.index, status: "unavailable" } : shown,
              )
            }
          />
        ) : current?.status === "unavailable" ? (
          <p role="alert" className="attachment-viewer__alert">
            {t("attachment.viewer.previewUnavailable")}
          </p>
        ) : null}
      </div>
    </div>
  );
}
