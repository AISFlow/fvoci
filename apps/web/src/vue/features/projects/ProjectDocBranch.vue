<script setup lang="ts">
import { computed } from "vue";
import type { TreeNode } from "@/features/projects/queries";
import { childrenOf, type ChildrenByParent } from "@/features/workspace/wiki-tree";
import { documentPath } from "@/lib/href";
import AppLink from "../../components/AppLink.vue";

defineOptions({ name: "ProjectDocBranch" });

const props = withDefaults(
  defineProps<{
    slug: string;
    projectKey: string;
    node: TreeNode;
    byParent: ChildrenByParent<TreeNode>;
    nested?: boolean;
  }>(),
  { nested: false },
);

const childNodes = computed(() => childrenOf(props.byParent, props.node.id));
const displayId = computed(() => `${props.projectKey}-${String(props.node.number)}`);
</script>

<template>
  <li :class="nested ? undefined : 'wiki-tree__branch'">
    <AppLink
      :to="documentPath(slug, displayId)"
      :class="nested ? 'wiki-tree__row wiki-tree__row--nested' : 'wiki-tree__row'"
      :data-testid="`project-doc-${displayId}`"
    >
      <span v-if="node.icon" class="wiki-tree__icon" aria-hidden>{{ node.icon }}</span>
      <span class="wiki-tree__title">{{ node.title }}</span>
      <span class="wiki-tree__key">{{ displayId }}</span>
    </AppLink>
    <ul v-if="childNodes.length > 0" class="wiki-tree wiki-tree--nested">
      <ProjectDocBranch
        v-for="child in childNodes"
        :key="child.id"
        :slug="slug"
        :project-key="projectKey"
        :node="child"
        :by-parent="byParent"
        nested
      />
    </ul>
  </li>
</template>
