<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed } from "vue";
import type { Project, TreeNode } from "@/features/projects/queries";
import { childrenByParent } from "@/features/workspace/wiki-tree";
import { projectTasksPath } from "@/lib/href";
import AppLink from "../../components/AppLink.vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import ProjectDocBranch from "./ProjectDocBranch.vue";
import { projectHomeChildNodes } from "./project-home";
import "@/features/documents/document-shell.css";
import "@/features/projects/projects.css";

const props = withDefaults(
  defineProps<{
    slug: string;
    project: Project;
    nodes: TreeNode[];
    loading: boolean;
    error: string | null;
    creating: boolean;
    canManage?: boolean;
    lifecyclePending?: boolean;
    lifecycleError?: string | null;
  }>(),
  { canManage: false, lifecyclePending: false, lifecycleError: null },
);

const emit = defineEmits<{
  retry: [];
  createDocument: [];
  lifecycle: [action: "archive" | "unarchive" | "delete"];
}>();

const childNodes = computed(() => projectHomeChildNodes(props.nodes, props.project));
const byParent = computed(() => childrenByParent(props.nodes));
const archived = computed(() => props.project.status === "archived");
const canWrite = computed(() => !archived.value && Boolean(props.project.rootDocumentId));

function onArchiveToggle(): void {
  if (archived.value) {
    emit("lifecycle", "unarchive");
    return;
  }
  if (window.confirm(`${t("project.archive.confirm.title")}\n${t("project.archive.confirm.body")}`)) {
    emit("lifecycle", "archive");
  }
}

function onDelete(): void {
  if (window.confirm(`${t("project.delete.confirm.title")}\n${t("project.delete.confirm.body")}`)) {
    emit("lifecycle", "delete");
  }
}
</script>

<template>
  <div class="project-home">
    <div class="project-home__head">
      <div class="project-home__crumb">
        <AppLink :to="projectTasksPath(slug, project.key)">{{ t("nav.tasks") }}</AppLink>
        <span aria-hidden="true"> / </span>
        <span>{{ t("nav.wiki") }}</span>
      </div>
      <h1 class="project-home__title">{{ project.name }}</h1>
      <p class="project-home__meta">
        <span class="project-list__key">{{ project.key }}</span>
        <span v-if="project.visibility === 'private'" class="project-list__private">{{
          t("project.visibility.private")
        }}</span>
        <span v-if="archived" class="project-list__private">{{ t("project.archived.badge") }}</span>
      </p>
    </div>
    <div class="project-home__section-head">
      <h2 class="project-home__section-title">{{ t("nav.wiki") }}</h2>
      <UButton
        v-if="canWrite && childNodes.length > 0"
        type="button"
        variant="outline"
        color="neutral"
        :disabled="creating"
        @click="emit('createDocument')"
      >
        {{ creating ? t("doc.create.pending") : t("nav.newDocument") }}
      </UButton>
    </div>
    <QueryLoading v-if="loading" />
    <QueryError v-if="!loading && error" :message="error" @retry="emit('retry')" />
    <div
      v-if="!loading && !error && childNodes.length === 0"
      class="mx-auto flex w-full max-w-lg flex-1 flex-col items-start justify-center gap-3 px-6 py-12 sm:px-8"
    >
      <p class="break-keep text-title font-semibold">{{ t("doc.empty") }}</p>
      <p v-if="!archived" class="max-w-prose break-keep text-ui leading-relaxed text-muted">{{ t("doc.emptyHint") }}</p>
      <UButton v-if="canWrite" type="button" size="sm" class="mt-2" :disabled="creating" @click="emit('createDocument')">
        {{ creating ? t("doc.create.pending") : t("nav.newDocument") }}
      </UButton>
    </div>
    <div
      v-if="canManage"
      class="project-home__lifecycle flex flex-wrap gap-2"
      data-testid="project-lifecycle"
    >
      <UButton
        type="button"
        variant="outline"
        color="neutral"
        size="sm"
        :disabled="lifecyclePending"
        @click="onArchiveToggle"
      >
        {{ archived ? t("project.unarchive") : t("project.archive") }}
      </UButton>
      <UButton
        type="button"
        variant="outline"
        color="neutral"
        size="sm"
        :disabled="lifecyclePending"
        @click="onDelete"
      >
        {{ t("project.delete") }}
      </UButton>
      <p v-if="lifecycleError" role="alert" class="task-form__alert">{{ lifecycleError }}</p>
    </div>
    <ul v-if="!loading && !error && childNodes.length > 0" class="wiki-tree">
      <ProjectDocBranch
        v-for="node in childNodes"
        :key="node.id"
        :slug="slug"
        :project-key="project.key"
        :node="node"
        :by-parent="byParent"
      />
    </ul>
  </div>
</template>
