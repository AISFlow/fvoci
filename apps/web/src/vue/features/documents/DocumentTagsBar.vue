<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, nextTick, onScopeDispose, ref, useId, useTemplateRef, watch } from "vue";
import {
  assignDocumentTag,
  createDocumentTag,
  removeDocumentTag,
} from "@/features/documents/document-tags-api";
import { meQuery } from "@/lib/queries";
import { loadErrorMessage, problemMessage } from "@/lib/api";
import {
  documentAssignedTagsQuery,
  documentTagPoolQuery,
  type DocumentTag,
} from "@/lib/queries/collections";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import "@/features/collections/collections.css";

// The document's tags with an inline picker (features/documents/document-tags-bar.tsx).
const props = defineProps<{
  workspaceId: string;
  documentId: string;
  projectId: string | null;
  readOnly: boolean;
}>();
const queryClient = useQueryClient();
const me = useQuery(meQuery);
const panelId = useId();
const trigger = useTemplateRef<{ $el?: Element }>("trigger");
const open = ref(false);
const filter = ref("");
const mutationError = ref<string | null>(null);
const assignedOptions = computed(() =>
  documentAssignedTagsQuery(props.workspaceId, props.documentId, props.projectId),
);
const assignedQuery = useQuery(assignedOptions);
const poolQuery = useQuery(() => ({
  ...documentTagPoolQuery(props.workspaceId),
  enabled: open.value && !props.readOnly,
}));

const assigned = computed(() => assignedQuery.data.value ?? []);
const assignedIds = computed(() => new Set(assigned.value.map((tag) => tag.id)));
const pool = computed(() => poolQuery.data.value?.items ?? []);
const needle = computed(() => filter.value.trim().toLowerCase());
const candidates = computed(() =>
  pool.value.filter(
    (tag) =>
      !assignedIds.value.has(tag.id) &&
      (needle.value === "" || tag.name.toLowerCase().includes(needle.value)),
  ),
);
const exact = computed(() => pool.value.find((tag) => tag.name.toLowerCase() === needle.value));
const canCreate = computed(
  () =>
    (poolQuery.data.value?.canCreate ?? false) && needle.value !== "" && exact.value === undefined,
);

type TagOperation = {
  workspaceId: string;
  documentId: string;
  projectId: string | null;
  lifecycle: number;
};
let operationLifecycle = 0;
watch(
  [
    () => props.workspaceId,
    () => props.documentId,
    () => props.projectId,
    () => props.readOnly,
    () => me.data.value?.userId,
    () => me.data.value?.sessionId,
  ],
  () => {
    operationLifecycle++;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  operationLifecycle++;
});
function captureOperation(): TagOperation {
  return {
    workspaceId: props.workspaceId,
    documentId: props.documentId,
    projectId: props.projectId,
    lifecycle: operationLifecycle,
  };
}
function currentOperation(operation: TagOperation): boolean {
  return operation.lifecycle === operationLifecycle;
}
async function invalidate(operation: TagOperation): Promise<void> {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["document-tags", operation.workspaceId] }),
    queryClient.invalidateQueries({ queryKey: ["wiki-discovery", operation.workspaceId] }),
  ]);
}

function close(): void {
  open.value = false;
  filter.value = "";
  const element = trigger.value?.$el;
  if (element instanceof HTMLElement) element.focus();
}

const assign = useMutation({
  mutationFn: (operation: TagOperation & { tagId: string }) =>
    assignDocumentTag(
      operation.workspaceId,
      operation.documentId,
      operation.projectId,
      operation.tagId,
    ),
  onMutate: (operation) => {
    if (currentOperation(operation)) mutationError.value = null;
  },
  onError: (err: unknown, operation) => {
    if (currentOperation(operation))
      mutationError.value = problemMessage(err, "error.http.fallback");
  },
  onSuccess: async (tag: DocumentTag, operation) => {
    queryClient.setQueryData(
      documentAssignedTagsQuery(operation.workspaceId, operation.documentId, operation.projectId)
        .queryKey,
      (items: DocumentTag[] = []) => [...items.filter((item) => item.id !== tag.id), tag],
    );
    await invalidate(operation);
    if (currentOperation(operation)) close();
  },
});

const create = useMutation({
  mutationFn: (operation: TagOperation & { name: string }) =>
    createDocumentTag(queryClient, operation.workspaceId, operation.name),
  onMutate: (operation) => {
    if (currentOperation(operation)) mutationError.value = null;
  },
  onError: (err: unknown, operation) => {
    if (currentOperation(operation))
      mutationError.value = problemMessage(err, "error.http.fallback");
  },
  onSuccess: async (created, operation) => {
    await assign.mutateAsync({ ...operation, tagId: created.id }).catch(() => undefined);
  },
});

