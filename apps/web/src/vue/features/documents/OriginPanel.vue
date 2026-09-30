<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import {
  createOriginProject,
  createTaskFromDocument,
  documentTaskProjectsQuery,
  originErrorText,
  taskOriginsQuery,
} from "@/features/collections/origin-api";
import { originCreateSurface } from "@/features/collections/origin-create-surface";
import { itemPath } from "@/lib/href";

// Tasks this document started, or the documents a task came from
// (features/collections/origin-panel.tsx). Create-task lives on the document
// page only. Links to the other app are full page loads.
const props = defineProps<{
  workspaceId: string;
  slug: string;
  documentId?: string;
  taskId?: string;
  hideWhenEmpty?: boolean;
}>();
const queryClient = useQueryClient();
const after = ref<string | null>(null);
const projectId = ref("");
const title = ref("");
const requestId = ref(crypto.randomUUID());
const projectName = ref("");
const projectKey = ref("");

const origins = useQuery(() =>
  taskOriginsQuery(
    props.workspaceId,
    { documentId: props.documentId, taskId: props.taskId },
    after.value,
  ),
);
const projects = useQuery(() => documentTaskProjectsQuery(props.workspaceId, props.documentId));
const hide = computed(
  () =>
    props.hideWhenEmpty &&
    !origins.isLoading.value &&
    !origins.isError.value &&
    origins.data.value?.count === 0,
);
const heading = computed(() =>
  props.documentId ? t("collection.linkedTasks") : t("collection.sourceDocument"),
);

watch(
  [() => projects.data.value, projectId],
  ([data]) => {
    if (!data) return;
    if (!data.items.some((item) => item.id === projectId.value))
      projectId.value = data.suggestedId ?? "";
  },
  { immediate: true },
);

const createTask = useMutation({
  mutationFn: () => {
    const documentId = props.documentId;
    if (!documentId) throw new Error("Document origin task requires a document ID");
    return createTaskFromDocument(props.workspaceId, documentId, {
      projectId: projectId.value,
      requestId: requestId.value,
      title: title.value.trim(),
    });
  },
  onSuccess: async () => {
    after.value = null;
    title.value = "";
    requestId.value = crypto.randomUUID();
    await queryClient.invalidateQueries({
      queryKey: ["task-origins", props.workspaceId, props.documentId],
    });
    await queryClient.invalidateQueries({ queryKey: ["tasks", props.workspaceId] });
  },
});

const createProject = useMutation({
  mutationFn: () => createOriginProject(props.workspaceId, projectKey.value, projectName.value),
  onSuccess: async (project) => {
    projectId.value = project.id;
    requestId.value = crypto.randomUUID();
    projectName.value = "";
    projectKey.value = "";
    await queryClient.invalidateQueries({
      queryKey: ["task-projects", props.workspaceId, props.documentId],
    });
    await queryClient.invalidateQueries({ queryKey: ["projects", props.workspaceId] });
  },
});

const surface = computed(() =>
  originCreateSurface({
    isLoading: projects.isLoading.value,
    isError: projects.isError.value,
    itemCount: projects.data.value?.items.length,
    canCreateProject: projects.data.value?.canCreateProject,
  }),
);

function preventImeSubmit(event: KeyboardEvent): void {
  if (event.key === "Enter" && event.isComposing) event.preventDefault();
}

// A changed request is a new request: the id makes a retried submit idempotent.
function onProjectChange(event: Event): void {
  projectId.value = (event.target as HTMLSelectElement).value;
  requestId.value = crypto.randomUUID();
}

function onTitleInput(event: Event): void {
  title.value = (event.target as HTMLInputElement).value;
  requestId.value = crypto.randomUUID();
}

function submitTask(): void {
  if (projectId.value && title.value.trim()) createTask.mutate();
}

function submitProject(): void {
  if (projectName.value.trim() && projectKey.value.trim()) createProject.mutate();
}

const fieldClass = "h-10 rounded-md border border-default bg-default px-2";
</script>

