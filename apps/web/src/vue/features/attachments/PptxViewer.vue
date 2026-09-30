<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, shallowRef } from "vue";
import { PDF_ZOOM_MAX, PDF_ZOOM_MIN, zoomIn, zoomOut } from "@/features/attachments/pdf-limits";
import { openPptxInWorker, PptxWorkerError, type RemotePptxDeck } from "@/features/attachments/pptx-client";
import { PPTX_MAX_BYTES } from "@/features/attachments/pptx-limits";
import { SLIDE_IMAGE_TYPE } from "@/features/attachments/pptx-svg";
import { downloadCapped, type ViewerPrefetch } from "@/features/attachments/viewer-download";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import ViewerLoadingPane from "./ViewerLoadingPane.vue";
import ViewerZoomToolbar from "./ViewerZoomToolbar.vue";
import "@/features/attachments/pptx-viewer.css";

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
const props = defineProps<{ downloadUrl: string; prefetch?: ViewerPrefetch | undefined }>();

const generation = ref(0);
const state = shallowRef<DeckState>({ status: "loading" });
const source = shallowRef<Source | null>(null);
const epoch = ref(0);
const deck = shallowRef<RemotePptxDeck | null>(null);
const slide = ref(0);
const zoom = ref(1);
const image = shallowRef<SlideImage | null>(null);
/** Slides of the current source whose layout timed out or took the worker down: not laid out again. */
const failed = { source: null as Source | null, slides: new Set<number>() };
/** Decks this viewer closed, or whose failure a render reported: the open effect replaces them. */
const retired = new WeakSet<RemotePptxDeck>();

useEffectAfterRender([() => props.downloadUrl, generation, () => props.prefetch], () => {
  const controller = new AbortController();
  let alive = true;
  const fail = (message: string, retry: boolean) => {
    if (alive) state.value = { status: "error", message, retry };
  };
  state.value = { status: "loading" };
  source.value = null;
  slide.value = 0;
  void (async () => {
    try {
      const body = await (props.prefetch?.take(controller.signal) ??
        downloadCapped(props.downloadUrl, PPTX_MAX_BYTES, controller.signal));
      if (!alive) return;
      if (body.status === "failed") {
        fail(t("load.failed"), true);
        return;
      }
      if (body.status === "tooLarge") {
        fail(t("attachment.viewer.previewUnavailable"), false);
        return;
      }
      source.value = { bytes: body.bytes };
    } catch (error) {
      if (!alive || (error instanceof Error && error.name === "AbortError")) return;
      fail(t("load.failed"), true);
    }
  })();
  return () => {
    alive = false;
    controller.abort();
  };
});

// Opens `source` in a worker; again whenever `epoch` moves on (a terminated layout).
useEffectAfterRender([source, epoch], () => {
  deck.value = null;
  const current = source.value;
  if (!current) return;
  if (failed.source !== current) {
    failed.source = current;
    failed.slides = new Set();
  }
  const controller = new AbortController();
  let alive = true;
  let opened: RemotePptxDeck | null = null;
  void openPptxInWorker(current.bytes, { signal: controller.signal }).then((result) => {
    if (result.status === "ok") opened = result.deck;
    if (!alive) {
      opened?.close();
      return;
    }
    if (result.status === "ok") {
      const { width, height, slideCount } = result.deck;
      state.value = { status: "ready", width, height, slideCount };
      deck.value = result.deck;
    } else if (result.status === "failed") {
      state.value = { status: "error", message: t("load.failed"), retry: true };
    } else {
      // Over a cap, too slow, or not a deck the renderer can read: fetching again will not help.
      state.value = { status: "error", message: t("attachment.viewer.previewUnavailable"), retry: false };
    }
  });
  return () => {
    alive = false;
    controller.abort();
    if (opened) {
      retired.add(opened);
      opened.close();
    }
  };
});

