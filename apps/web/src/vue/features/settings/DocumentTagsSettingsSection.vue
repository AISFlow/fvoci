<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { documentTagPoolQuery, TAG_COLORS, type TagColor } from "@/lib/queries/collections";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import ConfirmAction from "./ConfirmAction.vue";
import "@/features/settings/settings-shell.css";
import "@/features/collections/collections.css";

const props = defineProps<{ workspaceId: string }>();
const queryClient = useQueryClient();
const nameId = useId();
const colorId = useId();
const name = ref("");
const color = ref<TagColor>("gray");
const actionError = ref<string | null>(null);
const tags = useQuery(() => documentTagPoolQuery(props.workspaceId));

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

async function invalidate(): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: ["document-tags", props.workspaceId] });
}

const create = useMutation({
  mutationFn: async (input: { name: string; color: TagColor }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/document-tags", {
        params: { path: { workspace_id: props.workspaceId } },
        body: input,
      }),
    ),
  onSuccess: async () => {
    actionError.value = null;
    name.value = "";
    color.value = "gray";
    await invalidate();
  },
  onError: (err) => {
    actionError.value = failMessage(err);
  },
});

const patch = useMutation({
  mutationFn: async (vars: { id: string; body: { name?: string; color?: TagColor } }) =>
    ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}/document-tags/{tag_id}", {
        params: { path: { workspace_id: props.workspaceId, tag_id: vars.id } },
        body: vars.body,
      }),
    ),
  onSuccess: async () => {
    actionError.value = null;
    await invalidate();
  },
  onError: async (err) => {
    actionError.value = failMessage(err);
    await invalidate();
  },
});

const remove = useMutation({
  mutationFn: async (id: string) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/document-tags/{tag_id}", {
        params: { path: { workspace_id: props.workspaceId, tag_id: id } },
      }),
    ),
  onSuccess: async () => {
    actionError.value = null;
    await invalidate();
  },
  onError: (err) => {
    actionError.value = failMessage(err);
  },
});

const canCreate = computed(() => tags.data.value?.canCreate ?? false);
const canManage = computed(() => tags.data.value?.canManage ?? false);
const items = computed(() => tags.data.value?.items ?? []);
const pending = computed(() => create.isPending.value || patch.isPending.value || remove.isPending.value);

function onCreate(): void {
  const trimmed = name.value.trim();
  if (trimmed === "" || pending.value) return;
  create.mutate({ name: trimmed, color: color.value });
}

function onRenameEnter(event: KeyboardEvent): void {
  if (!event.isComposing) (event.target as HTMLInputElement).blur();
}

function onRename(id: string, current: string, event: Event): void {
  const input = event.target as HTMLInputElement;
  const next = input.value.trim();
  if (next === "" || next === current) {
    input.value = current;
    return;
  }
  patch.mutate({ id, body: { name: next } });
}

function onColor(id: string, next: string): void {
  const found = TAG_COLORS.find((item) => item === next);
  if (found) patch.mutate({ id, body: { color: found } });
}
</script>

<template>
  <QueryLoading v-if="tags.isPending.value" />
  <QueryError
    v-else-if="tags.isError.value"
    :message="loadErrorMessage(tags.error.value)"
    @retry="tags.refetch()"
  />
  <section v-else class="settings-section" data-testid="document-tags-settings">
    <h1 class="settings-section__title">{{ t("settings.documentTags.title") }}</h1>
    <p v-if="actionError" role="alert" class="settings-notice settings-notice--danger">{{ actionError }}</p>
    <p v-if="items.length === 0" class="settings-section__lede">{{ t("settings.documentTags.empty") }}</p>
    <div v-else class="overflow-x-auto">
      <table class="w-full border-collapse">
        <thead>
          <tr>
            <th scope="col" class="border-b border-default px-2 py-2 text-left">{{ t("doc.tags.name") }}</th>
            <th scope="col" class="border-b border-default px-2 py-2 text-left">{{ t("doc.tags.color") }}</th>
            <th scope="col" class="border-b border-default px-2 py-2 text-left">{{ t("doc.tags.assignments") }}</th>
            <th v-if="canManage" scope="col" class="border-b border-default px-2 py-2">
              <span class="sr-only">{{ t("doc.tags.delete") }}</span>
            </th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="row in items" :key="row.id" :data-testid="`document-tag-row-${row.name}`">
            <td class="border-b border-default px-2 py-2">
              <input
                v-if="canManage"
                :key="`${row.id}:${row.name}`"
                class="h-9 w-full rounded-md border border-default bg-default px-3"
                :value="row.name"
                :aria-label="`${t('doc.tags.rename')}: ${row.name}`"
                maxlength="100"
                :disabled="pending"
                @keydown.enter="onRenameEnter($event)"
                @blur="onRename(row.id, row.name, $event)"
              />
              <span v-else>{{ row.name }}</span>
            </td>
            <td class="border-b border-default px-2 py-2">
              <select
                v-if="canManage"
                class="h-11 rounded-md border border-default bg-default px-3"
                :aria-label="t('doc.tags.color')"
                :value="row.color"
                :disabled="pending"
                @change="onColor(row.id, ($event.target as HTMLSelectElement).value)"
              >
                <option v-for="item in TAG_COLORS" :key="item" :value="item">{{ item }}</option>
              </select>
              <span v-else>{{ row.color }}</span>
            </td>
            <td class="border-b border-default px-2 py-2 settings-tabular">{{ row.assignmentCount }}</td>
            <td v-if="canManage" class="border-b border-default px-2 py-2">
              <ConfirmAction
                :title="t('doc.tags.delete.confirm.title')"
                :description="t('doc.tags.delete.confirm.body', { count: row.assignmentCount })"
                :action-label="t('doc.tags.delete')"
                :disabled="pending"
                :run="async () => { try { await remove.mutateAsync(row.id); } catch { /* onError shows the problem title. */ } }"
              >
                {{ t("doc.tags.delete") }}
              </ConfirmAction>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <form v-if="canCreate" class="settings-form__row" @submit.prevent="onCreate">
      <div>
        <label :for="nameId">{{ t("doc.tags.name") }}</label>
        <UInput :id="nameId" v-model="name" :maxlength="100" :disabled="pending" />
      </div>
      <div>
        <label :for="colorId">{{ t("doc.tags.color") }}</label>
        <select
          :id="colorId"
          v-model="color"
          class="h-11 rounded-md border border-default bg-default px-3"
          :aria-label="t('doc.tags.color')"
          :disabled="pending"
        >
          <option v-for="item in TAG_COLORS" :key="item" :value="item">{{ item }}</option>
        </select>
      </div>
      <UButton type="submit" size="sm" :disabled="pending || name.trim() === ''">{{ t("doc.tags.create.action") }}</UButton>
    </form>
  </section>
</template>
