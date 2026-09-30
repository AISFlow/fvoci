<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { computed, nextTick, ref, shallowRef } from "vue";
import {
  createEditedAttachmentBridge,
  editedCopyName,
} from "@/features/workspace/attachment-upload";
import { HwpClientError, HwpDocumentClient } from "@/features/attachments/hwp-client";
import { hwpExportFormat } from "@/features/attachments/hwp-edit";
import { clampPage, HWP_MAX_BYTES } from "@/features/attachments/hwp-page";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, zoomIn, zoomOut } from "@/features/attachments/pdf-limits";
import { loadRhwpModule } from "@/features/attachments/rhwp-init";
import { downloadCapped, type ViewerPrefetch } from "@/features/attachments/viewer-download";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import DiscardEditsDialog from "./DiscardEditsDialog.vue";
import PendingEditsGuard from "./PendingEditsGuard.vue";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import ViewerLoadingPane from "./ViewerLoadingPane.vue";
import ViewerZoomToolbar from "./ViewerZoomToolbar.vue";
import "@/features/attachments/hwp-viewer.css";

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
const props = defineProps<{
  name: string;
  downloadUrl: string;
  prefetch?: ViewerPrefetch | undefined;
  chunk?: number | undefined;
  edit?: HwpEditProps | undefined;
}>();

const generation = ref(0);
const state = shallowRef<DocState>({ status: "loading" });
const start = shallowRef<PageChoice>(null);
const nav = shallowRef<PageChoice>(null);
const zoom = ref(1);
const image = shallowRef<PageImage>(null);
const renderFailed = ref(false);
const editing = ref(false);
const dirty = ref(false);
const findText = ref("");
const replaceText = ref("");
const editError = ref<string | null>(null);
const busy = ref<Busy>(null);
const discard = ref<Discard>(null);
// Bumped by every change to the document, so the current page is drawn again.
const revision = ref(0);
// Aborts an upload in flight when the document goes away.
let saveAbort: AbortController | null = null;
// The open document, for edit steps that finish after a switch or retry.
let liveClient: HwpDocumentClient | null = null;

useEffectAfterRender([() => props.downloadUrl, generation, () => props.prefetch], () => {
  const controller = new AbortController();
  let alive = true;
  let client: HwpDocumentClient | null = null;
  state.value = { status: "loading" };
  image.value = null;
  renderFailed.value = false;
  editing.value = false;
  dirty.value = false;
  editError.value = null;
  busy.value = null;
  discard.value = null;
  void (async () => {
    try {
      const body = await (props.prefetch?.take(controller.signal) ??
        downloadCapped(props.downloadUrl, HWP_MAX_BYTES, controller.signal));
      if (!alive) return;
      if (body.status === "failed") {
        state.value = { status: "error", message: t("load.failed"), retry: true };
        return;
      }
      if (body.status === "tooLarge") {
        state.value = unavailable();
        return;
      }
      const module = await loadRhwpModule();
      if (!alive) return;
      // The signal terminates the worker mid-parse, before a client exists here.
      const opened = await HwpDocumentClient.open(body.bytes, module, {
        signal: controller.signal,
      });
      if (!alive) {
        opened.client.close();
        return;
      }
      client = opened.client;
      liveClient = client;
      state.value = { status: "ready", client, pageCount: opened.pageCount };
    } catch (error) {
      if (!alive || (error instanceof Error && error.name === "AbortError")) return;
      // A file rhwp will not lay out, or one too costly to, stays download-only;
      // fetching and parsing it again will not help.
      const reason = error instanceof HwpClientError ? error.reason : null;
      if (reason === "tooLarge" || reason === "invalid" || reason === "timeout") {
        state.value = unavailable();
        return;
      }
      state.value = { status: "error", message: t("load.failed"), retry: true };
    }
  })();
  return () => {
    alive = false;
    controller.abort();
    saveAbort?.abort();
    saveAbort = null;
    liveClient = null;
    client?.close();
  };
});

const client = computed(() => (state.value.status === "ready" ? state.value.client : null));
const pageCount = computed(() => (state.value.status === "ready" ? state.value.pageCount : 1));

useEffectAfterRender([client, () => props.chunk, pageCount], () => {
  const current = client.value;
  if (!current) return;
  if (props.chunk === undefined) {
    start.value = { client: current, chunk: props.chunk, page: 0 };
    return;
  }
  let alive = true;
  const chunk = props.chunk;
  current.startPage(chunk).then(
    (page) => {
      if (alive) start.value = { client: current, chunk, page: clampPage(page, pageCount.value) };
    },
    () => {
      // The worker is gone; the page render below reports it.
      if (alive) start.value = { client: current, chunk, page: 0 };
    },
  );
  return () => {
    alive = false;
  };
});

function matches(choice: PageChoice): boolean {
  return choice !== null && choice.client === client.value && choice.chunk === props.chunk;
}