const remove = useMutation({
  mutationFn: (operation: TagOperation & { tagId: string }) =>
    removeDocumentTag(
      operation.workspaceId,
      operation.documentId,
      operation.projectId,
      operation.tagId,
    ),
  onMutate: (operation) => {
    if (currentOperation(operation)) mutationError.value = null;
  },
  onError: (err: unknown, operation) => {
    if (currentOperation(operation))
      mutationError.value = problemMessage(err, "error.http.fallback");
  },
  onSuccess: async (_ok, operation) => {
    queryClient.setQueryData(
      documentAssignedTagsQuery(operation.workspaceId, operation.documentId, operation.projectId)
        .queryKey,
      (items: DocumentTag[] = []) => items.filter((tag) => tag.id !== operation.tagId),
    );
    await invalidate(operation);
  },
});

function assignTag(tagId: string): void {
  assign.mutate({ ...captureOperation(), tagId });
}
function createTag(name: string): void {
  create.mutate({ ...captureOperation(), name });
}
function removeTag(tagId: string): void {
  remove.mutate({ ...captureOperation(), tagId });
}

const pending = computed(
  () => assign.isPending.value || create.isPending.value || remove.isPending.value,
);
const hidden = computed(
  () => props.readOnly && assignedQuery.isSuccess.value && assigned.value.length === 0,
);
const filterInput = useTemplateRef<HTMLInputElement>("filterInput");

function toggle(): void {
  if (open.value) {
    close();
    return;
  }
  open.value = true;
  void nextTick(() => filterInput.value?.focus());
}

function onFilterKeydown(event: KeyboardEvent): void {
  if (event.key !== "Enter" || event.isComposing) return;
  event.preventDefault();
  const first =
    exact.value && !assignedIds.value.has(exact.value.id) ? exact.value : candidates.value[0];
  if (first) assignTag(first.id);
  else if (canCreate.value) createTag(filter.value.trim());
}

function onPanelKeydown(event: KeyboardEvent): void {
  if (event.key !== "Escape") return;
  event.preventDefault();
  close();
}
</script>

<template>
  <div
    v-if="!hidden"
    class="tags-bar"
    role="group"
    :aria-label="t('doc.tags')"
    data-testid="document-tags-bar"
  >
    <QueryLoading v-if="assignedQuery.isPending.value" />
    <QueryError
      v-if="assignedQuery.isError.value"
      :message="loadErrorMessage(assignedQuery.error.value)"
      @retry="assignedQuery.refetch()"
    />
    <span v-for="tag in assigned" :key="tag.id" class="tags-bar__item">
      <span class="tag-chip" :data-color="tag.color">{{ tag.name }}</span>
      <button
        v-if="!readOnly"
        type="button"
        class="tags-bar__remove"
        :aria-label="`${t('doc.tags.remove')}: ${tag.name}`"
        :disabled="pending"
        @click="removeTag(tag.id)"
      >
        <span aria-hidden="true">×</span>
      </button>
    </span>
    <UButton
      v-if="!readOnly"
      ref="trigger"
      size="sm"
      variant="outline"
      color="neutral"
      :aria-expanded="open"
      :aria-controls="panelId"
      @click="toggle"
    >
      {{ t("doc.tags.add") }}
    </UButton>
    <div v-if="open && !readOnly" :id="panelId" class="tags-bar__picker" @keydown="onPanelKeydown">
      <input
        ref="filterInput"
        v-model="filter"
        class="h-9 w-full rounded-md border border-default bg-default px-3 text-sm"
        :aria-label="t('doc.tags')"
        maxlength="100"
        :disabled="pending"
        @keydown="onFilterKeydown"
      />
      <QueryLoading v-if="poolQuery.isPending.value" />
      <QueryError
        v-if="poolQuery.isError.value"
        :message="loadErrorMessage(poolQuery.error.value)"
        @retry="poolQuery.refetch()"
      />
      <ul v-if="poolQuery.isSuccess.value" class="tags-bar__options">
        <li v-for="tag in candidates" :key="tag.id">
          <button
            type="button"
            class="tags-bar__option"
            :disabled="pending"
            @click="assignTag(tag.id)"
          >
            <span class="tag-chip" :data-color="tag.color">{{ tag.name }}</span>
          </button>
        </li>
        <li v-if="canCreate">
          <button
            type="button"
            class="tags-bar__option"
            :disabled="pending"
            @click="createTag(filter.trim())"
          >
            {{ t("doc.tags.create", { name: filter.trim() }) }}
          </button>
        </li>
        <li v-if="candidates.length === 0 && !canCreate" class="px-2 py-1 text-sm text-muted">{{
          t("doc.tags.empty")
        }}</li>
      </ul>
    </div>
    <p v-if="mutationError" role="alert" class="basis-full text-sm text-error">{{
      mutationError
    }}</p>
  </div>
</template>
