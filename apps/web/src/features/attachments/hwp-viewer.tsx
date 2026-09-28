import { t } from "@fvoci/i18n";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { flushSync } from "react-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { createEditedAttachmentBridge, editedCopyName } from "@/features/workspace/attachment-upload";
import { HwpClientError, HwpDocumentClient } from "./hwp-client";
import { hwpExportFormat } from "./hwp-edit";
import { DiscardEditsDialog, PendingEditsGuard } from "./hwp-pending-edits";
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

/**
 * Session edit context (source `hwpEditable` + `onSavedCopy`). `editable`
 * comes from `GET …/edit-context`; `save` is present only in a workspace
 * session, and the server re-checks edit access when the copy is written.
 * A share view passes none of it.
 */
export type HwpEditProps = {
  editable: boolean;
  save?: { workspaceId: string; attachmentId: string; onSavedCopy: (attachmentId: string) => void };
};

/** An edit step waiting on the "discard edits?" dialog. */
type Discard = "undo" | "exit" | "retry" | null;

type Busy = "replace" | "revert" | "download" | "save" | null;

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
 * chunk.
 *
 * With `edit.editable` (session only) 간단 편집 edits that same worker
 * document: find/replace, revert to the original, download the edited copy
 * as a file (no upload needed), and — with `edit.save` — save it as a new
 * attachment through the edit-copy upload and open it. The copy keeps the
 * original's format by file name. Unsaved edits hold in-app navigation and
 * tab close behind a confirmation.
 */
