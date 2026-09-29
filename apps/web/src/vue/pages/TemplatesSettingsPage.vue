<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed } from "vue";
import { useRoute } from "vue-router";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import WorkspaceTemplatesSection from "../features/settings/WorkspaceTemplatesSection.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/settings/settings-shell.css";

const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
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
    active="settings"
  >
    <WorkspaceTemplatesSection :workspace-id="workspace.id" :slug="slug" />
  </WorkspaceShell>
</template>
