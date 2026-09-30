<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { computed, type Component, ref, shallowRef } from "vue";
import {
  startViewerPrefetch,
  VIEWER_MAX_BYTES,
  type LayoutKind,
  type ViewerPrefetch,
} from "@/features/attachments/viewer-download";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import ViewerLoadingPane from "./ViewerLoadingPane.vue";

defineOptions({ inheritAttrs: false });

/**
 * Starts the file download and the viewer chunk together, shows the loading
 * pane as ordinary state, and mounts the viewer once its module is in.
 * Not an async component: Vue's default 200 ms delay would hold the viewer
 * (and its download) back the same way React Suspense did. Unmount or a new
 * file aborts the download; key the loader by the file.
 */
const props = defineProps<{ kind: LayoutKind; downloadUrl: string }>();

const viewerLoaders: Record<LayoutKind, () => Promise<{ default: Component }>> = {
  pdf: () => import("./PdfViewer.vue"),
  docx: () => import("./DocxViewer.vue"),
  hwp: () => import("./HwpViewer.vue"),
  pptx: () => import("./PptxViewer.vue"),
  xlsx: () => import("./XlsxViewer.vue"),
};

type LayoutState =
  | { status: "loading" }
  | { status: "error" }
  | { status: "ready"; component: Component; prefetch: ViewerPrefetch };

const generation = ref(0);
const state = shallowRef<LayoutState>({ status: "loading" });

useEffectAfterRender([() => props.kind, () => props.downloadUrl, generation], () => {
  const controller = new AbortController();
  let alive = true;
  state.value = { status: "loading" };
  const prefetch = startViewerPrefetch(props.downloadUrl, VIEWER_MAX_BYTES[props.kind], controller.signal);
  viewerLoaders[props.kind]().then(
    (mod) => {
      if (alive) state.value = { status: "ready", component: mod.default, prefetch };
    },
    () => {
      controller.abort();
      if (alive) state.value = { status: "error" };
    },
  );
  return () => {
    alive = false;
    controller.abort();
  };
});

function retry(): void {
  generation.value += 1;
}

const ready = computed(() => (state.value.status === "ready" ? state.value : null));
</script>

<template>
  <ViewerLoadingPane v-if="state.status === 'loading'" />
  <ViewerErrorPane
    v-else-if="state.status === 'error'"
    :message="t('load.failed')"
    :download-url="downloadUrl"
    retryable
    @retry="retry"
  />
  <component
    :is="ready.component"
    v-else-if="ready"
    :download-url="downloadUrl"
    :prefetch="ready.prefetch"
    v-bind="$attrs"
  />
</template>
