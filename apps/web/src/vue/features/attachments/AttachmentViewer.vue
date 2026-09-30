<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { viewerKind } from "@/features/attachments/attachment-kind";
import { publicInstanceQuery } from "@/lib/queries/instance";
import LayoutLoader from "./LayoutLoader.vue";
import SearchChunkSupplement from "./SearchChunkSupplement.vue";
import TextBytesPane from "./TextBytesPane.vue";
import ViewerDownloadButton from "./ViewerDownloadButton.vue";
import ViewerErrorPane from "./ViewerErrorPane.vue";
import type { HwpEditProps } from "./HwpViewer.vue";
import "@/features/attachments/attachment-shell.css";

export type { HwpEditProps };

const props = defineProps<{
  name: string;
  mime: string;
  image: boolean;
  downloadUrl: string;
  error?: string | null;
  /** Offer a retry when fetching metadata again may help (session/share pages). */
  retryable?: boolean;
  chunk?: number | undefined;
  /** Session-only `preview-html` URL for the search-chunk supplement; share views omit it. */
  previewHtmlUrl?: string | undefined;
  /** Session-only HWP/HWPX 간단 편집 (edit-context + save-copy); share views omit it. */
  hwpEdit?: HwpEditProps | undefined;
}>();

const emit = defineEmits<{ metadataRetry: [] }>();

const kind = computed(() => viewerKind({ name: props.name, mime: props.mime, image: props.image }));
const officeKind = computed(() => {
  const current = kind.value;
  return current === "docx" || current === "pptx" || current === "xlsx" ? current : null;
});
const wantsOfficeSupplement = computed(
  () => props.chunk !== undefined && props.previewHtmlUrl !== undefined && officeKind.value !== null,
);

/**
 * HWP/HWPX (source `HwpPane`): the rhwp layout always, plus the search
 * supplement only when the instance extracts on the server (`mode ===
 * "server"`). The mode is read only for a session hit with a chunk, so a
 * share view never calls `/instance` or `preview-html`, and the layout does
 * not wait for it.
 */
const wantsHwpSupplement = computed(() => props.previewHtmlUrl !== undefined && props.chunk !== undefined);
const instance = useQuery(() => ({
  ...publicInstanceQuery,
  enabled: kind.value === "hwp" && wantsHwpSupplement.value,
}));
const hwpServerExtract = computed(() => instance.data.value?.values.attachmentPreview.mode === "server");
</script>

<template>
  <div data-attachment-viewer="" class="attachment-viewer">
    <header class="attachment-viewer__head">
      <p class="attachment-viewer__name">{{ name }}</p>
      <ViewerDownloadButton :href="downloadUrl" />
    </header>
    <div class="attachment-viewer__stage">
      <ViewerErrorPane
        v-if="error"
        :message="error"
        :download-url="downloadUrl"
        :retryable="retryable"
        @retry="emit('metadataRetry')"
      />
      <div
        v-else-if="kind === 'download'"
        class="attachment-viewer__pane attachment-viewer__pane--center"
      >
        <ViewerErrorPane :message="t('attachment.viewer.previewUnavailable')" :download-url="downloadUrl" />
      </div>
      <div v-else-if="kind === 'image'" class="attachment-viewer__pane">
        <img class="attachment-viewer__image" :src="downloadUrl" :alt="name" />
      </div>
      <LayoutLoader v-else-if="kind === 'pdf'" :key="downloadUrl" kind="pdf" :download-url="downloadUrl" />
      <template v-else-if="kind === 'hwp'">
        <SearchChunkSupplement
          v-if="wantsHwpSupplement && hwpServerExtract && previewHtmlUrl !== undefined && chunk !== undefined"
          :preview-html-url="previewHtmlUrl"
          :chunk="chunk"
        />
        <LayoutLoader
          :key="downloadUrl"
          kind="hwp"
          :download-url="downloadUrl"
          :name="name"
          :chunk="chunk"
          :edit="hwpEdit"
        />
      </template>
      <template v-else-if="officeKind">
        <SearchChunkSupplement
          v-if="wantsOfficeSupplement && previewHtmlUrl !== undefined && chunk !== undefined"
          :preview-html-url="previewHtmlUrl"
          :chunk="chunk"
        />
        <LayoutLoader :key="`${officeKind}:${downloadUrl}`" :kind="officeKind" :download-url="downloadUrl" />
      </template>
      <TextBytesPane v-else :download-url="downloadUrl" :chunk="chunk" />
    </div>
  </div>
</template>