<template>
  <section
    v-if="!hide"
    :aria-label="heading"
    class="flex flex-col gap-3 rounded-md border border-default p-4"
  >
    <h2 class="text-lg">{{ heading }} ({{ origins.data.value?.count ?? 0 }})</h2>
    <p v-if="origins.isLoading.value" role="status">{{ t("collection.origins.loading") }}</p>
    <p v-if="origins.isError.value" role="alert">{{
      originErrorText(origins.error.value, t("collection.origins.error"))
    }}</p>
    <a
      v-for="item in origins.data.value?.items ?? []"
      :key="item.taskId"
      class="text-sm underline"
      :href="itemPath(slug, documentId ? item.taskDisplayId : item.documentDisplayId)"
    >
      {{
        documentId
          ? `${item.taskDisplayId} · ${item.taskTitle}`
          : `${item.documentDisplayId} · ${item.documentTitle}`
      }}
    </a>
    <p v-if="origins.data.value && origins.data.value.count === 0">{{
      t("collection.noOrigins")
    }}</p>
    <UButton v-if="after" class="w-fit" variant="outline" color="neutral" @click="after = null">
      {{ t("collection.origins.first") }}
    </UButton>
    <UButton
      v-if="origins.data.value?.nextCursor"
      class="w-fit"
      variant="outline"
      color="neutral"
      @click="after = origins.data.value?.nextCursor ?? null"
    >
      {{ t("collection.origins.next") }}
    </UButton>
    <div v-if="documentId" class="flex flex-col gap-3">
      <p v-if="surface === 'loading'" role="status">{{
        t("collection.taskCreation.projectsLoading")
      }}</p>
      <p v-if="surface === 'error'" role="alert">
        {{ originErrorText(projects.error.value, t("collection.taskCreation.projectsError")) }}
      </p>
      <form
        v-if="surface === 'create-task' && projects.data.value"
        class="flex flex-wrap items-end gap-2"
        @keydown="preventImeSubmit"
        @submit.prevent="submitTask"
      >
        <div class="flex flex-col gap-1">
          <label :for="`origin-project-${documentId}`" class="text-sm font-medium">{{
            t("collection.taskProject")
          }}</label>
          <select
            :id="`origin-project-${documentId}`"
            :class="fieldClass"
            :value="projectId"
            @change="onProjectChange"
          >
            <option
              v-for="project in projects.data.value.items"
              :key="project.id"
              :value="project.id"
            >
              {{ project.name }} ({{ project.key }})
            </option>
          </select>
        </div>
        <div class="flex flex-col gap-1">
          <label :for="`origin-title-${documentId}`" class="text-sm font-medium">{{
            t("collection.taskTitle")
          }}</label>
          <input
            :id="`origin-title-${documentId}`"
            :class="fieldClass"
            :value="title"
            @input="onTitleInput"
          />
        </div>
        <UButton
          type="submit"
          :disabled="!projectId || !title.trim() || createTask.isPending.value"
        >
          {{ t("collection.createTask") }}
        </UButton>
      </form>
      <form
        v-if="surface === 'create-project'"
        class="flex flex-wrap items-end gap-2"
        @keydown="preventImeSubmit"
        @submit.prevent="submitProject"
      >
        <p class="w-full">{{ t("collection.projectRequired") }}</p>
        <div class="flex flex-col gap-1">
          <label :for="`origin-project-name-${documentId}`" class="text-sm font-medium">{{
            t("collection.projectName")
          }}</label>
          <input
            :id="`origin-project-name-${documentId}`"
            v-model="projectName"
            :class="fieldClass"
          />
        </div>
        <div class="flex flex-col gap-1">
          <label :for="`origin-project-key-${documentId}`" class="text-sm font-medium">{{
            t("project.keyLabel")
          }}</label>
          <input
            :id="`origin-project-key-${documentId}`"
            v-model="projectKey"
            :class="fieldClass"
          />
        </div>
        <UButton
          type="submit"
          :disabled="!projectName.trim() || !projectKey.trim() || createProject.isPending.value"
        >
          {{ t("project.new") }}
        </UButton>
      </form>
      <p v-if="surface === 'unavailable'">{{ t("collection.taskCreation.unavailable") }}</p>
      <p v-if="createTask.isError.value" role="alert">
        {{ originErrorText(createTask.error.value, t("collection.taskCreation.error")) }}
      </p>
      <p v-if="createProject.isError.value" role="alert">
        {{ originErrorText(createProject.error.value, t("collection.taskCreation.projectError")) }}
      </p>
    </div>
  </section>
</template>
