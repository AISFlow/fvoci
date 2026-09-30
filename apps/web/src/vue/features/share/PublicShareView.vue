<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref } from "vue";
import { downloadSharePdf, type ShareTreeNode } from "@/lib/queries/share";
import { shareTreeRoots } from "@/lib/share-links";
import PublicTreeBranch from "./PublicTreeBranch.vue";
import ShareBodyView from "./ShareBodyView.vue";
import PublicShareSearch from "./PublicShareSearch.vue";
import type { ProblemError } from "@/lib/api";

const props = defineProps<{
  token: string;
  title: string;
  expiresAt: string;
  tree: readonly ShareTreeNode[];
  activeDocumentId: string | null;
  body: string | null;
  bodyLoading: boolean;
  bodyError: string | null;
  refreshing: boolean;
}>();

const emit = defineEmits<{
  selectDocument: [documentId: string];
  retryBody: [];
  refresh: [];
  denied: [error: ProblemError];
}>();

const dateFormat = new Intl.DateTimeFormat("ko", {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});

function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : dateFormat.format(date);
}

const roots = computed(() => shareTreeRoots(props.tree));
const heading = computed(
  () => props.tree.find((node) => node.id === props.activeDocumentId)?.title ?? props.title,
);
const hasTree = computed(() => props.tree.length > 1);
const pdfPending = ref(false);

function exportPdf(): void {
  pdfPending.value = true;
  void downloadSharePdf(props.token, heading.value, props.activeDocumentId)
    .catch(() => {
      window.alert(t("export.pdf.failed"));
    })
    .finally(() => {
      pdfPending.value = false;
    });
}
</script>

<template>
  <div class="share-page" data-public-share="vue">
    <div :class="['share-page__frame', hasTree ? null : 'share-page__frame--solo']">
      <aside v-if="hasTree" class="share-page__rail">
        <nav :aria-label="t('share.document')" class="share-page__tree">
          <ul>
            <PublicTreeBranch
              v-for="node in roots"
              :key="node.id"
              :nodes="tree"
              :node="node"
              :depth="0"
              :active-document-id="activeDocumentId"
              @select="emit('selectDocument', $event)"
            />
          </ul>
        </nav>
      </aside>
      <main class="share-page__main">
        <div class="flex flex-wrap items-start justify-between gap-3">
          <div class="min-w-0">
            <h1 class="share-page__heading">{{ heading }}</h1>
            <p class="share-page__meta">
              <span class="share-page__badge">{{ t("doc.readOnly") }}</span>
              <span>{{ t("share.expires") }} {{ formatDate(expiresAt) }}</span>
            </p>
          </div>
          <div class="flex flex-wrap gap-2">
            <UButton
              type="button"
              variant="ghost"
              color="neutral"
              size="sm"
              :loading="refreshing"
              @click="emit('refresh')"
            >
              {{ t("load.retry") }}
            </UButton>
            <UButton
              type="button"
              variant="outline"
              color="neutral"
              size="sm"
              :disabled="pdfPending || bodyLoading || Boolean(bodyError)"
              @click="exportPdf"
            >
              {{ t("export.pdf") }}
            </UButton>
          </div>
        </div>
        <PublicShareSearch
          :token="token"
          @select-document="emit('selectDocument', $event)"
          @denied="emit('denied', $event)"
        />
        <p v-if="bodyLoading" role="status" class="share-page__status">
          {{ t("doc.loading") }}
        </p>
        <div v-if="bodyError" class="flex flex-wrap items-center gap-3">
          <p role="alert" class="share-page__alert">{{ bodyError }}</p>
          <UButton
            type="button"
            size="sm"
            variant="outline"
            color="neutral"
            @click="emit('retryBody')"
          >
            {{ t("load.retry") }}
          </UButton>
        </div>
        <ShareBodyView v-if="body !== null && !bodyLoading && !bodyError" :html="body" />
      </main>
    </div>
  </div>
</template>
