<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { useRoute } from "vue-router";
import {
  sharePublicBodyQuery,
  sharePublicMetaQuery,
  sharePublicTreeQuery,
} from "@/lib/queries/share";
import PublicShareView from "../features/share/PublicShareView.vue";
import { failMessage } from "../features/share/public-share-fail";
import "@/features/share/share.css";

/**
 * Anonymous `/s/:token` reader. It never needs a session and only calls
 * `/api/v1/share/{token}/…` (no `/workspaces`, `/auth/me` or setup guard).
 * Tree and body stay disabled until meta succeeds, matching React `meta.isSuccess`.
 * The boot module still sends this path to React (src/app-boundary.ts).
 * `/s/:token/attachments/...` is a different page (#275).
 */
const route = useRoute();
const token = computed(() => String(route.params.token ?? ""));
const selectedDocumentId = ref<string | null>(null);
const meta = useQuery(() => sharePublicMetaQuery(token.value));
const tree = useQuery(() => sharePublicTreeQuery(token.value, meta.isSuccess.value));
const body = useQuery(() =>
  sharePublicBodyQuery(token.value, selectedDocumentId.value, meta.isSuccess.value),
);

const share = computed(() => meta.data.value);
const metaError = computed(() => (meta.error.value ? failMessage(meta.error.value) : null));
const bodyError = computed(() => (body.error.value ? failMessage(body.error.value) : null));
const activeDocumentId = computed(() => selectedDocumentId.value ?? share.value?.documentId ?? null);

function onSelectDocument(documentId: string): void {
  const rootId = share.value?.documentId;
  selectedDocumentId.value = documentId === rootId ? null : documentId;
}
</script>

<template>
  <div v-if="metaError" class="share-page">
    <div class="share-page__gate">
      <p role="alert" class="share-page__alert">{{ metaError }}</p>
    </div>
  </div>
  <div v-else-if="!share" class="share-page">
    <div class="share-page__gate">
      <p role="status" class="share-page__status">{{ t("doc.loading") }}</p>
    </div>
  </div>
  <PublicShareView
    v-else
    :token="token"
    :title="share.title"
    :expires-at="share.expiresAt"
    :tree="tree.data.value?.items ?? []"
    :active-document-id="activeDocumentId"
    :body="body.data.value ?? null"
    :body-loading="body.isLoading.value"
    :body-error="bodyError"
    @select-document="onSelectDocument"
    @retry-body="() => void body.refetch()"
  />
</template>
