<script setup lang="ts">
import { computed } from "vue";
import { RouterLink } from "vue-router";
import { t } from "@fvoci/i18n";
import { childrenOf, type ChildrenByParent } from "@/features/workspace/wiki-tree";
import { documentPath, wikiDisplayId } from "@/lib/href";
import type { TreeNode } from "@/lib/queries/documents";

defineOptions({ name: "WikiBranch" });

const props = defineProps<{
  slug: string;
  node: TreeNode;
  byParent: ChildrenByParent<TreeNode>;
  nested?: boolean;
}>();

const childNodes = computed(() => childrenOf(props.byParent, props.node.id));
const wikiRef = computed(() => wikiDisplayId(props.node.number));
const status = computed(() => {
  if (props.node.status === "draft") return t("doc.status.draft");
  if (props.node.status === "archived") return t("doc.status.archived");
  return null;
});
</script>

<template>
  <li :class="nested ? undefined : 'wiki-tree__branch'">
    <RouterLink
      :to="documentPath(slug, wikiRef)"
      :class="nested ? 'wiki-tree__row wiki-tree__row--nested' : 'wiki-tree__row'"
      :data-testid="`wiki-doc-${wikiRef}`"
    >
      <span v-if="node.icon" class="wiki-tree__icon" aria-hidden>{{ node.icon }}</span>
      <span class="wiki-tree__title">{{ node.title }}</span>
      <span v-if="status" class="wiki-tree__status">{{ status }}</span>
      <span class="wiki-tree__key">{{ wikiRef }}</span>
    </RouterLink>
    <ul v-if="childNodes.length > 0" class="wiki-tree wiki-tree--nested">
      <WikiBranch
        v-for="child in childNodes"
        :key="child.id"
        :slug="slug"
        :node="child"
        :by-parent="byParent"
        nested
      />
    </ul>
  </li>
</template>
