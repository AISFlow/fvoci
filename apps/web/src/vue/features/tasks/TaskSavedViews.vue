<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId, watch } from "vue";
import {
  PROJECT_VIEW_TYPES,
  projectViewTypeLabel,
  viewConfigOf,
  type ProjectViewType,
} from "@/features/tasks/project-views";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { asJsonObject, projectViewsQuery, type ProjectView } from "@/lib/queries/collections";
import { viewQueriesEqual, type ViewQuery } from "@/lib/view-query";
import ConfirmActionButton from "../../components/ConfirmActionButton.vue";
import NativeModal from "../../components/NativeModal.vue";
import "@/features/projects/projects.css";
import "@/features/collections/collections.css";

const props = defineProps<{
  workspaceId: string;
  projectId: string;
  query: ViewQuery;
  selectedId: string | null;
}>();
const emit = defineEmits<{ select: [view: ProjectView | null] }>();

const queryClient = useQueryClient();
const selectId = useId();
const dialogTitleId = useId();
const nameId = useId();
const typeId = useId();
const renameId = useId();
const viewsKey = computed(() => projectViewsQuery(props.workspaceId, props.projectId).queryKey);
const views = useQuery(() => projectViewsQuery(props.workspaceId, props.projectId));
const items = computed(() => views.data.value ?? []);
const selected = computed(() => items.value.find((view) => view.id === props.selectedId));
const createOpen = ref(false);
const name = ref("");
const type = ref<ProjectViewType>("list");
const conflict = ref(false);
const error = ref<string | null>(null);
const changed = computed(
  () => selected.value !== undefined && !viewQueriesEqual(viewConfigOf(selected.value), props.query),
);

function failure(err: unknown): string {
  return err instanceof ProblemError && err.titleKnown ? err.title : t("task.savedView.failed");
}

const create = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/views", {
        params: { path: { workspace_id: props.workspaceId, project_id: props.projectId } },
        body: { name: name.value.trim(), type: type.value, config: asJsonObject(props.query) },
      }),
    ),
  onMutate: () => {
    error.value = null;
  },
  onError: (err) => {
    error.value = failure(err);
  },
  onSuccess: async (created) => {
    queryClient.setQueryData(viewsKey.value, (previous: ProjectView[] = []) => [...previous, created]);
    await queryClient.invalidateQueries({ queryKey: viewsKey.value });
    createOpen.value = false;
    name.value = "";
    conflict.value = false;
    emit("select", created);
  },
});

const update = useMutation({
  mutationFn: async (input: { view: ProjectView; name?: string; config?: ViewQuery }) =>
    ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}/views/{view_id}", {
        params: { path: { workspace_id: props.workspaceId, view_id: input.view.id } },
        body: {
          ...(input.name !== undefined ? { name: input.name } : {}),
          ...(input.config !== undefined
            ? {
                config: asJsonObject(input.config),
                expectedConfig: asJsonObject(viewConfigOf(input.view)),
              }
            : {}),
        },
      }),
    ),
  onMutate: () => {
    error.value = null;
  },
  onError: async (err) => {
    if (err instanceof ProblemError && err.status === 409) {
      const refreshed = await views.refetch();
      conflict.value = refreshed.isSuccess;
      return;
    }
    error.value = failure(err);
  },
  onSuccess: async () => {
    conflict.value = false;
    await queryClient.invalidateQueries({ queryKey: viewsKey.value });
  },
});

const remove = useMutation({
  mutationFn: async (view: ProjectView) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/views/{view_id}", {
        params: { path: { workspace_id: props.workspaceId, view_id: view.id } },
      }),
    ),
  onMutate: () => {
    error.value = null;
  },
  onError: (err) => {
    error.value = failure(err);
  },
  onSuccess: async (_ok, view) => {
    queryClient.setQueryData(viewsKey.value, (previous: ProjectView[] = []) =>
      previous.filter((item) => item.id !== view.id),
    );
    await queryClient.invalidateQueries({ queryKey: viewsKey.value });
    if (view.id === props.selectedId) emit("select", null);
  },
});

const pending = computed(() => create.isPending.value || update.isPending.value || remove.isPending.value);

watch(
  () => props.selectedId,
  () => {
    conflict.value = false;
    error.value = null;
  },
);

function onSelectChange(event: Event): void {
  conflict.value = false;
  error.value = null;
  emit("select", items.value.find((view) => view.id === (event.target as HTMLSelectElement).value) ?? null);
}

function onCreateType(event: Event): void {
  const next = PROJECT_VIEW_TYPES.find((value) => value === (event.target as HTMLSelectElement).value);
  if (next) type.value = next;
}

function onRename(event: Event): void {
  event.preventDefault();
  const current = selected.value;
  if (!current) return;
  const input = (event.currentTarget as HTMLFormElement).elements.namedItem("view-name");
  const next = input instanceof HTMLInputElement ? input.value.trim() : "";
  if (next === "" || next === current.name) return;
  update.mutate({ view: current, name: next });
}

