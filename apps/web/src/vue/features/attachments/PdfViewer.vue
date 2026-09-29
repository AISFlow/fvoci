<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import type { PDFDocumentLoadingTask, PDFDocumentProxy, RenderTask } from "pdfjs-dist";
import { computed, ref, shallowRef, useTemplateRef } from "vue";
import { pdfjsAssetBase } from "@/features/attachments/pdf-assets";
import {
  PDF_MAX_BYTES,
  PDF_MAX_IMAGE_PIXELS,
  PDF_ZOOM_MAX,
  PDF_ZOOM_MIN,
  renderScale,
  zoomIn,
  zoomOut,
} from "@/features/attachments/pdf-limits";
import { downloadCapped, type ViewerPrefetch } from "@/features/attachments/viewer-download";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import { loadPdfJs } from "./pdfjs";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import ViewerLoadingPane from "./ViewerLoadingPane.vue";
import ViewerZoomToolbar from "./ViewerZoomToolbar.vue";

/**
 * Renders one page at a time onto a canvas (source `PdfViewer`): page and
 * zoom controls only. No text layer, annotation layer, links, forms or
 * document scripts — pdf.js core never runs PDF JavaScript and nothing here
 * navigates to URLs from the document.
 */
const props = defineProps<{ downloadUrl: string; prefetch?: ViewerPrefetch | undefined }>();

type DocState =
  | { status: "loading" }
  | { status: "error"; message: string; retry: boolean }
  | { status: "ready"; doc: PDFDocumentProxy };

const generation = ref(0);
// Shallow: the pdf.js document keeps private state a reactive proxy would break.
const state = shallowRef<DocState>({ status: "loading" });
const page = ref(1);
const zoom = ref(1);
const renderFailed = ref(false);
const canvas = useTemplateRef<HTMLCanvasElement>("canvas");

useEffectAfterRender([() => props.downloadUrl, generation, () => props.prefetch], () => {
  const controller = new AbortController();
  let alive = true;
  let task: PDFDocumentLoadingTask | null = null;
  state.value = { status: "loading" };
  page.value = 1;
  renderFailed.value = false;
  void (async () => {
    try {
      const body = await (props.prefetch?.take(controller.signal) ??
        downloadCapped(props.downloadUrl, PDF_MAX_BYTES, controller.signal));
      if (!alive) return;
      if (body.status === "failed") {
        state.value = { status: "error", message: t("load.failed"), retry: true };
        return;
      }
      if (body.status === "tooLarge") {
        state.value = { status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false };
        return;
      }
      const pdfjs = await loadPdfJs();
      if (!alive) return;
      const assets = new URL(`${import.meta.env.BASE_URL}${pdfjsAssetBase(pdfjs.version)}`, window.location.href).href;
      task = pdfjs.getDocument({
        data: body.bytes,
        enableXfa: false,
        maxImageSize: PDF_MAX_IMAGE_PIXELS,
        cMapUrl: `${assets}cmaps/`,
        cMapPacked: true,
        standardFontDataUrl: `${assets}standard_fonts/`,
        wasmUrl: `${assets}wasm/`,
        iccUrl: `${assets}iccs/`,
      });
      const doc = await task.promise;
      if (alive) state.value = { status: "ready", doc };
    } catch (error) {
      if (!alive || (error instanceof Error && error.name === "AbortError")) return;
      state.value = { status: "error", message: t("load.failed"), retry: true };
    }
  })();
  return () => {
    alive = false;
    controller.abort();
    void task?.destroy();
  };
});

const doc = computed(() => (state.value.status === "ready" ? state.value.doc : null));

useEffectAfterRender([doc, page, zoom], () => {
  const target = canvas.value;
  const current = doc.value;
  if (!current || !target) return;
  let alive = true;
  let render: RenderTask | null = null;
  renderFailed.value = false;
  void (async () => {
    try {
      const pdfPage = await current.getPage(page.value);
      if (!alive) return;
      const base = pdfPage.getViewport({ scale: 1 });
      const scale = renderScale(base.width, base.height, zoom.value, window.devicePixelRatio);
      const viewport = pdfPage.getViewport({ scale });
      target.width = Math.floor(viewport.width);
      target.height = Math.floor(viewport.height);
      target.style.width = `${Math.floor(base.width * zoom.value)}px`;
      render = pdfPage.render({ canvas: target, viewport });
      await render.promise;
    } catch (error) {
      if (alive && !(error instanceof Error && error.name === "RenderingCancelledException")) {
        renderFailed.value = true;
      }
    }
  })();
  return () => {
    alive = false;
    render?.cancel();
  };
});

const pageCount = computed(() => doc.value?.numPages ?? 0);

function retry(): void {
  generation.value += 1;
}
</script>

<template>
  <ViewerLoadingPane v-if="state.status === 'loading'" />
  <ViewerErrorPane
    v-else-if="state.status === 'error'"
    :message="state.message"
    :download-url="downloadUrl"
    :retryable="state.retry"
    @retry="retry"
  />
  <ViewerErrorPane
    v-else-if="renderFailed"
    :message="t('load.failed')"
    :download-url="downloadUrl"
    retryable
    @retry="retry"
  />
  <div v-else class="attachment-viewer__pane" data-pdf-viewer="">
    <ViewerZoomToolbar
      :zoom="zoom"
      :can-zoom-out="zoom > PDF_ZOOM_MIN"
      :can-zoom-in="zoom < PDF_ZOOM_MAX"
      @zoom-in="zoom = zoomIn(zoom)"
      @zoom-out="zoom = zoomOut(zoom)"
      @reset="zoom = 1"
    >
      <UButton size="sm" variant="outline" color="neutral" :disabled="page <= 1" @click="page = Math.max(1, page - 1)">
        {{ t("attachment.viewer.prevPage") }}
      </UButton>
      <p class="attachment-viewer__page-label">
        {{ t("attachment.viewer.page", { current: page, total: pageCount }) }}
      </p>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="page >= pageCount"
        @click="page = Math.min(pageCount, page + 1)"
      >
        {{ t("attachment.viewer.nextPage") }}
      </UButton>
    </ViewerZoomToolbar>
    <div class="attachment-viewer__page-wrap">
      <canvas ref="canvas" class="attachment-viewer__pdf-canvas" :aria-label="t('attachment.viewer.pdf')" />
    </div>
  </div>
</template>
