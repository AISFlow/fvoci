<script setup lang="ts">
import type { ShareTreeNode } from "@/lib/queries/share";
import { shareTreeChildren } from "@/lib/share-links";
import { computed } from "vue";
import PublicTreeBranch from "./PublicTreeBranch.vue";

const props = defineProps<{
  nodes: readonly ShareTreeNode[];
  node: ShareTreeNode;
  depth: number;
  activeDocumentId: string | null;
}>();

const emit = defineEmits<{ select: [documentId: string] }>();
const children = computed(() => shareTreeChildren(props.nodes, props.node.id));
const isActive = computed(() => props.node.id === props.activeDocumentId);
</script>

<template>
  <li>
    <button
      type="button"
      :class="['share-page__tree-row', isActive && 'is-active']"
      :style="{ paddingLeft: `${depth * 0.75 + 0.375}rem` }"
      :aria-current="isActive ? 'page' : undefined"
      @click="emit('select', node.id)"
    >
      <span v-if="node.icon" aria-hidden>{{ node.icon }}</span>
      <span class="min-w-0 truncate break-keep">{{ node.title }}</span>
    </button>
    <ul v-if="children.length > 0">
      <PublicTreeBranch
        v-for="child in children"
        :key="child.id"
        :nodes="nodes"
        :node="child"
        :depth="depth + 1"
        :active-document-id="activeDocumentId"
        @select="emit('select', $event)"
      />
    </ul>
  </li>
</template>
