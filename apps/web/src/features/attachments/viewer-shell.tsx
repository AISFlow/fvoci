import { t } from "@fvoci/i18n";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";

export function ViewerDownloadButton({ href }: { href: string }) {
  return (
    <a href={href} download className="inline-flex h-8 items-center rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-accent">
      {t("attachment.download")}
    </a>
  );
}

export function ViewerLoadingPane() {
  return (
    <div className="attachment-viewer__pane attachment-viewer__pane--center">
      <p className="attachment-viewer__status">{t("attachment.preview.loading")}</p>
    </div>
  );
}

export function ViewerErrorPane({
  message,
  downloadUrl,
  onRetry,
}: {
  message: string;
  downloadUrl: string;
  onRetry?: () => void;
}) {
  return (
    <div className="attachment-viewer__pane attachment-viewer__pane--center">
      <p role="alert" className="attachment-viewer__alert">
        {message}
      </p>
      <div className="attachment-viewer__tools">
        {onRetry ? (
          <Button type="button" variant="outline" size="sm" onClick={onRetry}>
            {t("load.retry")}
          </Button>
        ) : null}
        <ViewerDownloadButton href={downloadUrl} />
      </div>
    </div>
  );
}

export function ViewerZoomToolbar(props: {
  zoom: number;
  canZoomOut: boolean;
  canZoomIn: boolean;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onReset: () => void;
  children?: ReactNode;
}) {
  return (
    <div className="attachment-viewer__tools">
      <Button
        type="button"
        variant="outline"
        size="sm"
        aria-label={t("attachment.viewer.zoomOut")}
        disabled={!props.canZoomOut}
        onClick={props.onZoomOut}
      >
        −
      </Button>
      <p className="attachment-viewer__page-label" aria-live="polite">
        {Math.round(props.zoom * 100)}%
      </p>
      <Button
        type="button"
        variant="outline"
        size="sm"
        aria-label={t("attachment.viewer.zoomIn")}
        disabled={!props.canZoomIn}
        onClick={props.onZoomIn}
      >
        +
      </Button>
      <Button type="button" variant="outline" size="sm" onClick={props.onReset}>
        {t("attachment.viewer.resetZoom")}
      </Button>
      {props.children}
    </div>
  );
}
