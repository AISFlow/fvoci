import { t } from "@fvoci/i18n";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, readCapped, zoomIn, zoomOut } from "./pdf-limits";
import { openPptxInWorker, PptxWorkerError, type RemotePptxDeck } from "./pptx-client";
import { PPTX_MAX_BYTES } from "./pptx-limits";
import { SLIDE_IMAGE_TYPE } from "./pptx-svg";
import { ViewerErrorPane, ViewerLoadingPane, ViewerZoomToolbar } from "./viewer-shell";
import "./pptx-viewer.css";

type DeckState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; width: number; height: number; slideCount: number };

/** The downloaded original bytes; a new object for every download. */
type Source = { bytes: Uint8Array };

/** The rendered slide: a blob URL of the outer image SVG, or why it cannot be shown. */
type SlideImage =
  | { deck: RemotePptxDeck; index: number; status: "ready"; url: string }
  | { deck: RemotePptxDeck; index: number; status: "unavailable" };

/**
 * PPTX slide viewer (source `PptxViewer`): the original bytes are laid out by
 * `@office-kit/pptx` + `@office-kit/pptx-preview` (browser entry), one slide
 * at a time as SVG, with slide and zoom controls. Read-only — no edit, save
 * or export.
 *
 * Parsing and layout run in a dedicated worker (`pptx-client.ts`) with
 * wall-clock bounds. A layout cannot be interrupted, so leaving a slide whose
 * layout is still running (slide change, unmount, new load) terminates the
 * worker, and the deck is opened again from the downloaded bytes. A slide
 * past its bound is shown as unavailable and not laid out again; the other
 * slides stay reachable.
 *
 * The slide is the renderer's SVG as an image inside a fixed outer SVG
 * (`pptx-svg.ts`), shown through `<img>` from a blob URL, so deck links,
 * scripts and external references stay inert.
 */
export function PptxViewer({ downloadUrl }: { downloadUrl: string }): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<DeckState>({ status: "loading" });
  const [source, setSource] = useState<Source | null>(null);
  const [epoch, setEpoch] = useState(0);
  const [deck, setDeck] = useState<RemotePptxDeck | null>(null);
  const [slide, setSlide] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [image, setImage] = useState<SlideImage | null>(null);
  /** Slides of the current source whose layout timed out or took the worker down: not laid out again. */
  const failed = useRef<{ source: Source | null; slides: Set<number> }>({ source: null, slides: new Set() });
  /** Decks this viewer closed, or whose failure a render reported: the open effect replaces them. */
  const retired = useRef(new WeakSet<RemotePptxDeck>());

  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    const fail = (message: string, retry: boolean) => {
      if (alive) setState({ status: "error", message, retry });
    };
    setState({ status: "loading" });
    setSource(null);
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
        setSource({ bytes: body.bytes });
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

  // Opens `source` in a worker; again whenever `epoch` moves on (a terminated layout).
  useEffect(() => {
    setDeck(null);
    if (!source) return;
    if (failed.current.source !== source) failed.current = { source, slides: new Set() };
    const controller = new AbortController();
    let alive = true;
    let opened: RemotePptxDeck | null = null;
    void openPptxInWorker(source.bytes, { signal: controller.signal }).then((result) => {
      if (result.status === "ok") opened = result.deck;
      if (!alive) {
        opened?.close();
        return;
      }
      if (result.status === "ok") {
        const { width, height, slideCount } = result.deck;
        setState({ status: "ready", width, height, slideCount });
        setDeck(result.deck);
      } else if (result.status === "failed") {
        setState({ status: "error", message: t("load.failed"), retry: true });
      } else {
        // Over a cap, too slow, or not a deck the renderer can read: fetching again will not help.
        setState({ status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false });
      }
    });
    return () => {
      alive = false;
      controller.abort();
      if (opened) {
        retired.current.add(opened);
        opened.close();
      }
    };
  }, [source, epoch]);

  useEffect(() => {
    if (!deck) return;
    if (deck.closed) {
      // A retired deck is about to be replaced by the open effect. Any other closed deck lost its
      // worker while idle, with no render to report it: reopening could repeat without end, so
      // this is a load failure whose retry downloads again.
      if (!retired.current.has(deck)) setState({ status: "error", message: t("load.failed"), retry: true });
      return;
    }
    if (failed.current.slides.has(slide)) {
      setImage({ deck, index: slide, status: "unavailable" });
      return;
    }
    let alive = true;
    let settled = false;
    let url: string | null = null;
    deck.render(slide).then(
      (rendered) => {
        settled = true;
        if (!alive) return;
        if (rendered.status === "ok") {
          url = URL.createObjectURL(new Blob([rendered.svg], { type: SLIDE_IMAGE_TYPE }));
          setImage({ deck, index: slide, status: "ready", url });
        } else {
          setImage({ deck, index: slide, status: "unavailable" });
        }
      },
      (error: unknown) => {
        settled = true;
        // `closed`: whoever closed the worker also opens the next one, or the viewer is gone.
        if (!alive || (error instanceof PptxWorkerError && error.reason === "closed")) return;
        failed.current.slides.add(slide);
        retired.current.add(deck);
        setImage({ deck, index: slide, status: "unavailable" });
        // The worker is gone; the other slides need a new one.
        setEpoch((n) => n + 1);
      },
    );
    return () => {
      alive = false;
      if (!settled && !deck.closed) {
        // Still laying out this slide: only terminating the worker stops it.
        retired.current.add(deck);
        deck.close();
        setEpoch((n) => n + 1);
      }
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
  const slideCount = state.slideCount;
  const current = image && deck && image.deck === deck && image.index === slide ? image : null;
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
            width={Math.round(state.width * zoom)}
            height={Math.round(state.height * zoom)}
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
