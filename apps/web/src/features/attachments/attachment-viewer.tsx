import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { loadErrorMessage } from "@/components/query-status";
import { publicInstanceQuery } from "@/lib/queries/admin";
import { chunkPlainText } from "./chunk-plain-text";
import type { HwpEditProps } from "./hwp-viewer";
import { viewerKind } from "./attachment-kind";
import { ViewerDownloadButton, ViewerErrorPane, ViewerLoadingPane } from "./viewer-shell";
import "./attachment-shell.css";

const PdfViewer = lazy(async () => {
  const mod = await import("./pdf-viewer");
  return { default: mod.PdfViewer };
});

const DocxViewer = lazy(async () => {
  const mod = await import("./docx-viewer");
  return { default: mod.DocxViewer };
});

const HwpViewer = lazy(async () => {
  const mod = await import("./hwp-viewer");
  return { default: mod.HwpViewer };
});

const PptxViewer = lazy(async () => {
  const mod = await import("./pptx-viewer");
  return { default: mod.PptxViewer };
});

const XlsxViewer = lazy(async () => {
  const mod = await import("./xlsx-viewer");
  return { default: mod.XlsxViewer };
});

export type AttachmentViewerProps = {
  name: string;
  mime: string;
  image: boolean;
  downloadUrl: string;
  error?: string | null;
  onMetadataRetry?: () => void;
  chunk?: number;
  /** Session-only `preview-html` URL for the search-chunk supplement; share views omit it. */
  previewHtmlUrl?: string;
  /** Session-only HWP/HWPX 간단 편집 (edit-context + save-copy); share views omit it. */
  hwpEdit?: HwpEditProps;
};

function ChunkText({ text, chunk }: { text: string; chunk?: number }) {
  const mark = useRef<HTMLElement>(null);
  const hit = chunk === undefined ? undefined : chunkPlainText(text)[chunk];
  useEffect(() => {
    mark.current?.scrollIntoView({ block: "center" });
  }, [text, chunk]);
  if (hit === undefined) {
    return <pre className="attachment-viewer__text">{text}</pre>;
  }
  return (
    <pre className="attachment-viewer__text">
      {text.slice(0, hit.start)}
      <mark ref={mark}>{hit.text}</mark>
      {text.slice(hit.end)}
    </pre>
  );
}

