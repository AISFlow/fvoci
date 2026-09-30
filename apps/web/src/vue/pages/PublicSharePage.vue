<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, watch } from "vue";
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
 * `/s/:token/attachments/...` has its own anonymous viewer route.
 */
const route = useRoute();
const token = computed(() => String(route.params.token ?? ""));
const selectedDocumentId = ref<string | null>(null);
watch(token, () => { selectedDocumentId.value = null; }, { flush: "sync" });
const meta = useQuery(() => sharePublicMetaQuery(token.value));
const tree = useQuery(() => sharePublicTreeQuery(token.value, meta.isSuccess.value));
const body = useQuery(() =>
  sharePublicBodyQuery(token.value, selectedDocumentId.value, meta.isSuccess.value),
);

const share = computed(() => meta.data.value);
const metaError = computed(() => (meta.error.value ? failMessage(meta.error.value) : null));
const treeError = computed(() => (tree.error.value ? failMessage(tree.error.value) : null));
const bodyError = computed(() => (body.error.value ? failMessage(body.error.value) : null));
const activeDocumentId = computed(() => selectedDocumentId.value ?? share.value?.documentId ?? null);

function onSelectDocument(documentId: string): void {
  const rootId = share.value?.documentId;
  selectedDocumentId.value = documentId === rootId ? null : documentId;
}

async function refresh(): Promise<void> {
  const result = await meta.refetch();
  if (result.isSuccess) await Promise.all([tree.refetch(), body.refetch()]);
}
</script>

<template>
  <div v-if="metaError || treeError" class="share-page" data-public-share="vue">
    <div class="share-page__gate">
      <div class="flex flex-col gap-3">
        <p role="alert" class="share-page__alert">{{ metaError || treeError }}</p>
        <UButton variant="outline" color="neutral" :loading="meta.isFetching.value || tree.isFetching.value" @click="refresh">
          {{ t("load.retry") }}
        </UButton>
      </div>
    </div>
  </div>
  <div v-else-if="!share" class="share-page" data-public-share="vue">
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
    :refreshing="meta.isFetching.value || tree.isFetching.value || body.isFetching.value"
    @select-document="onSelectDocument"
    @retry-body="() => void body.refetch()"
    @refresh="refresh"
  />
</template>
