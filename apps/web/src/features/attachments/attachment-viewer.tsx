import { t } from "@fvoci/i18n";
import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from "react";
import { loadErrorMessage } from "@/components/query-status";
import { chunkPlainText } from "./chunk-plain-text";
import { viewerKind } from "./attachment-kind";
import { ViewerDownloadButton, ViewerErrorPane, ViewerLoadingPane } from "./viewer-shell";
import "./attachment-shell.css";

const PdfViewer = lazy(async () => {
  const mod = await import("./pdf-viewer");
  return { default: mod.PdfViewer };
});

export type AttachmentViewerProps = {
  name: string;
  mime: string;
  image: boolean;
  downloadUrl: string;
  error?: string | null;
  onMetadataRetry?: () => void;
  chunk?: number;
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