function TextBytesPane({ downloadUrl, chunk }: { downloadUrl: string; chunk?: number }) {
  const [state, setState] = useState<
    { status: "loading" } | { status: "error"; message: string } | { status: "text"; text: string }
  >({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });
    void fetch(downloadUrl, { credentials: "include" })
      .then(async (response) => {
        if (!response.ok) {
          throw new Error(String(response.status));
        }
        return response.text();
      })
      .then((text) => {
        if (!cancelled) setState({ status: "text", text });
      })
      .catch((error: unknown) => {
        if (!cancelled) setState({ status: "error", message: loadErrorMessage(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [downloadUrl]);

  if (state.status === "loading") {
    return <ViewerLoadingPane />;
  }
  if (state.status === "error") {
    return <ViewerErrorPane message={state.message} downloadUrl={downloadUrl} />;
  }
  return (
    <div className="attachment-viewer__pane">
      <ChunkText text={state.text} chunk={chunk} />
    </div>
  );
}

type SupplementState =
  | { status: "loading" }
  | { status: "unavailable" }
  | { status: "error" }
  | { status: "text"; text: string };

/**
 * Search hit context for a laid-out document (source `SearchChunkSupplement`):
 * the stored extract text from `preview-html`, shown as plain text with the
 * hit highlighted above the layout viewer. It never replaces the layout, and
 * its failure never hides it.
 */
function SearchChunkSupplement({ previewHtmlUrl, chunk }: { previewHtmlUrl: string; chunk: number }) {
  const [state, setState] = useState<SupplementState>({ status: "loading" });
  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    setState({ status: "loading" });
    void (async () => {
      try {
        const response = await fetch(previewHtmlUrl, {
          credentials: "include",
          signal: controller.signal,
        });
        if (!response.ok) {
          await response.body?.cancel();
          if (alive) {
            setState({
              status: response.status === 404 || response.status === 413 ? "unavailable" : "error",
            });
          }
          return;
        }
        const payload: unknown = await response.json();
        const html =
          typeof payload === "object" && payload !== null && "html" in payload ? payload.html : null;
        if (!alive) return;
        if (typeof html !== "string") {
          setState({ status: "error" });
          return;
        }
        // The server escapes the text into one <pre>; only its text is used, never markup.
        const text = new DOMParser().parseFromString(html, "text/html").body.textContent ?? "";
        setState({ status: "text", text });
      } catch (error) {
        if (!alive || (error instanceof Error && error.name === "AbortError")) return;
        setState({ status: "error" });
      }
    })();
    return () => {
      alive = false;
      controller.abort();
    };
  }, [previewHtmlUrl]);

  return (
    <section className="attachment-viewer__pane attachment-viewer__pane--supplement" data-chunk-supplement="">
      <p className="attachment-viewer__status">{t("attachment.viewer.layoutNone")}</p>
      {state.status === "loading" ? (
        <p className="attachment-viewer__status">{t("attachment.preview.loading")}</p>
      ) : state.status === "text" ? (
        <ChunkText text={state.text} chunk={chunk} />
      ) : (
        <p className="attachment-viewer__status">
          {state.status === "unavailable" ? t("attachment.viewer.previewUnavailable") : t("load.failed")}
        </p>
      )}
    </section>
  );
}

/**
 * HWP/HWPX (source `HwpPane`): the rhwp layout always, plus the search
 * supplement only when the instance extracts on the server (`mode ===
 * "server"`). The mode is read only for a session hit with a chunk, so a
 * share view never calls `/instance` or `preview-html`, and the layout does
 * not wait for it.
 */
function HwpPane({
  name,
  downloadUrl,
  previewHtmlUrl,
  chunk,
  edit,
}: {
  name: string;
  downloadUrl: string;
  previewHtmlUrl?: string;
  chunk?: number;
  edit?: HwpEditProps;
}) {
  const wantsSupplement = previewHtmlUrl !== undefined && chunk !== undefined;
  const mode = useQuery({
    ...publicInstanceQuery,
    enabled: wantsSupplement,
    select: (data) => data.values.attachmentPreview.mode,
  });
  return (
    <>
      {wantsSupplement && mode.data === "server" ? (
        <SearchChunkSupplement previewHtmlUrl={previewHtmlUrl} chunk={chunk} />
      ) : null}
      <Suspense fallback={<ViewerLoadingPane />}>
        <HwpViewer
          key={downloadUrl}
          name={name}
          downloadUrl={downloadUrl}
          {...(chunk === undefined ? {} : { chunk })}
          {...(edit === undefined ? {} : { edit })}
        />
      </Suspense>
    </>
  );
}

export function AttachmentViewer(props: AttachmentViewerProps): ReactNode {
  const kind = viewerKind({ name: props.name, mime: props.mime, image: props.image });
  const chrome = (
    <header className="attachment-viewer__head">
      <p className="attachment-viewer__name">{props.name}</p>
      <ViewerDownloadButton href={props.downloadUrl} />
    </header>
  );
  if (props.error) {
    return (
      <div data-attachment-viewer="" className="attachment-viewer">
        {chrome}
        <div className="attachment-viewer__stage">
          <ViewerErrorPane
            message={props.error}
            downloadUrl={props.downloadUrl}
            onRetry={props.onMetadataRetry}
          />
        </div>
      </div>
    );
  }
  let body: ReactNode;
  if (kind === "download") {
    body = (
      <div className="attachment-viewer__pane attachment-viewer__pane--center">
        <ViewerErrorPane
          message={t("attachment.viewer.previewUnavailable")}
          downloadUrl={props.downloadUrl}
        />
      </div>
    );
  } else if (kind === "image") {
    body = (
      <div className="attachment-viewer__pane">
        <img className="attachment-viewer__image" src={props.downloadUrl} alt={props.name} />
      </div>
    );
  } else if (kind === "pdf") {
    body = (
      <Suspense fallback={<ViewerLoadingPane />}>
        <PdfViewer downloadUrl={props.downloadUrl} />
      </Suspense>
    );
  } else if (kind === "hwp") {
    body = (
      <HwpPane
        name={props.name}
        downloadUrl={props.downloadUrl}
        {...(props.previewHtmlUrl === undefined ? {} : { previewHtmlUrl: props.previewHtmlUrl })}
        {...(props.chunk === undefined ? {} : { chunk: props.chunk })}
        {...(props.hwpEdit === undefined ? {} : { edit: props.hwpEdit })}
      />
    );
  } else if (kind === "docx" || kind === "pptx" || kind === "xlsx") {
    const Layout = kind === "docx" ? DocxViewer : kind === "pptx" ? PptxViewer : XlsxViewer;
    body = (
      <>
        {props.chunk !== undefined && props.previewHtmlUrl !== undefined ? (
          <SearchChunkSupplement previewHtmlUrl={props.previewHtmlUrl} chunk={props.chunk} />
        ) : null}
        <Suspense fallback={<ViewerLoadingPane />}>
          <Layout key={props.downloadUrl} downloadUrl={props.downloadUrl} />
        </Suspense>
      </>
    );
  } else {
    body = <TextBytesPane downloadUrl={props.downloadUrl} chunk={props.chunk} />;
  }
  return (
    <div data-attachment-viewer="" className="attachment-viewer">
      {chrome}
      <div className="attachment-viewer__stage">{body}</div>
    </div>
  );
}
