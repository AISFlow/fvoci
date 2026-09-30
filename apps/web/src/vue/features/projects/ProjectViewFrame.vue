<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed } from "vue";
import { useRoute } from "vue-router";
import type { ProjectListItem } from "@/features/projects/queries";
import { loadErrorMessage } from "@/lib/api";
import type { components } from "@/generated/api";
import { projectPath, projectsPath } from "@/lib/href";
import ProjectViewTabs, { type ProjectViewTab } from "../../components/ProjectViewTabs.vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import WorkspaceShell from "../../components/WorkspaceShell.vue";
import { useTaskStream } from "../../composables/useTaskStream";
import { useProjectRef } from "../../session/useProjectRef";
import { useWorkspaceSession } from "../../session/useWorkspaceSession";
import "@/features/projects/projects.css";

type Workspace = components["schemas"]["WorkspaceListItemResponse"];

// The frame of a project view page (`/w/:slug/:ref/<view>`), as the React
// project pages draw it: the workspace session guards, the shell, the
// project resolved from the ref (not found, or a failed list with a retry),
// the crumb, the title and the view tabs. It holds the project's task stream,
// so the page's task queries follow peers' changes.
defineProps<{ active: ProjectViewTab }>();
defineSlots<{
  default(props: { slug: string; workspace: Workspace; project: ProjectListItem }): unknown;
}>();

const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
const refParam = computed(() => String(route.params.ref ?? ""));

const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const projectRef = useProjectRef(() => workspace.value?.id, refParam);
const { project, notFound } = projectRef;
useTaskStream(
  () => workspace.value?.id,
  () => project.value?.id,
);
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
  >
    <p v-if="notFound" role="alert" class="task-form__alert">{{ t("project.notFound") }}</p>
    <QueryError
      v-else-if="projectRef.failed.value"
      :message="loadErrorMessage(projectRef.projects.error.value)"
      @retry="projectRef.retry()"
    />
    <div v-else-if="project" class="task-home">
      <p class="task-home__crumb">
        <a :href="projectsPath(slug)">{{ t("nav.projects") }}</a>
        <span aria-hidden="true"> / </span>
        <a :href="projectPath(slug, project.key)">{{ project.key }}</a>
      </p>
      <div class="task-home__head">
        <h1 class="task-home__title">{{ project.name }}</h1>
      </div>
      <ProjectViewTabs :slug="slug" :project-key="project.key" :active="active" />
      <slot :slug="slug" :workspace="workspace" :project="project" />
    </div>
    <QueryLoading v-else />
  </WorkspaceShell>
</template>