function openCreate(): void {
  create.reset();
  error.value = null;
  createOpen.value = true;
}

function onCreateSubmit(): void {
  if (!name.value.trim() || create.isPending.value) return;
  create.mutate();
}

async function deleteSelected(): Promise<void> {
  const current = selected.value;
  if (!current) return;
  try {
    await remove.mutateAsync(current);
  } catch {
    /* onError shows the message. */
  }
}
</script>

<template>
  <div class="flex flex-col gap-2" data-testid="task-saved-views">
    <div class="collection-toolbar">
      <div class="collection-field">
        <label :for="selectId">{{ t("project.views") }}</label>
        <select
          :id="selectId"
          class="collection-select"
          :aria-label="t('task.savedView.select')"
          :value="selected?.id ?? ''"
          :disabled="views.isPending.value"
          @change="onSelectChange"
        >
          <option value="">{{ t("task.savedView.current") }}</option>
          <option v-for="view in items" :key="view.id" :value="view.id">
            {{ view.name }} · {{ projectViewTypeLabel(view.type) }}
          </option>
        </select>
      </div>
      <UButton size="sm" variant="outline" color="neutral" :disabled="pending" @click="openCreate">
        {{ t("task.savedView.create") }}
      </UButton>
      <template v-if="changed && selected">
        <span class="text-caption text-muted">{{ t("task.savedView.unsaved") }}</span>
        <UButton
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="pending || conflict"
          @click="update.mutate({ view: selected, config: query })"
        >
          {{ t("task.savedView.update") }}
        </UButton>
      </template>
    </div>
    <div v-if="selected" class="collection-toolbar">
      <form :key="`${selected.id}:${selected.name}`" class="collection-toolbar" @submit="onRename">
        <div class="collection-field">
          <label :for="renameId">{{ t("project.views.name") }}</label>
          <input
            :id="renameId"
            class="h-9 rounded-md border border-default bg-default px-3 text-sm"
            name="view-name"
            maxlength="100"
            :value="selected.name"
            :disabled="pending"
          />
        </div>
        <UButton type="submit" size="sm" variant="outline" color="neutral" :disabled="pending">
          {{ t("doc.tags.rename") }}
        </UButton>
      </form>
      <ConfirmActionButton
        :title="t('collection.deleteView')"
        :description="t('collection.deleteView.confirm')"
        :action-label="t('project.views.delete')"
        :disabled="pending"
        :action="deleteSelected"
      >
        {{ t("collection.deleteView") }}
      </ConfirmActionButton>
    </div>
    <div v-if="conflict && selected" role="alert" class="collection-toolbar">
      <span class="text-sm text-error">{{ t("task.savedView.conflict") }}</span>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        @click="
          conflict = false;
          emit('select', selected);
        "
      >
        {{ t("task.savedView.reload") }}
      </UButton>
    </div>
    <div v-if="views.isError.value" role="alert" class="collection-toolbar">
      <span class="text-sm text-error">{{ t("task.savedView.failed") }}</span>
      <UButton size="sm" variant="outline" color="neutral" @click="views.refetch()">{{ t("task.savedView.retry") }}</UButton>
    </div>
    <p v-if="error && !createOpen" role="alert" class="text-sm text-error">{{ error }}</p>
    <NativeModal :open="createOpen" :labelled-by="dialogTitleId" @close="createOpen = false">
      <form class="task-form" @submit.prevent="onCreateSubmit">
        <h2 :id="dialogTitleId" class="project-dialog__title">{{ t("task.savedView.createTitle") }}</h2>
        <p class="task-home__note">{{ t("task.savedView.createDescription") }}</p>
        <div class="task-form__field">
          <label :for="nameId" class="text-sm font-medium">{{ t("task.savedView.name") }}</label>
          <input
            :id="nameId"
            class="h-10 rounded-md border border-default bg-default px-3 text-sm"
            maxlength="100"
            autofocus
            :value="name"
            @input="name = ($event.target as HTMLInputElement).value"
          />
        </div>
        <div class="task-form__field">
          <label :for="typeId" class="text-sm font-medium">{{ t("common.view") }}</label>
          <select :id="typeId" class="collection-select" :value="type" @change="onCreateType">
            <option v-for="value in PROJECT_VIEW_TYPES" :key="value" :value="value">
              {{ projectViewTypeLabel(value) }}
            </option>
          </select>
        </div>
        <p v-if="error" role="alert" class="task-form__alert">{{ error }}</p>
        <div class="task-form__actions">
          <UButton variant="outline" color="neutral" @click="createOpen = false">{{ t("common.cancel") }}</UButton>
          <UButton type="submit" :disabled="create.isPending.value || !name.trim()">
            {{ create.isPending.value ? t("task.savedView.creating") : t("task.savedView.createAction") }}
          </UButton>
        </div>
      </form>
    </NativeModal>
  </div>
</template>
