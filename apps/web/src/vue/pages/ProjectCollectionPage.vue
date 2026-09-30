<script setup lang="ts">
import { computed } from "vue";
import { useRoute } from "vue-router";
import type { CollectionViewType } from "@/features/collections/collection-view";
import CollectionPanel from "../features/collections/CollectionPanel.vue";
import ProjectViewFrame from "../features/projects/ProjectViewFrame.vue";

const route = useRoute();
const type = computed<CollectionViewType>(() => {
  const name = String(route.name ?? "");
  if (name === "project-board") return "board";
  if (name === "project-calendar") return "calendar";
  return "table";
});
</script>

<template>
  <ProjectViewFrame v-slot="{ slug, workspace, project }" :active="type">
    <CollectionPanel :slug="slug" :workspace="workspace" :project="project" :type="type" />
  </ProjectViewFrame>
</template>