const chosen = computed(() => {
  if (matches(nav.value)) return nav.value!.page;
  if (matches(start.value)) return start.value!.page;
  return null;
});
// An edit may have shortened the document under the chosen page.
const page = computed(() =>
  chosen.value === null ? null : clampPage(chosen.value, pageCount.value),
);

function go(next: number): void {
  const current = client.value;
  if (current)
    nav.value = { client: current, chunk: props.chunk, page: clampPage(next, pageCount.value) };
}

useEffectAfterRender([client, page, revision], () => {
  const current = client.value;
  const target = page.value;
  if (!current || target === null) return;
  let alive = true;
  let url: string | null = null;
  image.value = null;
  current.renderPage(target).then(
    (svg) => {
      if (!alive) return;
      url = URL.createObjectURL(svg);
      image.value = { url, page: target, width: 0 };
      renderFailed.value = false;
    },
    () => {
      if (alive) renderFailed.value = true;
    },
  );
  return () => {
    alive = false;
    if (url) URL.revokeObjectURL(url);
  };
});

function retry(): void {
  generation.value += 1;
}

const canEdit = computed(() => props.edit?.editable === true && client.value !== null);
const save = computed(() => props.edit?.save);
const exportMeta = computed(() => hwpExportFormat(props.name));
const locked = computed(() => busy.value !== null);
const label = computed(() =>
  page.value === null
    ? ""
    : t("attachment.viewer.page", { current: page.value + 1, total: pageCount.value }),
);

const discardLabel = computed(() =>
  discard.value === "undo"
    ? t("attachment.viewer.edit.undo")
    : discard.value === "exit"
      ? t("attachment.viewer.edit.exit")
      : t("load.retry"),
);

function lost(): void {
  dirty.value = false;
  editing.value = false;
  state.value = { status: "error", message: t("load.failed"), retry: true };
}

function edited(count: number): void {
  const current = state.value;
  if (current.status === "ready") state.value = { ...current, pageCount: count };
  revision.value += 1;
}

/** One edit step on the open document; a step that outlives it changes nothing. */
async function run(
  kind: Exclude<Busy, null>,
  step: (doc: HwpDocumentClient, live: () => boolean) => Promise<void>,
  failure: string,
): Promise<void> {
  const doc = client.value;
  if (!doc || busy.value) return;
  const live = () => liveClient === doc && !doc.closed;
  busy.value = kind;
  editError.value = null;
  try {
    await step(doc, live);
  } catch {
    if (liveClient !== doc) return;
    if (doc.closed) lost();
    else editError.value = failure;
  } finally {
    if (liveClient === doc) busy.value = null;
  }
}

function replace(all: boolean): void {
  void run(
    "replace",
    async (doc, live) => {
      const result = await doc.replace(findText.value, replaceText.value, all);
      if (!live()) return;
      if (result.outcome === "changed") {
        dirty.value = true;
        edited(result.pageCount);
      } else {
        // Nothing replaced, so nothing to save or guard.
        editError.value =
          result.outcome === "unchanged"
            ? t("attachment.viewer.edit.notFound")
            : t("attachment.viewer.edit.failed");
      }
    },
    t("attachment.viewer.edit.failed"),
  );
}

function revert(then?: () => void): void {
  void run(
    "revert",
    async (doc, live) => {
      const count = await doc.revert();
      if (!live()) return;
      edited(count);
      dirty.value = false;
      then?.();
    },
    t("attachment.viewer.edit.failed"),
  );
}

function download(): void {
  void run(
    "download",
    async (doc, live) => {
      const bytes = await doc.exportDocument(exportMeta.value.format);
      if (!live()) return;
      const href = URL.createObjectURL(
        new Blob([bytes as Uint8Array<ArrayBuffer>], { type: exportMeta.value.mime }),
      );
      const link = document.createElement("a");
      link.href = href;
      link.download = editedCopyName(props.name);
      link.click();
      // The download has taken the bytes by the next task.
      setTimeout(() => URL.revokeObjectURL(href), 0);
    },
    t("attachment.viewer.edit.failed"),
  );
}

function saveCopy(): void {
  const dest = save.value;
  if (!dest) return;
  void run(
    "save",
    async (doc, live) => {
      const bytes = await doc.exportDocument(exportMeta.value.format);
      if (!live()) return;
      const file = new File([bytes as Uint8Array<ArrayBuffer>], editedCopyName(props.name), {
        type: exportMeta.value.mime,
      });
      const controller = new AbortController();
      saveAbort = controller;
      const saved = await createEditedAttachmentBridge(dest.workspaceId, dest.attachmentId).upload(
        file,
        () => undefined,
        controller.signal,
      );
      saveAbort = null;
      if (!live()) return;
      // Clear the guard before leaving for the copy.
      dirty.value = false;
      await nextTick();
      dest.onSavedCopy(saved.id);
    },
    t("attachment.viewer.edit.saveFailed"),
  );
}

