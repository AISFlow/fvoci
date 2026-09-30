<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed } from "vue";
import { asCollectionValue, type CollectionValue } from "@/lib/collection-values";
import { FALLBACK_TZ } from "@/lib/datetime";
import { loadErrorMessage } from "@/lib/api";
import { membersQuery, meQuery } from "@/lib/queries";
import {
  collectionFieldsQuery,
  collectionPrefix,
  putCollectionValue,
  taskCollectionItemQuery,
  type CollectionField,
} from "@/lib/queries/collections";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import ValueEditor from "./ValueEditor.vue";
import "@/features/collections/collections.css";
import "@/features/settings/settings-shell.css";

const props = defineProps<{
  workspaceId: string;
  taskId: string;
  readOnly: boolean;
}>();

const queryClient = useQueryClient();
const lookup = useQuery(() => taskCollectionItemQuery(props.workspaceId, props.taskId));
const item = computed(() => lookup.data.value?.item ?? null);
const collectionId = computed(() => item.value?.collectionId ?? "");
const fields = useQuery(() => collectionFieldsQuery(props.workspaceId, collectionId.value));
const members = useQuery(() => membersQuery(props.workspaceId));
const me = useQuery(meQuery);
const timeZone = computed(() => me.data.value?.timezone ?? FALLBACK_TZ);
const active = computed(() => (fields.data.value?.items ?? []).filter((field) => field.deletedAt === null));
const hidden = computed(
  () =>
    (lookup.isSuccess.value && lookup.data.value?.item === null) ||
    (fields.isSuccess.value && active.value.length === 0),
);
const error = computed(() => lookup.error.value ?? fields.error.value);

async function retryLoad(): Promise<void> {
  await lookup.refetch();
  await fields.refetch();
}

async function save(field: CollectionField, value: CollectionValue): Promise<void> {
  const current = item.value;
  if (!current) return;
  try {
    await putCollectionValue(props.workspaceId, collectionId.value, current.id, {
      fieldId: field.id,
      expectedVersion: current.version,
      expectedFieldVersion: field.version,
      value,
    });
  } finally {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: collectionPrefix(props.workspaceId, collectionId.value) }),
      queryClient.invalidateQueries({ queryKey: ["collection-item", props.workspaceId, "task", props.taskId] }),
    ]);
  }
}
</script>

<template>
  <section
    v-if="!hidden"
    class="settings-section mt-6"
    data-testid="task-properties"
    aria-labelledby="task-properties-title"
  >
    <h2 id="task-properties-title" class="settings-section__title">{{ t("task.properties") }}</h2>
    <QueryLoading v-if="lookup.isPending.value || fields.isPending.value" />
    <QueryError
      v-if="error"
      :message="loadErrorMessage(error)"
      @retry="retryLoad"
    />
    <div v-if="item && fields.data.value" class="grid gap-3 sm:grid-cols-2">
      <ValueEditor
        v-for="field in active"
        :key="field.id"
        :field="field"
        :value="asCollectionValue((lookup.data.value?.values as Record<string, unknown> | undefined)?.[field.id])"
        :members="members.data.value?.items ?? []"
        :time-zone="timeZone"
        :read-only="readOnly || !(lookup.data.value?.canEdit ?? false)"
        :save-value="(value) => save(field, value)"
      />
    </div>
  </section>
</template>
