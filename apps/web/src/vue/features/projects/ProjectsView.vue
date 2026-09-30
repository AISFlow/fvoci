<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, useId } from "vue";
import type { CloneProjectBody, CreateProjectBody, ProjectListItem } from "@/features/projects/queries";
import { projectTasksPath } from "@/lib/href";
import type { components } from "@/generated/api";
import NativeModal from "../../components/NativeModal.vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import CloneProjectForm from "./CloneProjectForm.vue";
import CreateProjectForm from "./CreateProjectForm.vue";
import "@/features/projects/projects.css";

type Member = components["schemas"]["MemberResponse"];

const props = defineProps<{
  slug: string;
  projects: readonly ProjectListItem[];
  members: readonly Member[];
  currentUserId: string | null;
  loading: boolean;
  error: string | null;
  creating?: boolean;
  cloning?: boolean;
  onRetry: () => void;
  onCreate: (input: CreateProjectBody) => Promise<void>;
  onClone: (projectId: string, input: CloneProjectBody) => Promise<void>;
}>();

const titleId = useId();
const cloneTitleId = useId();
const createOpen = ref(false);
const cloneSource = ref<ProjectListItem | null>(null);
const active = computed(() => props.projects.filter((project) => project.status === "active"));
const archived = computed(() => props.projects.filter((project) => project.status === "archived"));

function openCreate(): void {
  createOpen.value = true;
}

function closeCreate(): void {
  createOpen.value = false;
}

function closeClone(): void {
  cloneSource.value = null;
}

async function submitCreate(input: CreateProjectBody): Promise<void> {
  await props.onCreate(input);
  closeCreate();
}

async function submitClone(input: CloneProjectBody): Promise<void> {
  const source = cloneSource.value;
  if (!source) return;
  await props.onClone(source.id, input);
  closeClone();
}

function onCloneClick(project: ProjectListItem, event: Event): void {
  event.preventDefault();
  event.stopPropagation();
  cloneSource.value = project;
}
</script>

<template>
  <div class="project-home">
    <div class="project-home__head">
      <h1 class="project-home__title">{{ t("nav.projects") }}</h1>
      <UButton v-if="projects.length > 0" type="button" @click="openCreate">{{ t("project.new") }}</UButton>
    </div>
    <QueryLoading v-if="loading" />
    <QueryError v-else-if="error" :message="error" @retry="onRetry" />
    <div
      v-else-if="active.length === 0"
      class="mx-auto flex w-full max-w-lg flex-1 flex-col items-start justify-center gap-3 px-6 py-12"
    >
      <p class="break-keep text-lg font-semibold">
        {{ projects.length === 0 ? t("project.emptyHint") : t("project.emptyArchived") }}
      </p>
      <UButton type="button" size="sm" :disabled="creating" @click="openCreate">{{ t("project.new") }}</UButton>
    </div>
    <ul v-else class="project-list">
      <li v-for="project in active" :key="project.id">
        <a :href="projectTasksPath(slug, project.key)" class="project-list__row">
          <span class="project-list__key">{{ project.key }}</span>
          <span class="project-list__meta">
            <span class="project-list__name">{{ project.name }}</span>
            <span v-if="project.visibility === 'private'" class="project-list__private">{{
              t("project.visibility.private")
            }}</span>
          </span>
          <span class="project-list__counts">
            <span class="project-list__count">{{ t("entrance.documentCount", { count: project.documentCount ?? "—" }) }}</span>
            <span class="project-list__count">{{ t("entrance.openTaskCount", { count: project.openTaskCount }) }}</span>
          </span>
          <UButton
            type="button"
            variant="outline"
            color="neutral"
            class="project-list__clone"
            @click="onCloneClick(project, $event)"
          >
            {{ t("project.clone") }}
          </UButton>
        </a>
      </li>
    </ul>
    <ul v-if="!loading && !error && archived.length > 0" class="project-list" :aria-label="t('project.archived.badge')">
      <li v-for="project in archived" :key="project.id">
        <a :href="projectTasksPath(slug, project.key)" class="project-list__row">
          <span class="project-list__key">{{ project.key }}</span>
          <span class="project-list__meta">
            <span class="project-list__name">{{ project.name }}</span>
            <span>{{ t("project.archived.badge") }}</span>
            <span v-if="project.visibility === 'private'" class="project-list__private">{{
              t("project.visibility.private")
            }}</span>
          </span>
          <span class="project-list__counts">
            <span class="project-list__count">{{ t("entrance.documentCount", { count: project.documentCount ?? "—" }) }}</span>
            <span class="project-list__count">{{ t("entrance.openTaskCount", { count: project.openTaskCount }) }}</span>
          </span>
          <UButton
            type="button"
            variant="outline"
            color="neutral"
            class="project-list__clone"
            @click="onCloneClick(project, $event)"
          >
            {{ t("project.clone") }}
          </UButton>
        </a>
      </li>
    </ul>
    <NativeModal :open="createOpen" :labelled-by="titleId" @close="closeCreate">
      <h2 :id="titleId" class="project-dialog__title">{{ t("project.new") }}</h2>
      <CreateProjectForm
        :pending="creating"
        :members="members"
        :current-user-id="currentUserId"
        :on-submit="submitCreate"
        :on-cancel="closeCreate"
      />
    </NativeModal>
    <NativeModal :open="cloneSource !== null" :labelled-by="cloneTitleId" @close="closeClone">
      <template v-if="cloneSource">
        <h2 :id="cloneTitleId" class="project-dialog__title">{{ t("project.clone.title") }}</h2>
        <CloneProjectForm
          :source="cloneSource"
          :pending="cloning"
          :members="members"
          :current-user-id="currentUserId"
          :on-submit="submitClone"
          :on-cancel="closeClone"
        />
      </template>
    </NativeModal>
  </div>
</template>