export function HwpViewer({
  name,
  downloadUrl,
  chunk,
  edit,
}: {
  name: string;
  downloadUrl: string;
  chunk?: number;
  edit?: HwpEditProps;
}): ReactNode {
  const [generation, setGeneration] = useState(0);
  const [state, setState] = useState<DocState>({ status: "loading" });
  const [start, setStart] = useState<PageChoice>(null);
  const [nav, setNav] = useState<PageChoice>(null);
  const [zoom, setZoom] = useState(1);
  const [image, setImage] = useState<PageImage>(null);
  const [renderFailed, setRenderFailed] = useState(false);
  const [editing, setEditing] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [findText, setFindText] = useState("");
  const [replaceText, setReplaceText] = useState("");
  const [editError, setEditError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);
  const [discard, setDiscard] = useState<Discard>(null);
  // Bumped by every change to the document, so the current page is drawn again.
  const [revision, setRevision] = useState(0);
  // Aborts an upload in flight when the document goes away.
  const saveAbort = useRef<AbortController | null>(null);
  // The open document, for edit steps that finish after a switch or retry.
  const current = useRef<HwpDocumentClient | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    let alive = true;
    let client: HwpDocumentClient | null = null;
    setState({ status: "loading" });
    setImage(null);
    setRenderFailed(false);
    setEditing(false);
    setDirty(false);
    setEditError(null);
    setBusy(null);
    setDiscard(null);
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
        // The signal terminates the worker mid-parse, before a client exists here.
        const opened = await HwpDocumentClient.open(body.bytes, module, { signal: controller.signal });
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
      saveAbort.current?.abort();
      saveAbort.current = null;
      client?.close();
    };
  }, [downloadUrl, generation]);

  const client = state.status === "ready" ? state.client : null;
  const pageCount = state.status === "ready" ? state.pageCount : 1;
  current.current = client;

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
  const chosen = matches(nav) ? nav!.page : matches(start) ? start!.page : null;
  // An edit may have shortened the document under the chosen page.
  const page = chosen === null ? null : clampPage(chosen, pageCount);
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
  }, [client, page, revision]);

  const retry = () => setGeneration((n) => n + 1);
  const canEdit = edit?.editable === true && client !== null;
  const save = edit?.save;
  const { format, mime } = hwpExportFormat(name);

  // The worker died mid-edit (deadline, crash): its edits are gone with it.
  const lost = () => {
    setDirty(false);
    setEditing(false);
    setState({ status: "error", message: t("load.failed"), retry: true });
  };

  const edited = (count: number) => {
    setState((current) => (current.status === "ready" ? { ...current, pageCount: count } : current));
    setRevision((n) => n + 1);
  };

  /** One edit step on the open document; a step that outlives it changes nothing. */
  const run = async (
    kind: Exclude<Busy, null>,
    step: (doc: HwpDocumentClient, live: () => boolean) => Promise<void>,
    failure: string,
  ) => {
    const doc = client;
    if (!doc || busy) return;
    const live = () => current.current === doc && !doc.closed;
    setBusy(kind);
    setEditError(null);
    try {
      await step(doc, live);
    } catch {
      if (current.current !== doc) return;
      if (doc.closed) lost();
      else setEditError(failure);
    } finally {
      if (current.current === doc) setBusy(null);
    }
  };

  const replace = (all: boolean) =>
    void run(
      "replace",
      async (doc, live) => {
        const result = await doc.replace(findText, replaceText, all);
        if (!live()) return;
        if (result.outcome === "changed") {
          setDirty(true);
          edited(result.pageCount);
        } else {
          // Nothing replaced, so nothing to save or guard.
          setEditError(
            result.outcome === "unchanged" ? t("attachment.viewer.edit.notFound") : t("attachment.viewer.edit.failed"),
          );
        }
      },
      t("attachment.viewer.edit.failed"),
    );

  const revert = (then?: () => void) =>
    void run(
      "revert",
      async (doc, live) => {
        const count = await doc.revert();
        if (!live()) return;
        edited(count);
        setDirty(false);
        then?.();
      },
      t("attachment.viewer.edit.failed"),
    );

  const download = () =>
    void run(
      "download",
      async (doc, live) => {
        const bytes = await doc.exportDocument(format);
        if (!live()) return;
        const href = URL.createObjectURL(new Blob([bytes as Uint8Array<ArrayBuffer>], { type: mime }));
        const link = document.createElement("a");
        link.href = href;
        link.download = editedCopyName(name);
        link.click();
        // The download has taken the bytes by the next task.
        setTimeout(() => URL.revokeObjectURL(href), 0);
      },
      t("attachment.viewer.edit.failed"),
    );

  const saveCopy = () => {
    if (!save) return;
    void run(
      "save",
      async (doc, live) => {
        const bytes = await doc.exportDocument(format);
        if (!live()) return;
        const file = new File([bytes as Uint8Array<ArrayBuffer>], editedCopyName(name), { type: mime });
        const controller = new AbortController();
        saveAbort.current = controller;
        const saved = await createEditedAttachmentBridge(save.workspaceId, save.attachmentId).upload(
          file,
          () => undefined,
          controller.signal,
        );
        saveAbort.current = null;
        if (!live()) return;
        // Clear the guard before leaving for the copy.
        flushSync(() => setDirty(false));
        save.onSavedCopy(saved.id);
      },
      t("attachment.viewer.edit.saveFailed"),
    );
  };

  const confirmDiscard = () => {
    const action = discard;
    setDiscard(null);
    if (action === "undo") revert();
    else if (action === "exit") revert(() => setEditing(false));
    else if (action === "retry") retry();
  };

  const exitEditing = () => {
    if (dirty) setDiscard("exit");
    else {
      setEditing(false);
      setEditError(null);
    }
  };

  const retryRender = () => {
    if (dirty) setDiscard("retry");
    else retry();
  };

  const guard = (
    <>
      {dirty ? <PendingEditsGuard /> : null}
      {discard ? (
        <DiscardEditsDialog
          actionLabel={
            discard === "undo"
              ? t("attachment.viewer.edit.undo")
              : discard === "exit"
                ? t("attachment.viewer.edit.exit")
                : t("load.retry")
          }
          onConfirm={confirmDiscard}
          onCancel={() => setDiscard(null)}
        />
      ) : null}
    </>
  );

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
    return (
      <>
        {guard}
        <ViewerErrorPane message={t("load.failed")} downloadUrl={downloadUrl} onRetry={retryRender} />
      </>
    );
  }
  if (state.status === "loading" || page === null) return <ViewerLoadingPane />;
  const label = t("attachment.viewer.page", { current: page + 1, total: pageCount });
  const locked = busy !== null;
  return (
    <div className="attachment-viewer__pane" data-hwp-viewer="">
      {guard}
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
        {canEdit && !editing ? (
          <Button type="button" variant="default" size="sm" onClick={() => setEditing(true)}>
            {t("attachment.viewer.edit.start")}
          </Button>
        ) : null}
      </ViewerZoomToolbar>
      {canEdit && editing ? (
        <div className="hwp-viewer__edit-bar" data-hwp-edit-bar="" aria-busy={locked}>
          <Input
            className="hwp-viewer__edit-input"
            aria-label={t("attachment.viewer.edit.find")}
            placeholder={t("attachment.viewer.edit.find")}
            value={findText}
            disabled={locked}
            onChange={(event) => setFindText(event.target.value)}
          />
          <Input
            className="hwp-viewer__edit-input"
            aria-label={t("attachment.viewer.edit.replacement")}
            placeholder={t("attachment.viewer.edit.replacement")}
            value={replaceText}
            disabled={locked}
            onChange={(event) => setReplaceText(event.target.value)}
          />
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={findText.length === 0 || locked}
            onClick={() => replace(false)}
          >
            {t("attachment.viewer.edit.replaceOne")}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={findText.length === 0 || locked}
            onClick={() => replace(true)}
          >
            {t("attachment.viewer.edit.replaceAll")}
          </Button>
          <Button type="button" variant="outline" size="sm" disabled={!dirty || locked} onClick={() => setDiscard("undo")}>
            {t("attachment.viewer.edit.undo")}
          </Button>
          {save ? (
            <Button type="button" variant="default" size="sm" disabled={!dirty || locked} onClick={saveCopy}>
              {busy === "save" ? t("attachment.viewer.edit.saving") : t("attachment.viewer.edit.saveCopy")}
            </Button>
          ) : null}
          <Button type="button" variant="outline" size="sm" disabled={!dirty || locked} onClick={download}>
            {t("attachment.viewer.edit.download")}
          </Button>
          <Button type="button" variant="outline" size="sm" disabled={locked} onClick={exitEditing}>
            {t("attachment.viewer.edit.exit")}
          </Button>
          {editError ? (
            <p role="alert" className="attachment-viewer__alert">
              {editError}
            </p>
          ) : null}
        </div>
      ) : null}
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
