<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { taskParentListQuery } from "@/features/tasks/queries";

const props = withDefaults(
  defineProps<{
    workspaceId: string;
    projectId: string;
    childType: string;
    excludeTaskId: string;
    modelValue: string | null;
    currentTitle?: string;
    disabled?: boolean;
  }>(),
  { disabled: false },
);
const emit = defineEmits<{ "update:modelValue": [value: string | null] }>();

const open = ref(false);
const query = ref("");
const selected = ref<{ id: string; title: string } | null>(null);
const q = computed(() => query.value.trim());

const list = useInfiniteQuery(() => ({
  ...taskParentListQuery(
    props.workspaceId,
    props.projectId,
    props.childType,
    props.excludeTaskId,
    q.value,
  ),
  enabled:
    open.value &&
    Boolean(props.workspaceId) &&
    Boolean(props.projectId) &&
    props.childType !== "epic",
}));

watch(
  () => [props.childType, props.excludeTaskId] as const,
  () => {
    open.value = false;
    query.value = "";
    selected.value = null;
  },
);

const items = computed(() => list.data.value?.pages.flatMap((page) => page.items) ?? []);
const label = computed(() => {
  if (!props.modelValue) return t("task.parent.none");
  if (selected.value?.id === props.modelValue) return selected.value.title;
  return props.currentTitle ?? t("task.parent.current");
});
const listPending = computed(() => open.value && list.isPending.value);
const listError = computed(() => (open.value ? list.error.value : null));
const showEmpty = computed(() => items.value.length === 0 && !list.hasNextPage.value);
const triggerDisabled = computed(
  () => props.disabled || props.childType === "epic" || !props.workspaceId || !props.projectId,
);

function pick(id: string | null, title?: string): void {
  if (id && title) selected.value = { id, title };
  else selected.value = null;
  emit("update:modelValue", id);
  open.value = false;
}
</script>

<template>
  <div class="task-parent-select">
    <UButton
      id="task-edit-parent"
      type="button"
      size="sm"
      variant="outline"
      color="neutral"
      class="task-parent-select__trigger"
      data-testid="task-edit-parent"
      :aria-expanded="open"
      aria-haspopup="listbox"
      :disabled="triggerDisabled"
      @click="open = !open"
    >
      {{ label }}
    </UButton>
    <div v-if="open" class="task-parent-select__panel">
      <input
        v-model="query"
        class="task-form__field-input"
        :aria-label="t('task.parent.search')"
        :placeholder="t('task.parent.search')"
        data-testid="task-edit-parent-search"
        :disabled="disabled"
      />
      <p v-if="listPending" role="status" class="task-home__note">{{ t("task.parent.loading") }}</p>
      <div v-else-if="listError" role="alert">
        <p class="task-form__alert">{{ t("task.parent.failed") }}</p>
        <UButton size="sm" variant="outline" color="neutral" @click="list.refetch()">{{
          t("task.parent.retry")
        }}</UButton>
      </div>
      <ul
        v-else
        class="task-parent-select__list"
        role="listbox"
        :aria-label="t('task.parent.label')"
        :aria-busy="list.isFetching.value"
      >
        <li v-if="childType !== 'subtask'" role="presentation">
          <button
            type="button"
            role="option"
            class="task-parent-select__option"
            :aria-selected="modelValue == null"
            data-testid="task-edit-parent-none"
            @click="pick(null)"
          >
            {{ t("task.parent.none") }}
          </button>
        </li>
        <li v-for="item in items" :key="item.id" role="presentation">
          <button
            type="button"
            role="option"
            class="task-parent-select__option"
            :aria-selected="modelValue === item.id"
            :data-testid="`task-edit-parent-option-${item.id}`"
            @click="pick(item.id, `${item.displayId} ${item.title}`)"
          >
            {{ item.displayId }} {{ item.title }}
          </button>
        </li>
        <li v-if="showEmpty" role="presentation" class="task-home__note">{{
          t("task.parent.empty")
        }}</li>
      </ul>
      <UButton
        v-if="list.hasNextPage.value"
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="list.isFetchingNextPage.value"
        @click="list.fetchNextPage()"
      >
        {{ t("task.parent.more") }}
      </UButton>
    </div>
  </div>
</template>
