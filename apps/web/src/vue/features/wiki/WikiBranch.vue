<script setup lang="ts">
import { computed } from "vue";
import { RouterLink } from "vue-router";
import { t } from "@fvoci/i18n";
import { childrenOf, type ChildrenByParent } from "@/features/workspace/wiki-tree";
import { documentPath, wikiDisplayId, formatDisplayId } from "@/lib/href";
import type { TreeNode } from "@/lib/queries/documents";

defineOptions({ name: "WikiBranch" });

const props = defineProps<{
  slug: string;
  node: TreeNode;
  byParent: ChildrenByParent<TreeNode>;
  nested?: boolean;
  flat?: boolean;
  projectKeys?: ReadonlyMap<string, string>;
  draggable?: boolean;
}>();
const emit = defineEmits<{
  dropDocument: [sourceId: string, destId: string, position: "top" | "bottom" | "onto"];
}>();
function onDragStart(event: DragEvent): void {
  if (!props.draggable || (props.node.projectId && props.node.parentId === null)) {
    event.preventDefault();
    return;
  }
  event.dataTransfer?.setData("application/x-fvoci-document", props.node.id);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
}
function onDrop(event: DragEvent): void {
  const id = event.dataTransfer?.getData("application/x-fvoci-document");
  if (!props.draggable || !id) return;
  event.preventDefault();
  const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const ratio = (event.clientY - box.top) / Math.max(box.height, 1);
  emit("dropDocument", id, props.node.id, ratio < 0.28 ? "top" : ratio > 0.72 ? "bottom" : "onto");
}

const childNodes = computed(() => childrenOf(props.byParent, props.node.id));
const wikiRef = computed(() => {
  if (!props.node.projectId) return wikiDisplayId(props.node.number);
  const projectKey = props.projectKeys?.get(props.node.projectId);
  return projectKey ? formatDisplayId(projectKey, props.node.number) : null;
});
const status = computed(() => {
  if (props.node.status === "draft") return t("doc.status.draft");
  if (props.node.status === "archived") return t("doc.status.archived");
  return null;
});
</script>

<template>
  <li :class="nested ? undefined : 'wiki-tree__branch'">
    <RouterLink
      v-if="wikiRef"
      :to="documentPath(slug, wikiRef)"
      :class="nested ? 'wiki-tree__row wiki-tree__row--nested' : 'wiki-tree__row'"
      :data-testid="`wiki-doc-${wikiRef}`"
      :draggable="draggable && !(node.projectId && node.parentId === null)"
      @dragstart="onDragStart"
      @dragover="draggable && $event.preventDefault()"
      @drop.stop="onDrop"
    >
      <span v-if="node.icon" class="wiki-tree__icon" aria-hidden>{{ node.icon }}</span>
      <span class="wiki-tree__title">{{ node.title }}</span>
      <span v-if="status" class="wiki-tree__status">{{ status }}</span>
      <span class="wiki-tree__key">{{ wikiRef }}</span>
    </RouterLink>
    <ul v-if="!flat && childNodes.length > 0" class="wiki-tree wiki-tree--nested">
      <WikiBranch
        v-for="child in childNodes"
        :key="child.id"
        :slug="slug"
        :node="child"
        :by-parent="byParent"
        nested
        :project-keys="projectKeys"
        :draggable="draggable"
        @drop-document="(source, dest, position) => emit('dropDocument', source, dest, position)"
      />
    </ul>
  </li>
</template>
