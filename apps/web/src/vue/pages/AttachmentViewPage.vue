<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  attachmentDownloadUrl,
  attachmentPreviewHtmlUrl,
  attachmentViewPath,
  chunkSearch,
  viewerKind,
} from "@/features/attachments/attachment-kind";
import { ProblemError } from "@/lib/api";
import { attachmentEditContextQuery, attachmentQuery } from "@/lib/queries/attachments";
import QueryError from "../components/QueryError.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import AttachmentViewer from "../features/attachments/AttachmentViewer.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/attachments/attachment-shell.css";

const route = useRoute();
const router = useRouter();
const slug = computed(() => String(route.params.slug ?? ""));
const id = computed(() => String(route.params.attachmentId ?? ""));
const chunk = computed(() => {
  const raw = route.query.chunk;
  const value = typeof raw === "string" ? raw : "";
  return chunkSearch(new URLSearchParams(value === "" ? "" : `chunk=${value}`)).chunk;
});

const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");

const query = useQuery(() => attachmentQuery(workspaceId.value, id.value));
const isHwp = computed(() => query.data.value !== undefined && viewerKind(query.data.value) === "hwp");
const editContext = useQuery(() => attachmentEditContextQuery(workspaceId.value, id.value, isHwp.value));

const downloadUrl = computed(() =>
  workspace.value ? attachmentDownloadUrl(workspace.value.id, id.value) : "",
);
const notFound = computed(
  () => query.error.value instanceof ProblemError && (query.error.value.status === 404 || query.error.value.status === 403),
);
const retryable = computed(
  () =>
    query.isError.value &&
    !notFound.value &&
    (!(query.error.value instanceof ProblemError) || query.error.value.status >= 500),
);

function onSavedCopy(copyId: string): void {
  // The copy opens without the original's search chunk.
  void router.push(attachmentViewPath(slug.value, copyId));
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-name="workspace.name" active="wiki">
    <p v-if="query.isLoading.value && !query.data.value" class="attachment-viewer__status">
      {{ t("attachment.preview.loading") }}
    </p>
    <template v-else>
      <AttachmentViewer
        v-if="query.isError.value"
        name=""
        mime=""
        :image="false"
        :download-url="downloadUrl"
        :error="notFound ? t('attachment.error.notFound') : t('load.failed')"
        :retryable="retryable"
        @metadata-retry="query.refetch()"
      />
      <QueryError
        v-if="isHwp && editContext.isError.value"
        :message="t('attachment.viewer.edit.permissionFailed')"
        @retry="editContext.refetch()"
      />
      <AttachmentViewer
        v-if="query.data.value"
        :key="id"
        :name="query.data.value.name"
        :mime="query.data.value.mime"
        :image="query.data.value.image"
        :download-url="downloadUrl"
        :preview-html-url="attachmentPreviewHtmlUrl(workspace.id, id)"
        :chunk="chunk"
        :hwp-edit="
          isHwp
            ? {
                editable: editContext.data.value?.editable === true,
                save: { workspaceId: workspace.id, attachmentId: id, onSavedCopy },
              }
            : undefined
        "
      />
    </template>
  </WorkspaceShell>
</template>
