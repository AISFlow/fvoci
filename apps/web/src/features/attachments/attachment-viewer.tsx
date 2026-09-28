import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { loadErrorMessage } from "@/components/query-status";
import { publicInstanceQuery } from "@/lib/queries/admin";
import { chunkPlainText } from "./chunk-plain-text";
import type { HwpEditProps } from "./hwp-viewer";
import { viewerKind } from "./attachment-kind";
import {
  startViewerPrefetch,
  VIEWER_MAX_BYTES,
  type LayoutKind,
  type ViewerPrefetch,
} from "./viewer-download";
import { ViewerDownloadButton, ViewerErrorPane, ViewerLoadingPane } from "./viewer-shell";
import "./attachment-shell.css";

// Each layout viewer stays its own chunk (pdf.js, rhwp WASM, office renderers).
const viewerModules = {
  pdf: () => import("./pdf-viewer"),
  docx: () => import("./docx-viewer"),
  hwp: () => import("./hwp-viewer"),
  pptx: () => import("./pptx-viewer"),
  xlsx: () => import("./xlsx-viewer"),
};

type ViewerModules = { [K in LayoutKind]: Awaited<ReturnType<(typeof viewerModules)[K]>> };

/**
 * Loads the viewer chunk for `kind`. The page calls it as soon as the
 * attachment metadata names the kind; the module map makes repeat calls free.
 */
export function loadViewerModule<K extends LayoutKind>(kind: K): Promise<ViewerModules[K]> {
  return viewerModules[kind]() as Promise<ViewerModules[K]>;
}

type LayoutState<K extends LayoutKind> =
  | { status: "loading" }
  | { status: "error" }
  | { status: "ready"; module: ViewerModules[K]; prefetch: ViewerPrefetch };

/**
 * Starts the file download and the viewer chunk together, shows the loading
 * pane as ordinary state, and mounts the viewer once its module is in.
 * Not a Suspense fallback: React throttles revealing a boundary to 300 ms
 * after its fallback appeared, which held the viewer (and its download) back.
 * Unmount or a new file aborts the download; key the loader by the file.
 */
function LayoutLoader<K extends LayoutKind>({
  kind,
  downloadUrl,
  children,
}: {
  kind: K;
  downloadUrl: string;
  children: (module: ViewerModules[K], prefetch: ViewerPrefetch) => ReactNode;
}): ReactNode {
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<LayoutState<K>>({ status: "loading" });
  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    setState({ status: "loading" });
    const prefetch = startViewerPrefetch(downloadUrl, VIEWER_MAX_BYTES[kind], controller.signal);
    loadViewerModule(kind).then(
      (module) => {
        if (alive) setState({ status: "ready", module, prefetch });
      },
      () => {
        controller.abort();
        if (alive) setState({ status: "error" });
      },
    );
    return () => {
      alive = false;
      controller.abort();
    };
  }, [kind, downloadUrl, attempt]);

  if (state.status === "loading") return <ViewerLoadingPane />;
  if (state.status === "error") {
    return (
      <ViewerErrorPane
        message={t("load.failed")}
        downloadUrl={downloadUrl}
        onRetry={() => setAttempt((n) => n + 1)}
      />
    );
  }
  return children(state.module, state.prefetch);
}

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
      <LayoutLoader key={downloadUrl} kind="hwp" downloadUrl={downloadUrl}>
        {({ HwpViewer }, prefetch) => (
          <HwpViewer
            name={name}
            downloadUrl={downloadUrl}
            prefetch={prefetch}
            {...(chunk === undefined ? {} : { chunk })}
            {...(edit === undefined ? {} : { edit })}
          />
        )}
      </LayoutLoader>
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
      <LayoutLoader key={props.downloadUrl} kind="pdf" downloadUrl={props.downloadUrl}>
        {({ PdfViewer }, prefetch) => <PdfViewer downloadUrl={props.downloadUrl} prefetch={prefetch} />}
      </LayoutLoader>
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
    const url = props.downloadUrl;
    const layout =
      kind === "docx" ? (
        <LayoutLoader key={`docx:${url}`} kind="docx" downloadUrl={url}>
          {({ DocxViewer }, prefetch) => <DocxViewer downloadUrl={url} prefetch={prefetch} />}
        </LayoutLoader>
      ) : kind === "pptx" ? (
        <LayoutLoader key={`pptx:${url}`} kind="pptx" downloadUrl={url}>
          {({ PptxViewer }, prefetch) => <PptxViewer downloadUrl={url} prefetch={prefetch} />}
        </LayoutLoader>
      ) : (
        <LayoutLoader key={`xlsx:${url}`} kind="xlsx" downloadUrl={url}>
          {({ XlsxViewer }, prefetch) => <XlsxViewer downloadUrl={url} prefetch={prefetch} />}
        </LayoutLoader>
      );
    body = (
      <>
        {props.chunk !== undefined && props.previewHtmlUrl !== undefined ? (
          <SearchChunkSupplement previewHtmlUrl={props.previewHtmlUrl} chunk={props.chunk} />
        ) : null}
        {layout}
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
