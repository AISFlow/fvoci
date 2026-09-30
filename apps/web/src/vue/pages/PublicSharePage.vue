<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, nextTick, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { ProblemError } from "@/lib/api";
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
const queryClient = useQueryClient();
const refreshing = ref(false);
const token = computed(() => String(route.params.token ?? ""));
const selectedDocumentId = ref<string | null>(null);
const accessError = ref<string | null>(null);
let queryGeneration = 0;
let recoveryGeneration = 0;
let denialGeneration = 0;
watch(
  [token, selectedDocumentId],
  () => {
    queryGeneration += 1;
  },
  { flush: "sync" },
);
watch(
  token,
  () => {
    recoveryGeneration += 1;
    refreshing.value = false;
    selectedDocumentId.value = null;
    accessError.value = null;
  },
  { flush: "sync" },
);
const meta = useQuery(() => ({ ...sharePublicMetaQuery(token.value), staleTime: 0 }));
const tree = useQuery(() => ({
  ...sharePublicTreeQuery(token.value, meta.isSuccess.value),
  staleTime: 0,
}));
const body = useQuery(() => ({
  ...sharePublicBodyQuery(token.value, selectedDocumentId.value, meta.isSuccess.value),
  staleTime: 0,
}));

// Once a current body read is denied, no other cached share content remains visible.
watch(
  body.error,
  (error) => {
    if (error instanceof ProblemError && error.status === 404) onDenied(error);
  },
  { flush: "sync", immediate: true },
);

const share = computed(() => meta.data.value);
const metaError = computed(() => (meta.error.value ? failMessage(meta.error.value) : null));
const treeError = computed(() => (tree.error.value ? failMessage(tree.error.value) : null));
const bodyError = computed(() => (body.error.value ? failMessage(body.error.value) : null));
const activeDocumentId = computed(
  () => selectedDocumentId.value ?? share.value?.documentId ?? null,
);

function onSelectDocument(documentId: string): void {
  const rootId = share.value?.documentId;
  selectedDocumentId.value = documentId === rootId ? null : documentId;
}

function onDenied(error: unknown): void {
  // Repeated denials have the same message but must invalidate older recovery.
  denialGeneration += 1;
  accessError.value = failMessage(error);
}

async function refresh(): Promise<void> {
  const recovering = accessError.value !== null;
  const refreshToken = token.value;
  const recovery = ++recoveryGeneration;
  const denial = denialGeneration;
  let query = queryGeneration;
  const isCurrent = () =>
    token.value === refreshToken &&
    recovery === recoveryGeneration &&
    denial === denialGeneration &&
    query === queryGeneration;
  refreshing.value = true;
  try {
    const result = await meta.refetch();
    if (isCurrent() && result.isSuccess && meta.isSuccess.value && !meta.error.value) {
      // A denied child may have moved outside the share. Reauthorize the root,
      // keeping the denial gate until its fresh body and current tree succeed.
      if (recovering) selectedDocumentId.value = null;
      query = queryGeneration;
      await nextTick();
      if (!isCurrent()) return;
      const [freshTree, freshBody] = await Promise.all([
        tree.refetch(),
        body.refetch(),
        queryClient.refetchQueries({ queryKey: ["share-search", refreshToken], type: "active" }),
      ]);
      // Refetch results are snapshots. Reconnect can deny the root again while
      // the earlier tree is pending; only current, settled queries may reopen it.
      if (
        recovering &&
        isCurrent() &&
        selectedDocumentId.value === null &&
        freshTree.isSuccess &&
        freshBody.isSuccess &&
        meta.isSuccess.value &&
        tree.isSuccess.value &&
        body.isSuccess.value &&
        !meta.error.value &&
        !tree.error.value &&
        !body.error.value &&
        !meta.isFetching.value &&
        !tree.isFetching.value &&
        !body.isFetching.value
      ) {
        accessError.value = null;
      }
    }
  } finally {
    if (recovery === recoveryGeneration) refreshing.value = false;
  }
}
</script>

<template>
  <div v-if="metaError || treeError || accessError" class="share-page" data-public-share="vue">
    <div class="share-page__gate">
      <div class="flex flex-col gap-3">
        <p role="alert" class="share-page__alert">{{ metaError || treeError || accessError }}</p>
        <UButton variant="outline" color="neutral" :loading="refreshing" @click="refresh">
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
    :body-loading="body.isFetching.value"
    :body-error="bodyError"
    :refreshing="
      refreshing || meta.isFetching.value || tree.isFetching.value || body.isFetching.value
    "
    @select-document="onSelectDocument"
    @retry-body="() => void body.refetch()"
    @refresh="refresh"
    @denied="onDenied"
  />
</template>