useEffectAfterRender([deck, slide], () => {
  const currentDeck = deck.value;
  if (!currentDeck) return;
  if (currentDeck.closed) {
    // A retired deck is about to be replaced by the open effect. Any other closed deck lost its
    // worker while idle, with no render to report it: reopening could repeat without end, so
    // this is a load failure whose retry downloads again.
    if (!retired.has(currentDeck)) state.value = { status: "error", message: t("load.failed"), retry: true };
    return;
  }
  if (failed.slides.has(slide.value)) {
    image.value = { deck: currentDeck, index: slide.value, status: "unavailable" };
    return;
  }
  let alive = true;
  let settled = false;
  let url: string | null = null;
  const index = slide.value;
  currentDeck.render(index).then(
    (rendered) => {
      settled = true;
      if (!alive) return;
      if (rendered.status === "ok") {
        url = URL.createObjectURL(new Blob([rendered.svg], { type: SLIDE_IMAGE_TYPE }));
        image.value = { deck: currentDeck, index, status: "ready", url };
      } else {
        image.value = { deck: currentDeck, index, status: "unavailable" };
      }
    },
    (error: unknown) => {
      settled = true;
      // `closed`: whoever closed the worker also opens the next one, or the viewer is gone.
      if (!alive || (error instanceof PptxWorkerError && error.reason === "closed")) return;
      failed.slides.add(index);
      retired.add(currentDeck);
      image.value = { deck: currentDeck, index, status: "unavailable" };
      // The worker is gone; the other slides need a new one.
      epoch.value += 1;
    },
  );
  return () => {
    alive = false;
    if (!settled && !currentDeck.closed) {
      // Still laying out this slide: only terminating the worker stops it.
      retired.add(currentDeck);
      currentDeck.close();
      epoch.value += 1;
    }
    if (url) URL.revokeObjectURL(url);
  };
});

function retry(): void {
  generation.value += 1;
}

const current = computed(() => {
  const shown = image.value;
  const currentDeck = deck.value;
  return shown && currentDeck && shown.deck === currentDeck && shown.index === slide.value ? shown : null;
});

const slideCount = computed(() => (state.value.status === "ready" ? state.value.slideCount : 0));
const slideWidth = computed(() => (state.value.status === "ready" ? state.value.width : 0));
const slideHeight = computed(() => (state.value.status === "ready" ? state.value.height : 0));
const label = computed(() => t("attachment.viewer.slide", { current: slide.value + 1, total: slideCount.value }));

function onSlideError(): void {
  const shown = current.value;
  if (shown?.status === "ready") {
    image.value = { deck: shown.deck, index: shown.index, status: "unavailable" };
  }
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
  <div
    v-else
    class="attachment-viewer__pane"
    data-pptx-viewer=""
    :data-pptx-slide="slide"
    :data-pptx-slide-state="current?.status ?? 'loading'"
  >
    <ViewerZoomToolbar
      :zoom="zoom"
      :can-zoom-out="zoom > PDF_ZOOM_MIN"
      :can-zoom-in="zoom < PDF_ZOOM_MAX"
      @zoom-in="zoom = zoomIn(zoom)"
      @zoom-out="zoom = zoomOut(zoom)"
      @reset="zoom = 1"
    >
      <UButton size="sm" variant="outline" color="neutral" :disabled="slide <= 0" @click="slide = Math.max(0, slide - 1)">
        {{ t("attachment.viewer.prevSlide") }}
      </UButton>
      <p class="attachment-viewer__page-label">{{ label }}</p>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="slide + 1 >= slideCount"
        @click="slide = Math.min(slideCount - 1, slide + 1)"
      >
        {{ t("attachment.viewer.nextSlide") }}
      </UButton>
    </ViewerZoomToolbar>
    <div class="attachment-viewer__page-wrap">
      <img
        v-if="current?.status === 'ready'"
        :key="current.url"
        :src="current.url"
        :alt="label"
        class="pptx-viewer__slide"
        :width="Math.round(slideWidth * zoom)"
        :height="Math.round(slideHeight * zoom)"
        @error="onSlideError"
      />
      <p v-else-if="current?.status === 'unavailable'" role="alert" class="attachment-viewer__alert">
        {{ t("attachment.viewer.previewUnavailable") }}
      </p>
    </div>
  </div>
</template>
