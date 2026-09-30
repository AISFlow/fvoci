<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute } from "vue-router";
import { projectsQuery } from "@/features/projects/queries";
import { loadErrorMessage } from "@/lib/api";
import { recentQuery, starsQuery } from "@/lib/queries/share";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import WorkspaceEntrance from "../features/projects/WorkspaceEntrance.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/share/share.css";
import "@/features/projects/projects.css";

const dayFormat = new Intl.DateTimeFormat("ko", { month: "numeric", day: "numeric" });

const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");

const stars = useQuery(() => starsQuery(workspaceId.value));
const recent = useQuery(() => recentQuery(workspaceId.value, 8));
const projects = useQuery(() => projectsQuery(workspaceId.value));

const projectKeyById = computed(
  () => new Map((projects.data.value?.items ?? []).map((project) => [project.id, project.key] as const)),
);

const starItems = computed(() =>
  (stars.data.value?.items ?? []).map((star) => ({
    key: `${star.type}:${star.targetId}`,
    type: star.type,
    projectId: star.projectId,
    number: star.number,
    title: star.title,
  })),
);

const recentItems = computed(() =>
  (recent.data.value?.items ?? []).map((item) => ({
    key: `${item.type}:${item.id}`,
    type: item.type,
    projectId: item.projectId,
    number: item.number,
    title: item.title,
    meta: dayFormat.format(new Date(item.updatedAt)),
  })),
);
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-id="workspace.id" :workspace-name="workspace.name" active="home">
    <div class="entrance">
      <h1 class="project-home__title">{{ workspace.name }}</h1>
      <WorkspaceEntrance
        :slug="slug"
        :heading="t('star.home')"
        :empty="t('entrance.stars.emptyHint')"
        :items="starItems"
        :loading="stars.isLoading.value"
        :error="stars.isError.value ? loadErrorMessage(stars.error.value) : null"
        :on-retry="() => void stars.refetch()"
        :project-key-by-id="projectKeyById"
      />
      <WorkspaceEntrance
        :slug="slug"
        :heading="t('recent.title')"
        :empty="t('recent.empty')"
        :items="recentItems"
        :loading="recent.isLoading.value"
        :error="recent.isError.value ? loadErrorMessage(recent.error.value) : null"
        :on-retry="() => void recent.refetch()"
        :project-key-by-id="projectKeyById"
      />
    </div>
  </WorkspaceShell>
</template>
