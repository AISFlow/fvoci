<script setup lang="ts">
import { useNavigationError } from "../features/workspace/useNavigationError";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, watchEffect } from "vue";
import { useRoute, useRouter } from "vue-router";
import { itemPath, parseRef, projectPath } from "@/lib/href";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";

const route = useRoute();
const router = useRouter();
const navigation = useNavigationError(() => route.fullPath);
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const parsed = computed(() => parseRef(String(route.params.ref ?? "")));

watchEffect(() => {
  if (session.status.value !== "ready" || !workspace.value || !parsed.value) return;
  const path =
    parsed.value.kind === "item"
      ? itemPath(slug.value, parsed.value.displayId)
      : projectPath(slug.value, parsed.value.key);
  // Preserve the original encoded query and fragment during canonicalization.
  navigation.run(() => router.replace({ path, query: route.query, hash: route.hash }));
});
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{
    t("load.loading")
  }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell
    v-else-if="workspace"
    :slug="slug"
    :workspace-id="workspace.id"
    :workspace-name="workspace.name"
    active="projects"
  >
    <p v-if="!parsed" role="alert" class="task-form__alert">{{ t("error.resource.notFound") }}</p>
    <p v-else-if="navigation.error.value" role="alert" class="task-form__alert">{{
      navigation.error.value
    }}</p>
    <p v-else role="status" class="text-muted">{{ t("load.loading") }}</p>
  </WorkspaceShell>
</template>
