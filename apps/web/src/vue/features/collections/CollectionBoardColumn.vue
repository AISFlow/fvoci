<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, watch } from "vue";
import {
  BOARD_DRAG_TYPE,
  columnRows,
  moveChoices,
  moveRequest,
  type BoardGroup,
} from "@/features/collections/board-model";
import { isInvalidCursor, loadErrorMessage } from "@/lib/api";
import {
  collectionBoardColumnQuery,
  type CollectionConfig,
  type CollectionQueryItem,
} from "@/lib/queries/collections";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import "@/features/collections/collections.css";

const props = defineProps<{
  workspaceId: string;
  collectionId: string;
  config: CollectionConfig;
  groups: readonly BoardGroup[];
  moving: boolean;
  group: BoardGroup;
  drag: { row: CollectionQueryItem | null; over: string | null | undefined };
}>();
const emit = defineEmits<{ move: [row: CollectionQueryItem, target: BoardGroup] }>();

defineSlots<{
  default(props: { row: CollectionQueryItem }): unknown;
}>();

const queryClient = useQueryClient();
const columnQuery = computed(() =>
  collectionBoardColumnQuery(props.workspaceId, props.collectionId, props.config, props.group.id),
);
const pages = useInfiniteQuery(() => ({
  ...columnQuery.value,
  enabled: Boolean(props.workspaceId) && Boolean(props.collectionId),
}));
const rows = computed(() => columnRows(pages.data.value?.pages ?? [], props.group.id));
const choices = computed(() => moveChoices(props.config.groupBy, props.groups));
const invalidCursor = computed(() => isInvalidCursor(pages.error.value));
const groupName = computed(() => props.group.name || t("collection.unassigned"));

watch(invalidCursor, (invalid) => {
  if (invalid) void queryClient.resetQueries({ queryKey: columnQuery.value.queryKey, exact: true });
});

function accepts(row: CollectionQueryItem | null): boolean {
  return (
    !props.moving && row !== null && moveRequest(props.config.groupBy, row, props.group) !== null
  );
}

function onDragStart(event: DragEvent, row: CollectionQueryItem, movable: boolean): void {
  if (!movable) return;
  event.dataTransfer?.setData(BOARD_DRAG_TYPE, row.id);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  props.drag.row = row;
}

function onDragEnd(): void {
  props.drag.row = null;
  props.drag.over = undefined;
}

function onDragOver(event: DragEvent): void {
  if (!accepts(props.drag.row)) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
  if (props.drag.over !== props.group.id) props.drag.over = props.group.id;
}

function onDragLeave(event: DragEvent): void {
  const current = event.currentTarget as Node | null;
  if (current && !current.contains(event.relatedTarget as Node | null)) props.drag.over = undefined;
}

function onDrop(event: DragEvent): void {
  const row = props.drag.row;
  props.drag.row = null;
  props.drag.over = undefined;
  if (!row || !accepts(row) || event.dataTransfer?.getData(BOARD_DRAG_TYPE) !== row.id) return;
  event.preventDefault();
  emit("move", row, props.group);
}

function onSelectMove(row: CollectionQueryItem, event: Event): void {
  const target = props.groups.find(
    (item) => (item.id ?? "") === (event.target as HTMLSelectElement).value,
  );
  if (target) emit("move", row, target);
}
</script>

<template>
  <section
    class="collection-board__column"
    :aria-label="groupName"
    :data-testid="`collection-group-${group.name || 'none'}`"
    :data-drop-over="drag.over === group.id ? 'true' : undefined"
    @dragover="onDragOver"
    @dragleave="onDragLeave"
    @drop="onDrop"
  >
    <h3 class="collection-board__head">
      <span>{{ groupName }}{{ group.deleted ? ` · ${t("collection.archived")}` : "" }}</span>
      <span class="text-muted">{{ group.count }}</span>
    </h3>
    <QueryLoading v-if="pages.isPending.value" />
    <QueryError
      v-if="pages.isError.value && !invalidCursor && !pages.isFetchNextPageError.value"
      :message="loadErrorMessage(pages.error.value)"
      @retry="pages.refetch()"
    />
    <p v-else-if="pages.data.value && rows.length === 0" class="text-sm text-muted">{{
      t("collection.emptyPage")
    }}</p>
    <ul v-else-if="pages.data.value" class="flex flex-col gap-2">
      <li
        v-for="row in rows"
        :key="row.id"
        class="collection-card"
        :data-testid="`collection-card-${row.displayId}`"
        :draggable="row.canEdit && !moving && choices.length > 0"
        :aria-busy="moving || undefined"
        @dragstart="onDragStart($event, row, row.canEdit && !moving && choices.length > 0)"
        @dragend="onDragEnd"
      >
        <slot :row="row" />
        <select
          v-if="row.canEdit && choices.length > 0"
          class="collection-select"
          :aria-label="`${t('collection.group')} · ${row.displayId}`"
          :value="row.group ?? ''"
          :disabled="moving"
          @change="onSelectMove(row, $event)"
        >
          <option
            v-for="choice in choices"
            :key="choice.id ?? 'none'"
            :value="choice.id ?? ''"
            :disabled="choice.disabled && choice.id !== row.group"
          >
            {{ choice.name || t("collection.unassigned")
            }}{{ choice.disabled ? ` · ${t("collection.archived")}` : "" }}
          </option>
        </select>
      </li>
    </ul>
    <p
      v-if="pages.isFetchNextPageError.value && !invalidCursor"
      role="alert"
      class="text-sm text-error"
    >
      {{ t("search.loadMoreError") }}
    </p>
    <UButton
      v-if="pages.hasNextPage.value"
      size="sm"
      variant="outline"
      color="neutral"
      :aria-label="`${groupName} · ${t('search.loadMore')}`"
      :disabled="pages.isFetchingNextPage.value"
      @click="pages.fetchNextPage()"
    >
      {{ t("search.loadMore") }}
    </UButton>
  </section>
</template>