function confirmDiscard(): void {
  const action = discard.value;
  discard.value = null;
  if (action === "undo") revert();
  else if (action === "exit") revert(() => (editing.value = false));
  else if (action === "retry") retry();
}

function exitEditing(): void {
  if (dirty.value) discard.value = "exit";
  else {
    editing.value = false;
    editError.value = null;
  }
}

function retryRender(): void {
  if (dirty.value) discard.value = "retry";
  else retry();
}

function onPageLoad(event: Event): void {
  const el = event.currentTarget as HTMLImageElement;
  const width = el.naturalWidth;
  const shown = image.value;
  if (shown && shown.url === el.src) image.value = { ...shown, width };
}
</script>

<template>
  <template v-if="state.status === 'error'">
    <ViewerErrorPane
      :message="state.message"
      :download-url="downloadUrl"
      :retryable="state.retry"
      @retry="retry"
    />
  </template>
  <template v-else-if="renderFailed">
    <PendingEditsGuard v-if="dirty" />
    <DiscardEditsDialog
      v-if="discard"
      :action-label="discardLabel"
      @confirm="confirmDiscard"
      @cancel="discard = null"
    />
    <ViewerErrorPane
      :message="t('load.failed')"
      :download-url="downloadUrl"
      retryable
      @retry="retryRender"
    />
  </template>
  <ViewerLoadingPane v-else-if="state.status === 'loading' || page === null" />
  <div v-else class="attachment-viewer__pane" data-hwp-viewer="">
    <PendingEditsGuard v-if="dirty" />
    <DiscardEditsDialog
      v-if="discard"
      :action-label="discardLabel"
      @confirm="confirmDiscard"
      @cancel="discard = null"
    />
    <ViewerZoomToolbar
      :zoom="zoom"
      :can-zoom-out="zoom > PDF_ZOOM_MIN"
      :can-zoom-in="zoom < PDF_ZOOM_MAX"
      @zoom-in="zoom = zoomIn(zoom)"
      @zoom-out="zoom = zoomOut(zoom)"
      @reset="zoom = 1"
    >
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="page <= 0"
        @click="go(page - 1)"
      >
        {{ t("attachment.viewer.prevPage") }}
      </UButton>
      <p class="attachment-viewer__page-label">{{ label }}</p>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="page + 1 >= pageCount"
        @click="go(page + 1)"
      >
        {{ t("attachment.viewer.nextPage") }}
      </UButton>
      <UButton v-if="canEdit && !editing" size="sm" @click="editing = true">
        {{ t("attachment.viewer.edit.start") }}
      </UButton>
    </ViewerZoomToolbar>
    <div
      v-if="canEdit && editing"
      class="hwp-viewer__edit-bar"
      data-hwp-edit-bar=""
      :aria-busy="locked"
    >
      <UInput
        v-model="findText"
        class="hwp-viewer__edit-input"
        :aria-label="t('attachment.viewer.edit.find')"
        :placeholder="t('attachment.viewer.edit.find')"
        :disabled="locked"
      />
      <UInput
        v-model="replaceText"
        class="hwp-viewer__edit-input"
        :aria-label="t('attachment.viewer.edit.replacement')"
        :placeholder="t('attachment.viewer.edit.replacement')"
        :disabled="locked"
      />
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="findText.length === 0 || locked"
        @click="replace(false)"
      >
        {{ t("attachment.viewer.edit.replaceOne") }}
      </UButton>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="findText.length === 0 || locked"
        @click="replace(true)"
      >
        {{ t("attachment.viewer.edit.replaceAll") }}
      </UButton>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="!dirty || locked"
        @click="discard = 'undo'"
      >
        {{ t("attachment.viewer.edit.undo") }}
      </UButton>
      <UButton v-if="save" size="sm" :disabled="!dirty || locked" @click="saveCopy">
        {{
          busy === "save"
            ? t("attachment.viewer.edit.saving")
            : t("attachment.viewer.edit.saveCopy")
        }}
      </UButton>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="!dirty || locked"
        @click="download"
      >
        {{ t("attachment.viewer.edit.download") }}
      </UButton>
      <UButton size="sm" variant="outline" color="neutral" :disabled="locked" @click="exitEditing">
        {{ t("attachment.viewer.edit.exit") }}
      </UButton>
      <p v-if="editError" role="alert" class="attachment-viewer__alert">{{ editError }}</p>
    </div>
    <div class="attachment-viewer__page-wrap">
      <img
        v-if="image"
        :key="image.url"
        :src="image.url"
        :alt="label"
        class="hwp-viewer__page"
        :data-page="image.page"
        :style="image.width > 0 ? { width: `${image.width * zoom}px` } : undefined"
        @load="onPageLoad"
      />
    </div>
  </div>
</template>
