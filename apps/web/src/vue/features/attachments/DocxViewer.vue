<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { renderAsync } from "docx-preview";
import { computed, ref, shallowRef, useTemplateRef } from "vue";
import {
  adoptFrameStyles,
  createDocxFrame,
  DOCX_FRAME_BASE_CSS,
  inertElementFactory,
  sanitizeRenderedDocx,
  transferInlineStyles,
} from "@/features/attachments/docx-frame";
import { checkDocxPackage, DOCX_MAX_BYTES } from "@/features/attachments/docx-limits";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, zoomIn, zoomOut } from "@/features/attachments/pdf-limits";
import { downloadCapped, type ViewerPrefetch } from "@/features/attachments/viewer-download";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import ViewerLoadingPane from "./ViewerLoadingPane.vue";
import ViewerZoomToolbar from "./ViewerZoomToolbar.vue";

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
const props = defineProps<{ downloadUrl: string; prefetch?: ViewerPrefetch | undefined }>();

const generation = ref(0);
const state = shallowRef<DocxState>({ status: "loading" });
const page = ref(0);
const zoom = ref(1);
const mount = useTemplateRef<HTMLDivElement>("mount");

useEffectAfterRender([() => props.downloadUrl, generation, () => props.prefetch], () => {
  const target = mount.value;
  if (!target) return;
  const controller = new AbortController();
  let alive = true;
  const isAlive = () => alive;
  const fail = (message: string, retry: boolean) => {
    if (alive) state.value = { status: "error", message, retry };
  };
  state.value = { status: "loading" };
  page.value = 0;
  void (async () => {
    try {
      const body = await (props.prefetch?.take(controller.signal) ??
        downloadCapped(props.downloadUrl, DOCX_MAX_BYTES, controller.signal));
      if (!alive) return;
      if (body.status === "failed") {
        fail(t("load.failed"), true);
        return;
      }
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
      target.replaceChildren(frame);
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
      state.value = { status: "ready", frame, pages };
    } catch (error) {
      if (!alive || (error instanceof Error && error.name === "AbortError")) return;
      fail(t("attachment.viewer.previewUnavailable"), true);
    }
  })();
  return () => {
    alive = false;
    controller.abort();
    target.replaceChildren();
  };
});

const ready = computed(() => (state.value.status === "ready" ? state.value : null));

useEffectAfterRender([ready, page, zoom], () => {
  const current = ready.value;
  if (!current) return;
  const doc = current.frame.contentDocument;
  if (!doc) return;
  current.pages.forEach((section, index) => {
    section.style.display = index === page.value ? "" : "none";
  });
  doc.documentElement.style.zoom = String(zoom.value);
  // The frame never scrolls vertically: it takes the laid-out page height.
  current.frame.style.height = `${Math.ceil(doc.documentElement.getBoundingClientRect().height * zoom.value)}px`;
});

const pageCount = computed(() => ready.value?.pages.length ?? 0);

function retry(): void {
  generation.value += 1;
}
</script>

<template>
  <div class="attachment-viewer__pane" data-docx-viewer="" :data-docx-state="state.status">
    <ViewerLoadingPane v-if="state.status === 'loading'" />
    <ViewerErrorPane
      v-else-if="state.status === 'error'"
      :message="state.message"
      :download-url="downloadUrl"
      :retryable="state.retry"
      @retry="retry"
    />
    <ViewerZoomToolbar
      v-if="ready"
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
        @click="page = Math.max(0, page - 1)"
      >
        {{ t("attachment.viewer.prevPage") }}
      </UButton>
      <p class="attachment-viewer__page-label">
        {{ t("attachment.viewer.page", { current: page + 1, total: pageCount }) }}
      </p>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="page + 1 >= pageCount"
        @click="page = Math.min(pageCount - 1, page + 1)"
      >
        {{ t("attachment.viewer.nextPage") }}
      </UButton>
    </ViewerZoomToolbar>
    <div
      ref="mount"
      class="attachment-viewer__page-wrap attachment-viewer__docx-mount"
      :style="ready ? undefined : { display: 'none' }"
    />
  </div>
</template>
