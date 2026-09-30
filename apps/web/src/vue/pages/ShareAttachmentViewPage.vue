<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute } from "vue-router";
import { chunkSearch } from "@/features/attachments/attachment-kind";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { shareAttachmentDownloadUrl, shareAttachmentQuery } from "@/lib/queries/share-attachment";
import AttachmentViewer from "../features/attachments/AttachmentViewer.vue";
import "@/features/attachments/attachment-shell.css";
import "@/features/share/share.css";

/**
 * Anonymous `/s/:token/attachments/:attachmentId/view`. Like `/s/:token` it needs no
 * session and only calls `/api/v1/share/{token}/…`. The share is view-only: no
 * preview-html, edit-context or edit-copy, and no workspace chrome.
 */
const route = useRoute();
const token = computed(() => String(route.params.token ?? ""));
const attachmentId = computed(() => String(route.params.attachmentId ?? ""));
const chunk = computed(() => {
  const raw = route.query.chunk;
  const value = typeof raw === "string" ? raw : "";
  return chunkSearch(new URLSearchParams(value === "" ? "" : `chunk=${value}`)).chunk;
});
const query = useQuery(() => shareAttachmentQuery(token.value, attachmentId.value));
const downloadUrl = computed(() => shareAttachmentDownloadUrl(token.value, attachmentId.value));

const notFound = computed(() => query.error.value instanceof ProblemError && query.error.value.status === 404);
const forbidden = computed(() => query.error.value instanceof ProblemError && query.error.value.status === 403);
const retryable = computed(
  () =>
    query.isError.value &&
    !notFound.value &&
    !forbidden.value &&
    (!(query.error.value instanceof ProblemError) || query.error.value.status >= 500),
);
</script>

<template>
  <div class="share-page">
    <main class="share-page__frame share-page__frame--solo">
      <div class="share-page__main">
        <AttachmentViewer
          v-if="query.error.value"
          name=""
          mime=""
          :image="false"
          :download-url="downloadUrl"
          :error="notFound ? t('attachment.error.notFound') : loadErrorMessage(query.error.value)"
          :retryable="retryable"
          @metadata-retry="query.refetch()"
        />
        <p v-else-if="!query.data.value" role="status" class="attachment-viewer__status">
          {{ t("attachment.preview.loading") }}
        </p>
        <AttachmentViewer
          v-else
          :name="query.data.value.name"
          :mime="query.data.value.mime"
          :image="query.data.value.image"
          :download-url="downloadUrl"
          :chunk="chunk"
        />
      </div>
    </main>
  </div>
</template>
