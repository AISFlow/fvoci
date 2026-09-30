<script setup lang="ts">
import { reactive } from "vue";
import type { BoardGroup } from "@/features/collections/board-model";
import type { CollectionConfig, CollectionQueryItem } from "@/lib/queries/collections";
import CollectionBoardColumn from "./CollectionBoardColumn.vue";
import "@/features/collections/collections.css";

defineProps<{
  workspaceId: string;
  collectionId: string;
  config: CollectionConfig;
  groups: readonly BoardGroup[];
  moving: boolean;
}>();
const emit = defineEmits<{ move: [row: CollectionQueryItem, target: BoardGroup] }>();
defineSlots<{
  default(props: { row: CollectionQueryItem }): unknown;
}>();

const drag = reactive<{ row: CollectionQueryItem | null; over: string | null | undefined }>({
  row: null,
  over: undefined,
});

function onDragChange(next: typeof drag): void {
  drag.row = next.row;
  drag.over = next.over;
}

function onMove(row: CollectionQueryItem, target: BoardGroup): void {
  emit("move", row, target);
}
</script>

<template>
  <div class="collection-board" data-testid="collection-board">
    <CollectionBoardColumn
      v-for="group in groups"
      :key="group.id ?? 'none'"
      :workspace-id="workspaceId"
      :collection-id="collectionId"
      :config="config"
      :groups="groups"
      :moving="moving"
      :group="group"
      :drag="drag"
      @move="onMove"
      @drag-change="onDragChange"
    >
      <template #default="{ row }">
        <slot :row="row" />
      </template>
    </CollectionBoardColumn>
  </div>
</template>
