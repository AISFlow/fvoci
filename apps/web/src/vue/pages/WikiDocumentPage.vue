<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, watchEffect } from "vue";
import { useRoute } from "vue-router";
import { wikiPath } from "@/lib/href";
import { collabRoomName } from "../collab/useCollabRoom";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import WikiDocumentView from "../features/documents/WikiDocumentView.vue";
import { redirectTo } from "../session/navigation";
import { useWikiDocumentRef } from "../session/useWikiDocumentRef";
import { useWorkspaceSession } from "../session/useWorkspaceSession";

// `/w/:slug/WIKI-<n>`. The document view is keyed by its collab room, so an
// in-app move to another document tears the room down (flush, then socket
// and provider) and builds the next one; there is never a second provider.
const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
const refParam = computed(() => String(route.params.ref ?? ""));

const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const documentRef = useWikiDocumentRef(() => workspace.value?.id, refParam);
const node = documentRef.node;

// A ref that names no readable wiki document goes to the wiki list, as in the React app.
watchEffect(() => {
  if (workspace.value && documentRef.notFound.value) redirectTo(wikiPath(slug.value));
});
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell
    v-else-if="workspace"
    :slug="slug"
    :workspace-id="workspace.id"
    :workspace-name="workspace.name"
    active="wiki"
  >
    <div v-if="documentRef.failed.value">
      <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
      <UButton size="sm" class="mt-2" @click="documentRef.retry()">{{ t("load.retry") }}</UButton>
    </div>
    <WikiDocumentView
      v-else-if="node && !documentRef.notFound.value"
      :key="collabRoomName(workspace.id, 'document', node.id)"
      :workspace-id="workspace.id"
      :slug="slug"
      :document-id="node.id"
    />
    <p v-else role="status" class="text-muted">{{ t("load.loading") }}</p>
  </WorkspaceShell>
</template>
